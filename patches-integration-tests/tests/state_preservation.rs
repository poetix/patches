//! Integration tests: module state preservation across replans.
//!
//! Validates that a module surviving a re-plan (same `InstanceId` in both the
//! old and new graph) retains its internal state in the audio-thread module
//! pool, and that a module replaced by a fresh instance starts from its
//! default state.
//!
//! ## Mechanism (post-E009)
//!
//! State preservation works through the audio-thread-owned module pool
//! (ADR-0009). Surviving modules stay in the pool between plan swaps and
//! continue running — their state is preserved automatically because the pool
//! slot is unchanged across replans. `Planner::build` is called without any
//! `prev_plan` argument; the running plan is owned by the audio thread and
//! inaccessible to the control thread.
//!
//! ## Test fixture: StatefulCounter
//!
//! `StatefulCounter` is a minimal module whose only state is a `u64` count
//! incremented on each `process` call.

use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleGraph, NodeId, PortDescriptor,
    PortRef,
};
use patches_engine::{ExecutionPlan, Planner};
use patches_modules::AudioOut;

// ── StatefulCounter ───────────────────────────────────────────────────────────

struct StatefulCounter {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    count: u64,
}

impl StatefulCounter {
    fn new() -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
            count: 0,
        }
    }

    fn with_id(id: InstanceId) -> Self {
        Self {
            instance_id: id,
            descriptor: ModuleDescriptor {
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
            count: 0,
        }
    }
}

impl Module for StatefulCounter {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, _inputs: &[f64], outputs: &mut [f64]) {
        self.count += 1;
        outputs[0] = self.count as f64;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ── HeadlessEngine ────────────────────────────────────────────────────────────

struct HeadlessEngine {
    plan: Option<ExecutionPlan>,
    buffer_pool: Vec<[f64; 2]>,
    module_pool: Vec<Option<Box<dyn Module>>>,
    wi: usize,
    env: AudioEnvironment,
}

impl HeadlessEngine {
    fn new(
        plan: ExecutionPlan,
        buffer_pool_capacity: usize,
        module_pool_capacity: usize,
        env: AudioEnvironment,
    ) -> Self {
        let mut engine = Self {
            plan: None,
            buffer_pool: vec![[0.0; 2]; buffer_pool_capacity],
            module_pool: (0..module_pool_capacity).map(|_| None).collect(),
            wi: 0,
            env,
        };
        engine.adopt_plan(plan);
        engine
    }

    fn adopt_plan(&mut self, mut plan: ExecutionPlan) {
        // Tombstone first: the freelist may recycle tombstoned slots for new modules.
        for &idx in &plan.tombstones {
            self.module_pool[idx].take();
        }
        for (idx, mut m) in plan.new_modules.drain(..) {
            m.initialise(&self.env);
            self.module_pool[idx] = Some(m);
        }
        for &i in &plan.to_zero {
            self.buffer_pool[i] = [0.0; 2];
        }
        self.plan = Some(plan);
    }

    fn tick(&mut self) {
        let plan = self.plan.as_mut().expect("HeadlessEngine::tick: no current plan");
        plan.tick(&mut self.module_pool, &mut self.buffer_pool, self.wi);
        self.wi = 1 - self.wi;
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

const BUFFER_POOL_CAPACITY: usize = 256;
const MODULE_POOL_CAPACITY: usize = 64;
const ENV: AudioEnvironment = AudioEnvironment { sample_rate: 48_000.0 };

fn p(name: &'static str) -> PortRef {
    PortRef { name, index: 0 }
}

fn counter_graph(counter: StatefulCounter) -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let counter_id = NodeId::from("counter");
    let out_id = NodeId::from("out");
    graph.add_module(counter_id.clone(), Box::new(counter)).unwrap();
    graph.add_module(out_id.clone(), Box::new(AudioOut::new())).unwrap();
    graph.connect(&counter_id, p("out"), &out_id, p("left"), 1.0).unwrap();
    graph.connect(&counter_id, p("out"), &out_id, p("right"), 1.0).unwrap();
    graph
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// A module surviving a re-plan must retain its internal state.
///
/// Timeline:
///   1. Build plan A with `StatefulCounter` at NodeId "counter" (InstanceId = id_a).
///   2. Tick 10 samples via HeadlessEngine → module in the pool has count=10.
///   3. Build plan B with a new `StatefulCounter` that shares the same InstanceId
///      as plan A's counter (so the planner sees it as a surviving module).
///   4. Adopt plan B — counter's pool slot is unchanged, count remains 10.
///   5. Tick once more → count=11.
#[test]
fn replan_preserves_state_for_surviving_instance() {
    let mut planner = Planner::with_capacity(BUFFER_POOL_CAPACITY);

    let counter_a = StatefulCounter::new();
    let counter_id = counter_a.instance_id();

    let plan_a = planner.build(counter_graph(counter_a)).unwrap();

    // Find counter's pool index before adoption drains new_modules.
    let counter_pool_idx = plan_a
        .new_modules
        .iter()
        .find(|(_, m)| m.as_any().downcast_ref::<StatefulCounter>().is_some())
        .map(|(idx, _)| *idx)
        .expect("StatefulCounter not in plan_a.new_modules");

    let mut engine = HeadlessEngine::new(plan_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY, ENV);

    for _ in 0..10 {
        engine.tick();
    }

    // Build plan B with a fresh StatefulCounter sharing the same InstanceId.
    // The planner sees counter_id in module_alloc_state → surviving → not in new_modules.
    let counter_b = StatefulCounter::with_id(counter_id);
    let plan_b = planner.build(counter_graph(counter_b)).unwrap();

    assert!(
        plan_b.new_modules.iter().all(|(_, m)| m.as_any().downcast_ref::<StatefulCounter>().is_none()),
        "surviving StatefulCounter must not appear in plan_b.new_modules"
    );

    engine.adopt_plan(plan_b);
    engine.tick();

    let count = engine.module_pool[counter_pool_idx]
        .as_ref()
        .expect("counter must still be in pool")
        .as_any()
        .downcast_ref::<StatefulCounter>()
        .expect("must be StatefulCounter")
        .count;

    assert_eq!(count, 11, "state must be preserved: count was 10, ticked once → 11");
}

/// A module replaced by a fresh instance of the same type must start from its
/// default state.
///
/// Timeline:
///   1. Build plan A with `StatefulCounter`. Tick 10 times → count=10.
///   2. Build plan B with a completely fresh `StatefulCounter` (new InstanceId).
///      Plan A's counter is tombstoned; plan B's counter gets a new pool slot
///      with count=0.
///   3. Tick once → count=1.
#[test]
fn replan_fresh_instance_starts_from_default_state() {
    let mut planner = Planner::with_capacity(BUFFER_POOL_CAPACITY);

    let plan_a = planner.build(counter_graph(StatefulCounter::new())).unwrap();
    let mut engine = HeadlessEngine::new(plan_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY, ENV);

    for _ in 0..10 {
        engine.tick();
    }

    // Build plan B with a completely fresh StatefulCounter (new InstanceId).
    let plan_b = planner.build(counter_graph(StatefulCounter::new())).unwrap();

    // Find the new counter's pool index.
    let new_counter_pool_idx = plan_b
        .new_modules
        .iter()
        .find(|(_, m)| m.as_any().downcast_ref::<StatefulCounter>().is_some())
        .map(|(idx, _)| *idx)
        .expect("fresh StatefulCounter must be in plan_b.new_modules");

    engine.adopt_plan(plan_b);
    engine.tick();

    let count = engine.module_pool[new_counter_pool_idx]
        .as_ref()
        .expect("counter must be in pool")
        .as_any()
        .downcast_ref::<StatefulCounter>()
        .expect("must be StatefulCounter")
        .count;

    assert_eq!(count, 1, "fresh replacement instance must start from count=0, ticked once → 1");
}
