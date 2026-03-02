//! Integration tests: hot-reload replanning with graph modification.
//!
//! Validates three properties of the replanning lifecycle:
//!
//! 1. **Drop** — modules removed from the graph are dropped when the engine
//!    *adopts* the new plan (i.e. when the old plan is replaced), not during
//!    `Planner::build`.  In the real engine the old plan lives on the audio
//!    thread and the control thread cannot reach it; `Planner::build` is
//!    called with `prev_plan = None`.  DropSpy must therefore remain alive
//!    until `adopt_plan` discards the old `ExecutionPlan`.
//!
//! 2. **Freed-slot zeroing** — buffer pool indices freed when a module is
//!    removed appear in `ExecutionPlan::to_zero` and are zeroed by the engine
//!    before the first `tick` with the new plan.
//!
//! 3. **New-slot zeroing** — buffer pool indices allocated for a newly added
//!    module also appear in `to_zero` and are zeroed on adoption.
//!
//! ## Real engine flow (what the tests replicate)
//!
//! ```text
//! control thread                   audio thread
//! ──────────────────────────────   ──────────────────────────────
//! plan_a = Planner::build(g_a, None)
//! engine.adopt_plan(plan_a)   ──►  current_plan = plan_a
//!                                  loop { tick() }
//! plan_b = Planner::build(g_b, None)   ← old plan never leaves audio thread
//! engine.adopt_plan(plan_b)   ──►  zero to_zero slots
//!                                  drop(plan_a)   ← DropSpy freed HERE
//!                                  current_plan = plan_b
//!                                  loop { tick() }
//! ```
//!
//! ## Test fixture
//!
//! [`HeadlessEngine`] mirrors this sequence synchronously, without opening
//! any audio hardware, making the buffer pool inspectable between steps.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleGraph, NodeId, PortDescriptor,
};
use patches_engine::{ExecutionPlan, Planner};
use patches_modules::{AudioOut, SineOscillator};

// ── HeadlessEngine ────────────────────────────────────────────────────────────

/// Synchronous, device-free engine fixture that mirrors the real audio callback.
///
/// The CPAL callback does, in order:
///   1. Pop a new plan from the lock-free channel (if available).
///   2. Zero every index in `new_plan.to_zero`.
///   3. Set `current_plan = new_plan`, which **drops the old plan**.
///   4. Tick all modules for each sample in the buffer.
///
/// `adopt_plan` replicates steps 2–3; `tick` replicates step 4.
///
/// There is intentionally no method to extract the active plan from the engine.
/// In the real system the running plan lives on the audio thread and is never
/// accessible to the control thread.
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
    ///   1. Zero every slot in `plan.to_zero` (freed and newly allocated indices).
    ///   2. Initialise all modules in the new plan.
    ///   3. Replace `self.plan` — **this drops the old plan**, and with it any
    ///      module instances that are no longer in the new graph.
    ///
    /// Drop of the old plan occurs at step 3, after zeroing but before the first
    /// `tick` with the new plan.
    fn adopt_plan(&mut self, mut plan: ExecutionPlan) {
        for &i in &plan.to_zero {
            self.pool[i] = [0.0; 2];
        }
        plan.initialise(&self.env);
        self.plan = Some(plan); // ← old plan (and its modules) dropped here
    }

    /// Process one sample, alternating `wi` each call.
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

    /// Read both double-buffer halves for a given pool slot index.
    fn pool_slot(&self, idx: usize) -> [f64; 2] {
        self.pool[idx]
    }
}

// ── DropSpy ──────────────────────────────────────────────────────────────────

/// A module that outputs a constant `1.0` and sets an `Arc<AtomicBool>` flag
/// when it is dropped.
///
/// The flag lets tests observe the exact moment a module instance is freed,
/// independent of any other side-channel.
struct DropSpy {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    dropped: Arc<AtomicBool>,
}

impl DropSpy {
    fn new(flag: Arc<AtomicBool>) -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out" }],
            },
            dropped: flag,
        }
    }
}

impl Drop for DropSpy {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl Module for DropSpy {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, _inputs: &[f64], outputs: &mut [f64]) {
        // Constant output so the pool slot is predictably non-zero after ticking.
        outputs[0] = 1.0;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ── Graph builders ────────────────────────────────────────────────────────────

const POOL_CAPACITY: usize = 256;
const ENV: AudioEnvironment = AudioEnvironment { sample_rate: 48_000.0 };

/// Two sources → stereo out: `DropSpy` on left, `SineOscillator` on right.
///
/// NodeId sort order: `"out"` < `"sine"` < `"spy"` (ascending, per builder
/// convention).  The 1-sample cable delay makes any execution order valid.
///
/// Buffer allocation (after `"out"` with 0 outputs, in execution order):
///   - `("sine", 0)` → pool slot 1
///   - `("spy",  0)` → pool slot 2
fn two_source_graph(spy: DropSpy) -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let spy_id = NodeId::from("spy");
    let sine_id = NodeId::from("sine");
    let out_id = NodeId::from("out");
    graph.add_module(spy_id.clone(), Box::new(spy)).unwrap();
    graph.add_module(sine_id.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
    graph.add_module(out_id.clone(), Box::new(AudioOut::new())).unwrap();
    graph.connect(&spy_id, "out", &out_id, "left", 1.0).unwrap();
    graph.connect(&sine_id, "out", &out_id, "right", 1.0).unwrap();
    graph
}

/// Single source → stereo out: `SineOscillator` on both channels.
///
/// Buffer allocation:
///   - `("sine", 0)` → pool slot 1  (stable when transitioning from/to
///     `two_source_graph`, where `"sine"` uses the same slot)
fn one_source_graph() -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let sine_id = NodeId::from("sine");
    let out_id = NodeId::from("out");
    graph.add_module(sine_id.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
    graph.add_module(out_id.clone(), Box::new(AudioOut::new())).unwrap();
    graph.connect(&sine_id, "out", &out_id, "left", 1.0).unwrap();
    graph.connect(&sine_id, "out", &out_id, "right", 1.0).unwrap();
    graph
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Modules removed from the graph must be dropped when the engine adopts the
/// new plan, not earlier.
///
/// In the real engine the old plan is owned by the audio callback and the
/// control thread never touches it.  `Planner::build` is therefore called with
/// `prev_plan = None`; plan A (with DropSpy) stays in the engine until
/// `adopt_plan` replaces `current_plan` with plan B.
///
/// Checkpoints:
///   - After `Planner::build(graph_b, None)` returns: DropSpy is **alive**
///     (the build did not touch the old plan).
///   - After `engine.adopt_plan(plan_b)` returns: DropSpy is **dead**
///     (the old plan was dropped inside `adopt_plan`).
#[test]
fn replan_drops_removed_module() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut planner = Planner::with_capacity(POOL_CAPACITY);
    let plan_a = planner.build(two_source_graph(DropSpy::new(flag.clone())), None).unwrap();
    let mut engine = HeadlessEngine::new(plan_a, POOL_CAPACITY, ENV);

    for _ in 0..10 {
        engine.tick();
    }

    assert!(!flag.load(Ordering::SeqCst), "spy must not be dropped while plan A is active");

    // Build plan B on the control thread — plan A remains in the engine.
    // The Planner's internal alloc_state handles buffer stability without
    // needing a reference to the running plan.
    let plan_b = planner.build(one_source_graph(), None).unwrap();

    assert!(
        !flag.load(Ordering::SeqCst),
        "spy must still be alive after Planner::build — the old plan has not been replaced yet"
    );

    // Adopt plan B: self.plan = Some(plan_b) drops the old Some(plan_a),
    // which drops every ModuleSlot in plan_a, which drops DropSpy.
    engine.adopt_plan(plan_b);

    assert!(
        flag.load(Ordering::SeqCst),
        "spy must be dropped when the engine adopts the new plan"
    );

    for _ in 0..100 {
        engine.tick();
    }
    assert!(engine.last_right().abs() > 0.0, "sine must produce non-zero output after replan");
}

/// Buffer pool slots freed when a module is removed must appear in
/// `ExecutionPlan::to_zero` and be zeroed by `HeadlessEngine::adopt_plan`.
///
/// The `Planner`'s internal `alloc_state` tracks which slots are in use
/// across builds regardless of whether the old plan is passed as `prev_plan`,
/// so this test mirrors the real engine flow (no `take_plan`).
///
/// Timeline:
///   1. Plan A (`spy` + `sine`) runs for several ticks → spy's pool slot is
///      non-zero (DropSpy writes `1.0` every sample).
///   2. Plan B (`sine` only) is built with `prev_plan = None` — spy's slot
///      is freed by `Planner` and listed in `plan_b.to_zero`.
///   3. Pool is inspected *before* adoption: slot is still dirty (the old
///      plan is still active in the engine).
///   4. `adopt_plan(plan_b)` is called: slot must be `[0.0; 2]` afterwards.
#[test]
fn replan_zeroes_freed_buffer_slot() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut planner = Planner::with_capacity(POOL_CAPACITY);
    let plan_a = planner.build(two_source_graph(DropSpy::new(flag.clone())), None).unwrap();

    // Record spy's output buffer index before moving plan_a into the engine.
    let spy_buf = plan_a
        .slots
        .iter()
        .find(|s| s.module.as_any().downcast_ref::<DropSpy>().is_some())
        .expect("DropSpy slot not found in plan A")
        .output_buffers[0];

    let mut engine = HeadlessEngine::new(plan_a, POOL_CAPACITY, ENV);

    // Two ticks write both double-buffer halves; use four for clarity.
    for _ in 0..4 {
        engine.tick();
    }

    // DropSpy always outputs 1.0; both halves of the double buffer must be
    // non-zero at this point.
    let dirty = engine.pool_slot(spy_buf);
    assert!(
        dirty != [0.0; 2],
        "spy's pool slot must be non-zero after ticking (got {dirty:?})"
    );

    // Build plan B on the control thread, mirroring the real engine flow.
    // Plan A is still running in the engine; its spy slot is freed in the
    // Planner's alloc_state and appears in plan_b.to_zero.
    let plan_b = planner.build(one_source_graph(), None).unwrap();

    assert!(
        plan_b.to_zero.contains(&spy_buf),
        "freed slot {spy_buf} must appear in plan_b.to_zero (got {:?})",
        plan_b.to_zero
    );

    // Plan A is still active: the pool slot is still dirty.
    assert!(
        engine.pool_slot(spy_buf) != [0.0; 2],
        "pool must still be dirty before adopt_plan is called"
    );

    // adopt_plan zeros the freed slot, then drops plan A (and DropSpy).
    engine.adopt_plan(plan_b);

    assert_eq!(
        engine.pool_slot(spy_buf),
        [0.0; 2],
        "freed slot {spy_buf} must be zeroed when plan B is adopted"
    );
}

/// Buffer pool slots allocated for a *newly added* module must also appear in
/// `to_zero` and be zeroed on adoption.
///
/// To make zeroing observable the test pre-contaminates the target pool slot
/// with a sentinel value before calling `adopt_plan`, ruling out the
/// possibility that the slot appears zero only because it was never written.
#[test]
fn replan_zeroes_newly_allocated_slot() {
    let mut planner = Planner::with_capacity(POOL_CAPACITY);
    let plan_a = planner.build(one_source_graph(), None).unwrap();
    let mut engine = HeadlessEngine::new(plan_a, POOL_CAPACITY, ENV);

    for _ in 0..10 {
        engine.tick();
    }

    // Build plan B which adds spy. Spy gets a freshly allocated pool slot
    // (the planner's alloc_state had no entry for NodeId "spy").
    // Plan A is still in the engine — this mirrors normal replanning.
    let flag = Arc::new(AtomicBool::new(false));
    let plan_b = planner.build(two_source_graph(DropSpy::new(flag.clone())), None).unwrap();

    let spy_buf = plan_b
        .slots
        .iter()
        .find(|s| s.module.as_any().downcast_ref::<DropSpy>().is_some())
        .expect("DropSpy slot not found in plan B")
        .output_buffers[0];

    assert!(
        plan_b.to_zero.contains(&spy_buf),
        "newly allocated slot {spy_buf} must appear in plan_b.to_zero (got {:?})",
        plan_b.to_zero
    );

    // Contaminate the pool slot to prove zeroing is performed by adopt_plan,
    // not by the initial `vec![[0.0; 2]; capacity]` at engine construction.
    engine.pool[spy_buf] = [99.0; 2];

    engine.adopt_plan(plan_b);

    assert_eq!(
        engine.pool[spy_buf],
        [0.0; 2],
        "newly allocated slot {spy_buf} must be zeroed on plan adoption"
    );

    // The engine must produce correct output after the replan.
    // DropSpy outputs a constant 1.0; the 1-sample cable delay means the
    // first tick after adoption reads zero (zeroed slot), but by tick 3 the
    // value has propagated through both double-buffer halves.
    for _ in 0..100 {
        engine.tick();
    }
    assert_eq!(engine.last_left(), 1.0, "spy (constant 1.0) must be audible on the left channel");
    assert!(engine.last_right().abs() > 0.0, "sine must produce non-zero output on the right channel");
}
