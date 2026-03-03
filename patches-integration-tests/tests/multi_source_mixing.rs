//! Integration tests: multi-source mixing via `Sum`, including stable buffer
//! index allocation and output correctness across a re-plan.

use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleGraph, NodeId, PortDescriptor,
    PortRef,
};
use patches_engine::{build_patch, BufferAllocState, ExecutionPlan, ModuleAllocState, ModulePool};
use patches_modules::{AudioOut, Sum};

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

    fn last_right(&self) -> f64 {
        self.module_pool.read_sink_right()
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
