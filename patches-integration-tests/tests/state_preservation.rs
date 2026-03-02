//! Integration tests: module state preservation across replans.
//!
//! Validates that a module surviving a re-plan (same `NodeId` in both the old
//! and new graph) retains its internal state, and that a module replaced by a
//! fresh instance of the same type starts from its default state.
//!
//! ## Intended mechanism (post-E009)
//!
//! State preservation works through the audio-thread-owned module pool
//! (ADR-0009). Surviving modules stay in the pool between plan swaps and
//! continue running — their state is preserved automatically without any
//! `prev_plan` argument. These tests reflect that intended behaviour:
//! `Planner::build` is called with `None` (mirroring the real engine flow where
//! the running plan is owned by the audio thread and inaccessible to the
//! control thread).
//!
//! ## Current status
//!
//! Both tests are marked `#[ignore]` because the module pool is not yet
//! implemented (E009). Under the current design, calling `Planner::build` with
//! `prev_plan = None` produces a plan with fresh, stateless modules — so
//! `replan_preserves_state_for_surviving_instance` fails at its assertion.
//! The tests should be re-enabled and their API calls updated (tick signature,
//! pool access) once T-0043 and T-0045 are complete.
//!
//! ## Test fixture: StatefulCounter
//!
//! `StatefulCounter` is a minimal module whose only state is a `u64` count
//! incremented on each `process` call.

use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleGraph, NodeId, PortDescriptor,
    PortRef,
};
use patches_engine::Planner;
use patches_modules::AudioOut;

// ── StatefulCounter ───────────────────────────────────────────────────────────

/// A module that outputs its call count as `f64`.
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

// ── Helpers ───────────────────────────────────────────────────────────────────

const POOL_CAPACITY: usize = 256;
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
///   1. Build plan A with `StatefulCounter` at NodeId "counter".
///   2. Tick 10 samples via HeadlessEngine → module in the pool has count=10.
///   3. Build plan B with a new `StatefulCounter` at the same NodeId "counter".
///   4. `Planner::build` is called with `prev_plan = None`: the surviving module
///      stays in the audio-thread pool and is referenced by the same pool index.
///   5. Tick once more → count=11.
///
/// This test currently FAILS under the pre-E009 design (prev_plan=None produces
/// a fresh module with count=0). It documents the correct post-E009 behaviour.
#[test]
#[ignore = "requires E009 module pool; currently fails because prev_plan=None produces a fresh module"]
fn replan_preserves_state_for_surviving_instance() {
    let mut planner = Planner::with_capacity(POOL_CAPACITY);

    let mut plan_a = planner.build(counter_graph(StatefulCounter::new()), None).unwrap();
    plan_a.initialise(&ENV);

    let mut pool = vec![[0.0f64; 2]; POOL_CAPACITY];
    for i in 0..10usize {
        plan_a.tick(&mut pool, i % 2);
    }

    // Build plan B: same NodeId "counter", new StatefulCounter instance.
    // Under E009 the Planner recognises the NodeId as a surviving module and
    // keeps the existing pool entry — no prev_plan argument needed.
    // TODO(T-0045): update tick call and counter access for the module pool API.
    let plan_b = planner.build(counter_graph(StatefulCounter::new()), None).unwrap();

    let count_b = plan_b
        .slots
        .iter()
        .find_map(|s| s.module.as_any().downcast_ref::<StatefulCounter>())
        .expect("StatefulCounter not found in plan_b")
        .count;

    assert_eq!(
        count_b, 10,
        "surviving module's state (count=10) must be preserved across the replan"
    );
}

/// A module replaced by a fresh instance of the same type must start from its
/// default state.
///
/// Timeline:
///   1. Build plan A with `StatefulCounter` at NodeId "counter". Tick 10 times.
///   2. Build plan B with a *different* NodeId entirely (forcing tombstone + new
///      slot) — or equivalently verify that a module whose InstanceId has no
///      prior pool entry starts at count=0.
///
/// Under E009 the fresh counter gets a new pool slot and starts at count=0.
/// This behaviour is unchanged from the pre-E009 design; the test is marked
/// ignored because its API calls (`tick` signature, `slot.module` access) will
/// need updating after T-0043.
#[test]
#[ignore = "requires E009 API updates (tick signature, pool access) from T-0043/T-0045"]
fn replan_fresh_instance_starts_from_default_state() {
    let mut planner = Planner::with_capacity(POOL_CAPACITY);

    let mut plan_a = planner.build(counter_graph(StatefulCounter::new()), None).unwrap();
    plan_a.initialise(&ENV);

    let mut pool = vec![[0.0f64; 2]; POOL_CAPACITY];
    for i in 0..10usize {
        plan_a.tick(&mut pool, i % 2);
    }

    // Build plan B with a completely fresh StatefulCounter (new InstanceId, new
    // pool slot). Under E009: plan_a's counter is tombstoned; plan_b's counter
    // gets a new slot with count=0.
    // TODO(T-0045): update tick call and counter access for the module pool API.
    let plan_b = planner.build(counter_graph(StatefulCounter::new()), None).unwrap();

    let count_b = plan_b
        .slots
        .iter()
        .find_map(|s| s.module.as_any().downcast_ref::<StatefulCounter>())
        .expect("StatefulCounter not found in plan_b")
        .count;

    assert_eq!(count_b, 0, "fresh replacement instance must start from default state (count=0)");
}
