//! Integration tests: stable buffer-index allocation across successive re-plans.
//!
//! Validates four properties of the buffer-allocation machinery:
//!
//! 1. **Slot stability** — a cable surviving a re-plan uses the same pool slot
//!    before and after.
//!
//! 2. **Signal continuity** — because the stable cable's slot is absent from
//!    `to_zero`, the audio thread does not zero it on plan adoption.
//!
//! 3. **New cable zeroed** — a cable added in a re-plan receives a fresh slot
//!    that appears in `to_zero` and is zeroed on adoption.
//!
//! 4. **Removed cable zeroed** — a cable removed in a re-plan has its former
//!    slot freed; it appears in `to_zero` and is zeroed on plan acceptance.
//!
//! ## Why `build_patch` is used directly
//!
//! The acceptance criteria require inspecting `BufferAllocState::output_buf`
//! after each plan build.  Calling `build_patch` directly lets the tests
//! compare allocation states from successive builds without going through the
//! `Planner` abstraction.

use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleGraph, NodeId, PortDescriptor,
    PortRef,
};
use patches_engine::{build_patch, BufferAllocState, ExecutionPlan, ModuleAllocState, ModulePool};
use patches_modules::AudioOut;

// ── HeadlessEngine ────────────────────────────────────────────────────────────

struct HeadlessEngine {
    plan: ExecutionPlan,
    buffer_pool: Box<[[f64; 2]]>,
    module_pool: ModulePool,
    wi: usize,
    env: AudioEnvironment,
}

impl HeadlessEngine {
    fn new(
        mut plan: ExecutionPlan,
        buffer_pool_capacity: usize,
        module_pool_capacity: usize,
        env: AudioEnvironment,
    ) -> Self {
        let mut buffer_pool = vec![[0.0_f64; 2]; buffer_pool_capacity].into_boxed_slice();
        let mut module_pool = ModulePool::new(module_pool_capacity);
        for &idx in &plan.tombstones {
            module_pool.tombstone(idx);
        }
        for (idx, mut m) in plan.new_modules.drain(..) {
            m.initialise(&env);
            module_pool.install(idx, m);
        }
        for &i in &plan.to_zero {
            buffer_pool[i] = [0.0; 2];
        }
        Self { plan, buffer_pool, module_pool, wi: 0, env }
    }

    fn adopt_plan(&mut self, mut plan: ExecutionPlan) {
        for &idx in &plan.tombstones {
            self.module_pool.tombstone(idx);
        }
        for (idx, mut m) in plan.new_modules.drain(..) {
            m.initialise(&self.env);
            self.module_pool.install(idx, m);
        }
        for &i in &plan.to_zero {
            self.buffer_pool[i] = [0.0; 2];
        }
        self.plan = plan;
    }

    fn tick(&mut self) {
        self.plan.tick(&mut self.module_pool, &mut self.buffer_pool, self.wi);
        self.wi = 1 - self.wi;
    }

    fn last_left(&self) -> f64 {
        self.module_pool.read_sink_left()
    }
}

// ── ConstSource ───────────────────────────────────────────────────────────────

struct ConstSource {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
}

impl ConstSource {
    fn new() -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
        }
    }
}

impl Module for ConstSource {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, _inputs: &[f64], outputs: &mut [f64]) {
        outputs[0] = 1.0;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

const BUFFER_POOL_CAPACITY: usize = 256;
const MODULE_POOL_CAPACITY: usize = 64;
const ENV: AudioEnvironment = AudioEnvironment { sample_rate: 48_000.0 };

fn p(name: &'static str) -> PortRef {
    PortRef { name, index: 0 }
}

fn one_source_graph() -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let src_id = NodeId::from("src");
    let out_id = NodeId::from("out");
    graph.add_module(src_id.clone(), Box::new(ConstSource::new())).unwrap();
    graph.add_module(out_id.clone(), Box::new(AudioOut::new())).unwrap();
    graph.connect(&src_id, p("out"), &out_id, p("left"), 1.0).unwrap();
    graph.connect(&src_id, p("out"), &out_id, p("right"), 1.0).unwrap();
    graph
}

fn two_source_graph() -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let src_id = NodeId::from("src");
    let src2_id = NodeId::from("src2");
    let out_id = NodeId::from("out");
    graph.add_module(src_id.clone(), Box::new(ConstSource::new())).unwrap();
    graph.add_module(src2_id.clone(), Box::new(ConstSource::new())).unwrap();
    graph.add_module(out_id.clone(), Box::new(AudioOut::new())).unwrap();
    graph.connect(&src_id, p("out"), &out_id, p("left"), 1.0).unwrap();
    graph.connect(&src2_id, p("out"), &out_id, p("right"), 1.0).unwrap();
    graph
}

fn slot_of(alloc: &BufferAllocState, node: &str) -> usize {
    *alloc
        .output_buf
        .get(&(NodeId::from(node), 0))
        .unwrap_or_else(|| panic!("node '{node}' not found in alloc state"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn stable_slot_survives_replan() {
    let alloc_0 = BufferAllocState::default();
    let module_alloc_0 = ModuleAllocState::default();

    let (_, alloc_a, module_alloc_a) =
        build_patch(one_source_graph(), &alloc_0, &module_alloc_0, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan A");
    let slot_a = slot_of(&alloc_a, "src");

    let (_, alloc_b, _) =
        build_patch(one_source_graph(), &alloc_a, &module_alloc_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan B");
    let slot_b = slot_of(&alloc_b, "src");

    assert_eq!(slot_a, slot_b, "surviving cable must reuse the same pool slot across a re-plan");
}

#[test]
fn signal_continuous_across_replan() {
    let alloc_0 = BufferAllocState::default();
    let module_alloc_0 = ModuleAllocState::default();

    let (plan_a, alloc_a, module_alloc_a) =
        build_patch(one_source_graph(), &alloc_0, &module_alloc_0, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan A");
    let src_slot = slot_of(&alloc_a, "src");

    let mut engine = HeadlessEngine::new(plan_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY, ENV);

    for _ in 0..4 {
        engine.tick();
    }
    assert_eq!(engine.last_left(), 1.0, "left channel must be 1.0 before re-plan");

    let (plan_b, _alloc_b, _) =
        build_patch(one_source_graph(), &alloc_a, &module_alloc_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan B");
    assert!(
        !plan_b.to_zero.contains(&src_slot),
        "stable slot {src_slot} must not appear in to_zero (got {:?})",
        plan_b.to_zero
    );

    engine.adopt_plan(plan_b);
    engine.tick();

    assert_eq!(
        engine.last_left(),
        1.0,
        "left channel must remain 1.0 immediately after re-plan (no discontinuity)"
    );
}

#[test]
fn new_cable_slot_starts_from_zero() {
    let alloc_0 = BufferAllocState::default();
    let module_alloc_0 = ModuleAllocState::default();

    let (plan_a, alloc_a, module_alloc_a) =
        build_patch(one_source_graph(), &alloc_0, &module_alloc_0, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan A");

    let mut engine = HeadlessEngine::new(plan_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY, ENV);
    for _ in 0..4 {
        engine.tick();
    }

    let (plan_b, alloc_b, _) =
        build_patch(two_source_graph(), &alloc_a, &module_alloc_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan B");
    let src2_slot = slot_of(&alloc_b, "src2");

    assert!(
        plan_b.to_zero.contains(&src2_slot),
        "new slot {src2_slot} must appear in plan_b.to_zero (got {:?})",
        plan_b.to_zero
    );

    engine.buffer_pool[src2_slot] = [99.0; 2];

    engine.adopt_plan(plan_b);

    assert_eq!(
        engine.buffer_pool[src2_slot],
        [0.0; 2],
        "new cable slot {src2_slot} must be zeroed on plan adoption"
    );
}

#[test]
fn removed_cable_slot_zeroed_on_adoption() {
    let alloc_0 = BufferAllocState::default();
    let module_alloc_0 = ModuleAllocState::default();

    let (plan_a, alloc_a, module_alloc_a) =
        build_patch(two_source_graph(), &alloc_0, &module_alloc_0, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan A");
    let src2_slot = slot_of(&alloc_a, "src2");

    let mut engine = HeadlessEngine::new(plan_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY, ENV);

    for _ in 0..4 {
        engine.tick();
    }

    let dirty = engine.buffer_pool[src2_slot];
    assert!(
        dirty != [0.0; 2],
        "src2's pool slot must be non-zero after ticking (got {dirty:?})"
    );

    let (plan_b, _alloc_b, _) =
        build_patch(one_source_graph(), &alloc_a, &module_alloc_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan B");

    assert!(
        plan_b.to_zero.contains(&src2_slot),
        "freed slot {src2_slot} must appear in plan_b.to_zero (got {:?})",
        plan_b.to_zero
    );

    assert!(
        engine.buffer_pool[src2_slot] != [0.0; 2],
        "pool must still be dirty before adopt_plan is called"
    );

    engine.adopt_plan(plan_b);

    assert_eq!(
        engine.buffer_pool[src2_slot],
        [0.0; 2],
        "freed slot {src2_slot} must be zeroed when plan B is adopted"
    );
}
