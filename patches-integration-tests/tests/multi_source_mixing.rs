//! Integration tests: multi-source mixing via `Sum`, including stable buffer
//! index allocation and output correctness across a re-plan.

use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleGraph, NodeId, PortDescriptor,
    PortRef,
};
use patches_engine::{build_patch, BufferAllocState, ExecutionPlan, ModuleAllocState};
use patches_modules::{AudioOut, Sum};

// ── HeadlessEngine ────────────────────────────────────────────────────────────

struct HeadlessEngine {
    plan: Option<ExecutionPlan>,
    pub pool: Vec<[f64; 2]>,
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
            pool: vec![[0.0; 2]; buffer_pool_capacity],
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
            self.pool[i] = [0.0; 2];
        }
        self.plan = Some(plan);
    }

    fn tick(&mut self) {
        let plan = self.plan.as_mut().expect("HeadlessEngine::tick: no current plan");
        plan.tick(&mut self.module_pool, &mut self.pool, self.wi);
        self.wi = 1 - self.wi;
    }

    fn last_left(&self) -> f64 {
        self.plan.as_ref().map_or(0.0, |p| p.last_left(&self.module_pool))
    }

    fn last_right(&self) -> f64 {
        self.plan.as_ref().map_or(0.0, |p| p.last_right(&self.module_pool))
    }
}

// ── ConstSource ───────────────────────────────────────────────────────────────

struct ConstSource {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    value: f64,
}

impl ConstSource {
    fn new(value: f64) -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
            value,
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
        outputs[0] = self.value;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

const BUFFER_POOL_CAPACITY: usize = 256;
const MODULE_POOL_CAPACITY: usize = 64;
const ENV: AudioEnvironment = AudioEnvironment { sample_rate: 48_000.0 };

const SRC_A_VAL: f64 = 0.3;
const SRC_B_VAL: f64 = 0.5;

fn p(name: &'static str) -> PortRef {
    PortRef { name, index: 0 }
}

fn pi(name: &'static str, index: u32) -> PortRef {
    PortRef { name, index }
}

fn slot_of(alloc: &BufferAllocState, node: &str) -> usize {
    *alloc
        .output_buf
        .get(&(NodeId::from(node), 0))
        .unwrap_or_else(|| panic!("node '{node}' not found in alloc state"))
}

fn two_source_mix_graph() -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let src_a = NodeId::from("src_a");
    let src_b = NodeId::from("src_b");
    let mix = NodeId::from("mix");
    let out = NodeId::from("out");
    graph.add_module(src_a.clone(), Box::new(ConstSource::new(SRC_A_VAL))).unwrap();
    graph.add_module(src_b.clone(), Box::new(ConstSource::new(SRC_B_VAL))).unwrap();
    graph.add_module(mix.clone(), Box::new(Sum::new(2))).unwrap();
    graph.add_module(out.clone(), Box::new(AudioOut::new())).unwrap();
    graph.connect(&src_a, p("out"), &mix, pi("in", 0), 1.0).unwrap();
    graph.connect(&src_b, p("out"), &mix, pi("in", 1), 1.0).unwrap();
    graph.connect(&mix, p("out"), &out, p("left"), 1.0).unwrap();
    graph.connect(&mix, p("out"), &out, p("right"), 1.0).unwrap();
    graph
}

fn one_source_mix_graph() -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let src_a = NodeId::from("src_a");
    let mix = NodeId::from("mix");
    let out = NodeId::from("out");
    graph.add_module(src_a.clone(), Box::new(ConstSource::new(SRC_A_VAL))).unwrap();
    graph.add_module(mix.clone(), Box::new(Sum::new(2))).unwrap();
    graph.add_module(out.clone(), Box::new(AudioOut::new())).unwrap();
    graph.connect(&src_a, p("out"), &mix, pi("in", 0), 1.0).unwrap();
    graph.connect(&mix, p("out"), &out, p("left"), 1.0).unwrap();
    graph.connect(&mix, p("out"), &out, p("right"), 1.0).unwrap();
    graph
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn two_sources_mixed_output_equals_sum() {
    let alloc_0 = BufferAllocState::default();
    let module_alloc_0 = ModuleAllocState::default();
    let (plan, _, _) =
        build_patch(two_source_mix_graph(), &alloc_0, &module_alloc_0, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build mix plan");

    let mut engine = HeadlessEngine::new(plan, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY, ENV);

    for _ in 0..4 {
        engine.tick();
    }

    let expected = SRC_A_VAL + SRC_B_VAL;
    let left = engine.last_left();
    let right = engine.last_right();

    assert!(
        (left - expected).abs() < f64::EPSILON,
        "left channel: expected {expected}, got {left}"
    );
    assert!(
        (right - expected).abs() < f64::EPSILON,
        "right channel: expected {expected}, got {right}"
    );
}

#[test]
fn mix_output_slot_stable_across_replan() {
    let alloc_0 = BufferAllocState::default();
    let module_alloc_0 = ModuleAllocState::default();
    let (_, alloc_a, module_alloc_a) =
        build_patch(two_source_mix_graph(), &alloc_0, &module_alloc_0, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan A");
    let slot_a = slot_of(&alloc_a, "mix");

    let (_, alloc_b, _) =
        build_patch(two_source_mix_graph(), &alloc_a, &module_alloc_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan B");
    let slot_b = slot_of(&alloc_b, "mix");

    assert_eq!(slot_a, slot_b, "Sum output buffer slot must be reused unchanged across a re-plan");
}

#[test]
fn dropped_source_slot_in_to_zero_and_output_correct() {
    let alloc_0 = BufferAllocState::default();
    let module_alloc_0 = ModuleAllocState::default();
    let (plan_a, alloc_a, module_alloc_a) =
        build_patch(two_source_mix_graph(), &alloc_0, &module_alloc_0, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan A");
    let src_b_slot = slot_of(&alloc_a, "src_b");

    let mut engine = HeadlessEngine::new(plan_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY, ENV);
    for _ in 0..4 {
        engine.tick();
    }

    let (plan_b, _, _) =
        build_patch(one_source_mix_graph(), &alloc_a, &module_alloc_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY)
            .expect("build plan B (one source)");

    assert!(
        plan_b.to_zero.contains(&src_b_slot),
        "freed slot {src_b_slot} for src_b must appear in plan_b.to_zero (got {:?})",
        plan_b.to_zero
    );

    engine.adopt_plan(plan_b);

    for _ in 0..4 {
        engine.tick();
    }

    let left = engine.last_left();
    assert!(
        (left - SRC_A_VAL).abs() < f64::EPSILON,
        "after dropping src_b, left channel must equal SRC_A_VAL ({SRC_A_VAL}), got {left}"
    );
}
