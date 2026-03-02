//! Integration tests: stable buffer-index allocation across successive re-plans.
//!
//! Validates four properties of the buffer-allocation machinery:
//!
//! 1. **Slot stability** — a cable surviving a re-plan uses the same pool slot
//!    before and after; `BufferAllocState::output_buf` returns the same index
//!    for the same `(NodeId, port)` key.
//!
//! 2. **Signal continuity** — because the stable cable's slot is absent from
//!    `to_zero`, the audio thread does not zero it on plan adoption, so the
//!    output signal shows no discontinuity immediately after the swap.
//!
//! 3. **New cable zeroed** — a cable added in a re-plan receives a fresh slot
//!    that appears in `to_zero` and is zeroed by the engine before the first
//!    tick with the new plan.
//!
//! 4. **Removed cable zeroed** — a cable removed in a re-plan has its former
//!    slot freed; it appears in `to_zero` and is zeroed on plan acceptance.
//!
//! ## Why `build_patch` is used directly
//!
//! The acceptance criteria require inspecting `BufferAllocState::output_buf`
//! after each plan build.  `Planner::build` wraps `build_patch` and carries the
//! state internally.  Calling `build_patch` directly lets the tests compare
//! allocation states from successive builds without going through the planner
//! abstraction.

use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleGraph, NodeId, PortDescriptor,
    PortRef,
};
use patches_engine::{build_patch, BufferAllocState, ExecutionPlan};
use patches_modules::AudioOut;

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
}

// ── ConstSource ───────────────────────────────────────────────────────────────

/// A minimal module with one output port that emits a constant `1.0` on every
/// `process` call.
///
/// Produces a predictably non-zero, stable signal so the pool slot is
/// verifiably non-zero after ticking and the output can be checked for
/// continuity after a re-plan.
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

const POOL_CAPACITY: usize = 256;
const ENV: AudioEnvironment = AudioEnvironment { sample_rate: 48_000.0 };

fn p(name: &'static str) -> PortRef {
    PortRef { name, index: 0 }
}

/// One-source graph: `src` drives both channels of `out`.
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

/// Two-source graph: `src` on left, `src2` on right → `out`.
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

/// Look up the pool slot assigned to output port 0 of `node` in `alloc`.
fn slot_of(alloc: &BufferAllocState, node: &str) -> usize {
    *alloc
        .output_buf
        .get(&(NodeId::from(node), 0))
        .unwrap_or_else(|| panic!("node '{node}' not found in alloc state"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// A cable that survives a re-plan must use the same pool slot before and after.
///
/// Timeline:
///   1. Build plan A for `one_source_graph`; record the slot assigned to `src`.
///   2. Build plan B for the same graph, threading the alloc state from step 1.
///   3. Assert both builds produced the same slot index for `src`.
#[test]
fn stable_slot_survives_replan() {
    let alloc_0 = BufferAllocState::default();
    let (_, alloc_a) =
        build_patch(one_source_graph(), None, &alloc_0, POOL_CAPACITY).expect("build plan A");
    let slot_a = slot_of(&alloc_a, "src");

    let (_, alloc_b) =
        build_patch(one_source_graph(), None, &alloc_a, POOL_CAPACITY).expect("build plan B");
    let slot_b = slot_of(&alloc_b, "src");

    assert_eq!(
        slot_a, slot_b,
        "surviving cable must reuse the same pool slot across a re-plan"
    );
}

/// The output signal must be continuous across a re-plan: the stable cable's
/// slot is absent from `to_zero`, so its value is preserved when the new plan
/// is adopted and the first tick after the swap produces the same result.
///
/// `ConstSource` always emits `1.0`.  After enough ticks to fill both
/// double-buffer halves the left channel reads `1.0`.  After re-planning to
/// an identical graph (same `NodeId` keys → same pool slot), the left channel
/// must still read `1.0` on the very next tick.
#[test]
fn signal_continuous_across_replan() {
    let alloc_0 = BufferAllocState::default();
    let (plan_a, alloc_a) =
        build_patch(one_source_graph(), None, &alloc_0, POOL_CAPACITY).expect("build plan A");
    let src_slot = slot_of(&alloc_a, "src");

    let mut engine = HeadlessEngine::new(plan_a, POOL_CAPACITY, ENV);

    // Four ticks fills both double-buffer halves (two per half) with the
    // stable 1.0 value from ConstSource.
    for _ in 0..4 {
        engine.tick();
    }
    assert_eq!(engine.last_left(), 1.0, "left channel must be 1.0 before re-plan");

    // Build plan B for the same graph; the stable slot must NOT appear in to_zero.
    let (plan_b, _alloc_b) =
        build_patch(one_source_graph(), None, &alloc_a, POOL_CAPACITY).expect("build plan B");
    assert!(
        !plan_b.to_zero.contains(&src_slot),
        "stable slot {src_slot} must not appear in to_zero (got {:?})",
        plan_b.to_zero
    );

    engine.adopt_plan(plan_b);

    // First tick after plan swap: AudioOut reads the previous sample from the
    // stable slot, which still holds 1.0 (not zeroed).
    engine.tick();

    assert_eq!(
        engine.last_left(),
        1.0,
        "left channel must remain 1.0 immediately after re-plan (no discontinuity)"
    );
}

/// A cable added in a re-plan must receive a newly allocated slot that is
/// listed in `to_zero` and zeroed by the engine on adoption.
///
/// Timeline:
///   1. Build plan A for `one_source_graph` (src only); tick N samples.
///   2. Build plan B for `two_source_graph` (src + src2).  `src2` is new; its
///      slot must appear in `plan_b.to_zero`.
///   3. Contaminate the pool slot to rule out incidental zero.
///   4. Adopt plan B; verify the slot is `[0.0; 2]` afterwards.
#[test]
fn new_cable_slot_starts_from_zero() {
    let alloc_0 = BufferAllocState::default();
    let (plan_a, alloc_a) =
        build_patch(one_source_graph(), None, &alloc_0, POOL_CAPACITY).expect("build plan A");

    let mut engine = HeadlessEngine::new(plan_a, POOL_CAPACITY, ENV);
    for _ in 0..4 {
        engine.tick();
    }

    let (plan_b, alloc_b) =
        build_patch(two_source_graph(), None, &alloc_a, POOL_CAPACITY).expect("build plan B");
    let src2_slot = slot_of(&alloc_b, "src2");

    assert!(
        plan_b.to_zero.contains(&src2_slot),
        "new slot {src2_slot} must appear in plan_b.to_zero (got {:?})",
        plan_b.to_zero
    );

    // Contaminate the slot to prove zeroing is performed by adopt_plan,
    // not by the initial pool allocation.
    engine.pool[src2_slot] = [99.0; 2];

    engine.adopt_plan(plan_b);

    assert_eq!(
        engine.pool[src2_slot],
        [0.0; 2],
        "new cable slot {src2_slot} must be zeroed on plan adoption"
    );
}

/// A cable removed in a re-plan must have its former slot freed: it must appear
/// in `to_zero` and be zeroed by the engine on plan acceptance.
///
/// Timeline:
///   1. Build plan A for `two_source_graph` (src + src2); tick N samples so
///      both slots carry non-zero values.
///   2. Build plan B for `one_source_graph` (src only).  `src2`'s slot is freed
///      and must appear in `plan_b.to_zero`.
///   3. Confirm the slot is still dirty before adoption.
///   4. Adopt plan B; verify the slot is `[0.0; 2]` afterwards.
#[test]
fn removed_cable_slot_zeroed_on_adoption() {
    let alloc_0 = BufferAllocState::default();
    let (plan_a, alloc_a) =
        build_patch(two_source_graph(), None, &alloc_0, POOL_CAPACITY).expect("build plan A");
    let src2_slot = slot_of(&alloc_a, "src2");

    let mut engine = HeadlessEngine::new(plan_a, POOL_CAPACITY, ENV);

    // Four ticks write both double-buffer halves of src2's slot.
    for _ in 0..4 {
        engine.tick();
    }

    let dirty = engine.pool[src2_slot];
    assert!(
        dirty != [0.0; 2],
        "src2's pool slot must be non-zero after ticking (got {dirty:?})"
    );

    // Build plan B (src only); src2's slot is freed and must appear in to_zero.
    let (plan_b, _alloc_b) =
        build_patch(one_source_graph(), None, &alloc_a, POOL_CAPACITY).expect("build plan B");

    assert!(
        plan_b.to_zero.contains(&src2_slot),
        "freed slot {src2_slot} must appear in plan_b.to_zero (got {:?})",
        plan_b.to_zero
    );

    // Slot still dirty before adoption.
    assert!(
        engine.pool[src2_slot] != [0.0; 2],
        "pool must still be dirty before adopt_plan is called"
    );

    engine.adopt_plan(plan_b);

    assert_eq!(
        engine.pool[src2_slot],
        [0.0; 2],
        "freed slot {src2_slot} must be zeroed when plan B is adopted"
    );
}
