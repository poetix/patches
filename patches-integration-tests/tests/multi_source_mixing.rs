//! Integration tests: multi-source mixing via `Sum`, including stable buffer
//! index allocation and output correctness across a re-plan.
//!
//! Validates three properties:
//!
//! 1. **Correct mixing** — two source modules feeding a `Sum` module produce
//!    the expected sum on the audio output (within floating-point tolerance).
//!    This doubles as a check that `input_scales` (from T-0021) are applied
//!    correctly end-to-end.
//!
//! 2. **Slot stability** — the `Sum` module's output buffer slot is identical
//!    across successive re-plans of the same graph.
//!
//! 3. **Drop-source correctness** — re-planning to remove one source zeroes
//!    that source's output buffer slot (it appears in `to_zero`) and the
//!    remaining source's contribution is the only signal on the output.
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
use patches_engine::{build_patch, BufferAllocState, ExecutionPlan};
use patches_modules::{AudioOut, Sum};

// ── HeadlessEngine ────────────────────────────────────────────────────────────

/// Synchronous, device-free engine fixture that mirrors the real audio callback.
///
/// See `replan_integration.rs` for the full description of the lifecycle it
/// replicates.  `pool` is exposed as a public field so tests can inspect and
/// contaminate slots directly.
struct HeadlessEngine {
    plan: Option<ExecutionPlan>,
    pool: Vec<[f64; 2]>,
    /// Write index: alternates 0 / 1 on each call to `tick`.
    wi: usize,
    env: AudioEnvironment,
}

impl HeadlessEngine {
    fn new(plan: ExecutionPlan, pool_capacity: usize, env: AudioEnvironment) -> Self {
        let mut engine = Self {
            plan: None,
            pool: vec![[0.0; 2]; pool_capacity],
            wi: 0,
            env,
        };
        engine.adopt_plan(plan);
        engine
    }

    /// Adopt a new execution plan, mirroring the audio-callback plan-swap sequence:
    ///
    ///   1. Zero every slot in `plan.to_zero`.
    ///   2. Initialise all modules in the new plan.
    ///   3. Replace `self.plan` — this drops the old plan.
    fn adopt_plan(&mut self, mut plan: ExecutionPlan) {
        for &i in &plan.to_zero {
            self.pool[i] = [0.0; 2];
        }
        plan.initialise(&self.env);
        self.plan = Some(plan);
    }

    fn tick(&mut self) {
        let plan = self.plan.as_mut().expect("HeadlessEngine::tick: no current plan");
        plan.tick(&mut self.pool, self.wi);
        self.wi = 1 - self.wi;
    }

    fn last_left(&self) -> f64 {
        self.plan.as_ref().map_or(0.0, |p| p.last_left())
    }

    fn last_right(&self) -> f64 {
        self.plan.as_ref().map_or(0.0, |p| p.last_right())
    }
}

// ── ConstSource ───────────────────────────────────────────────────────────────

/// A minimal module with one output port that emits a fixed constant on every
/// `process` call.  The value is set at construction time.
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

const POOL_CAPACITY: usize = 256;
const ENV: AudioEnvironment = AudioEnvironment { sample_rate: 48_000.0 };

/// Value emitted by source A.
const SRC_A_VAL: f64 = 0.3;
/// Value emitted by source B.
const SRC_B_VAL: f64 = 0.5;

fn p(name: &'static str) -> PortRef {
    PortRef { name, index: 0 }
}

fn pi(name: &'static str, index: u32) -> PortRef {
    PortRef { name, index }
}

/// Look up the pool slot assigned to output port 0 of `node` in `alloc`.
fn slot_of(alloc: &BufferAllocState, node: &str) -> usize {
    *alloc
        .output_buf
        .get(&(NodeId::from(node), 0))
        .unwrap_or_else(|| panic!("node '{node}' not found in alloc state"))
}

/// Two-source mix graph: `src_a` and `src_b` each feed one input of `Sum(2)`;
/// the mixer output drives both channels of `AudioOut`.
///
/// Execution order (ascending `NodeId`): `mix` → `out` → `src_a` → `src_b`.
/// The 1-sample cable delay means any execution order is correct; steady state
/// is reached after a few ticks.
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

/// One-source mix graph: only `src_a` feeds `Sum(2)` input 0; input 1 is
/// unconnected and reads the permanent-zero buffer.
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

/// Two sources of known constant values feed a `Sum` module; the audio output
/// must equal their sum on both channels after steady state.
///
/// With `input_scales = 1.0` on every edge, the values pass unscaled: the
/// output must be exactly `SRC_A_VAL + SRC_B_VAL` in f64 arithmetic.
#[test]
fn two_sources_mixed_output_equals_sum() {
    let alloc_0 = BufferAllocState::default();
    let (plan, _) = build_patch(two_source_mix_graph(), None, &alloc_0, POOL_CAPACITY)
        .expect("build mix plan");

    let mut engine = HeadlessEngine::new(plan, POOL_CAPACITY, ENV);

    // Four ticks fill both double-buffer halves and propagate through the two
    // hops (sources → mix → AudioOut) with the 1-sample cable delay.
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

/// The `Sum` module's output buffer slot must be identical across successive
/// re-plans of the same graph.
///
/// The `(NodeId("mix"), output_port_index=0)` key is present in both builds,
/// so `build_patch` reuses the existing pool slot without adding it to
/// `to_zero`.
#[test]
fn mix_output_slot_stable_across_replan() {
    let alloc_0 = BufferAllocState::default();
    let (_, alloc_a) = build_patch(two_source_mix_graph(), None, &alloc_0, POOL_CAPACITY)
        .expect("build plan A");
    let slot_a = slot_of(&alloc_a, "mix");

    let (_, alloc_b) = build_patch(two_source_mix_graph(), None, &alloc_a, POOL_CAPACITY)
        .expect("build plan B");
    let slot_b = slot_of(&alloc_b, "mix");

    assert_eq!(
        slot_a, slot_b,
        "Sum output buffer slot must be reused unchanged across a re-plan"
    );
}

/// Dropping one source from the mix graph must:
///
///   1. List the dropped source's output slot in `plan_b.to_zero`.
///   2. Produce only the remaining source's value on the audio output after
///      steady state (`src_a` only → `SRC_A_VAL`).
///
/// When `src_b` is removed, `Sum(2)` input/1 is unconnected and reads the
/// permanent-zero buffer, so the output equals `src_a`'s value alone.
#[test]
fn dropped_source_slot_in_to_zero_and_output_correct() {
    let alloc_0 = BufferAllocState::default();
    let (plan_a, alloc_a) = build_patch(two_source_mix_graph(), None, &alloc_0, POOL_CAPACITY)
        .expect("build plan A");
    let src_b_slot = slot_of(&alloc_a, "src_b");

    let mut engine = HeadlessEngine::new(plan_a, POOL_CAPACITY, ENV);
    for _ in 0..4 {
        engine.tick();
    }

    // Build plan B without src_b; its output slot is freed and must appear in to_zero.
    let (plan_b, _) = build_patch(one_source_mix_graph(), None, &alloc_a, POOL_CAPACITY)
        .expect("build plan B (one source)");

    assert!(
        plan_b.to_zero.contains(&src_b_slot),
        "freed slot {src_b_slot} for src_b must appear in plan_b.to_zero (got {:?})",
        plan_b.to_zero
    );

    engine.adopt_plan(plan_b);

    // Tick to steady state: only src_a feeds the mixer; src_b slot is zeroed.
    for _ in 0..4 {
        engine.tick();
    }

    let left = engine.last_left();
    assert!(
        (left - SRC_A_VAL).abs() < f64::EPSILON,
        "after dropping src_b, left channel must equal SRC_A_VAL ({SRC_A_VAL}), got {left}"
    );
}
