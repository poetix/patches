//! Integration tests: hot-reload replanning with graph modification.
//!
//! Validates three properties of the replanning lifecycle:
//!
//! 1. **Drop** — modules removed from the graph are dropped when the engine
//!    *adopts* the new plan (i.e. when `adopt_plan` processes tombstones),
//!    not during `Planner::build`.
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
//! plan_a = Planner::build(g_a)
//! engine.adopt_plan(plan_a)   ──►  current_plan = plan_a
//!                                  loop { tick() }
//! plan_b = Planner::build(g_b)   ← old plan never leaves audio thread
//! engine.adopt_plan(plan_b)   ──►  install new_modules, tombstone removed
//!                                  zero to_zero slots
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
    PortRef,
};
use patches_engine::{ExecutionPlan, Planner};
use patches_modules::{AudioOut, SineOscillator};

// ── HeadlessEngine ────────────────────────────────────────────────────────────

/// Synchronous, device-free engine fixture that mirrors the real audio callback.
///
/// The CPAL callback does, in order:
///   1. Pop a new plan from the lock-free channel (if available).
///   2. Install `new_modules` into the module pool.
///   3. Tombstone removed modules (`pool[idx].take()`).
///   4. Zero every index in `new_plan.to_zero`.
///   5. Set `current_plan = new_plan`.
///   6. Tick all modules for each sample in the buffer.
///
/// `adopt_plan` replicates steps 2–5; `tick` replicates step 6.
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

    /// Adopt a new execution plan, mirroring the audio-callback plan-swap sequence:
    ///
    ///   1. Tombstone removed modules (`module_pool[idx].take()`).
    ///   2. Install `plan.new_modules` into the module pool (initialising first).
    ///   3. Zero every slot in `plan.to_zero`.
    ///   4. Replace `self.plan`.
    ///
    /// Tombstones are processed before new modules because the freelist may
    /// recycle tombstoned slots for new modules.
    fn adopt_plan(&mut self, mut plan: ExecutionPlan) {
        // Tombstone removed modules first — drop happens here.
        for &idx in &plan.tombstones {
            self.module_pool[idx].take();
        }
        // Install new modules (initialise before inserting).
        for (idx, mut m) in plan.new_modules.drain(..) {
            m.initialise(&self.env);
            self.module_pool[idx] = Some(m);
        }
        // Zero freed/new cable buffer slots.
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

    fn last_left(&self) -> f64 {
        self.plan.as_ref().map_or(0.0, |p| p.last_left(&self.module_pool))
    }

    fn last_right(&self) -> f64 {
        self.plan.as_ref().map_or(0.0, |p| p.last_right(&self.module_pool))
    }

    fn pool_slot(&self, idx: usize) -> [f64; 2] {
        self.buffer_pool[idx]
    }
}

// ── DropSpy ──────────────────────────────────────────────────────────────────

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
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
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
        outputs[0] = 1.0;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ── Graph builders ────────────────────────────────────────────────────────────

const BUFFER_POOL_CAPACITY: usize = 256;
const MODULE_POOL_CAPACITY: usize = 64;
const ENV: AudioEnvironment = AudioEnvironment { sample_rate: 48_000.0 };

fn p(name: &'static str) -> PortRef {
    PortRef { name, index: 0 }
}

fn two_source_graph(spy: DropSpy) -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let spy_id = NodeId::from("spy");
    let sine_id = NodeId::from("sine");
    let out_id = NodeId::from("out");
    graph.add_module(spy_id.clone(), Box::new(spy)).unwrap();
    graph.add_module(sine_id.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
    graph.add_module(out_id.clone(), Box::new(AudioOut::new())).unwrap();
    graph.connect(&spy_id, p("out"), &out_id, p("left"), 1.0).unwrap();
    graph.connect(&sine_id, p("out"), &out_id, p("right"), 1.0).unwrap();
    graph
}

fn one_source_graph() -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let sine_id = NodeId::from("sine");
    let out_id = NodeId::from("out");
    graph.add_module(sine_id.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
    graph.add_module(out_id.clone(), Box::new(AudioOut::new())).unwrap();
    graph.connect(&sine_id, p("out"), &out_id, p("left"), 1.0).unwrap();
    graph.connect(&sine_id, p("out"), &out_id, p("right"), 1.0).unwrap();
    graph
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Modules removed from the graph must be dropped when the engine adopts the
/// new plan (tombstone processing), not earlier.
///
/// Checkpoints:
///   - After `Planner::build(graph_b)` returns: DropSpy is **alive**
///     (the build did not touch the module pool).
///   - After `engine.adopt_plan(plan_b)` returns: DropSpy is **dead**
///     (tombstone processing called `module_pool[idx].take()`).
#[test]
fn replan_drops_removed_module() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut planner = Planner::with_capacity(BUFFER_POOL_CAPACITY);
    let plan_a = planner.build(two_source_graph(DropSpy::new(flag.clone()))).unwrap();
    let mut engine = HeadlessEngine::new(plan_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY, ENV);

    for _ in 0..10 {
        engine.tick();
    }

    assert!(!flag.load(Ordering::SeqCst), "spy must not be dropped while plan A is active");

    let plan_b = planner.build(one_source_graph()).unwrap();

    assert!(
        !flag.load(Ordering::SeqCst),
        "spy must still be alive after Planner::build — the module pool has not been updated yet"
    );

    // adopt_plan processes tombstones: module_pool[spy_slot].take() drops DropSpy.
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
/// Timeline:
///   1. Plan A (`spy` + `sine`) runs for several ticks → spy's pool slot is
///      non-zero (DropSpy writes `1.0` every sample).
///   2. Plan B (`sine` only) is built — spy's slot is freed by Planner and
///      listed in `plan_b.to_zero`.
///   3. Pool is inspected *before* adoption: slot is still dirty.
///   4. `adopt_plan(plan_b)` zeroes the slot and tombstones DropSpy.
#[test]
fn replan_zeroes_freed_buffer_slot() {
    let flag = Arc::new(AtomicBool::new(false));
    let mut planner = Planner::with_capacity(BUFFER_POOL_CAPACITY);
    let plan_a = planner.build(two_source_graph(DropSpy::new(flag.clone()))).unwrap();

    // Find spy's output buffer index from new_modules before adopting.
    let spy_pool_idx = plan_a
        .new_modules
        .iter()
        .find(|(_, m)| m.as_any().downcast_ref::<DropSpy>().is_some())
        .map(|(idx, _)| *idx)
        .expect("DropSpy not found in plan_a.new_modules");
    let spy_buf = plan_a
        .slots
        .iter()
        .find(|s| s.pool_index == spy_pool_idx)
        .expect("no slot with spy's pool_index")
        .output_buffers[0];

    let mut engine =
        HeadlessEngine::new(plan_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY, ENV);

    for _ in 0..4 {
        engine.tick();
    }

    let dirty = engine.pool_slot(spy_buf);
    assert!(
        dirty != [0.0; 2],
        "spy's buffer slot must be non-zero after ticking (got {dirty:?})"
    );

    let plan_b = planner.build(one_source_graph()).unwrap();

    assert!(
        plan_b.to_zero.contains(&spy_buf),
        "freed slot {spy_buf} must appear in plan_b.to_zero (got {:?})",
        plan_b.to_zero
    );

    assert!(
        engine.pool_slot(spy_buf) != [0.0; 2],
        "pool must still be dirty before adopt_plan is called"
    );

    engine.adopt_plan(plan_b);

    assert_eq!(
        engine.pool_slot(spy_buf),
        [0.0; 2],
        "freed slot {spy_buf} must be zeroed when plan B is adopted"
    );
}

/// Buffer pool slots allocated for a *newly added* module must also appear in
/// `to_zero` and be zeroed on adoption.
#[test]
fn replan_zeroes_newly_allocated_slot() {
    let mut planner = Planner::with_capacity(BUFFER_POOL_CAPACITY);
    let plan_a = planner.build(one_source_graph()).unwrap();
    let mut engine = HeadlessEngine::new(plan_a, BUFFER_POOL_CAPACITY, MODULE_POOL_CAPACITY, ENV);

    for _ in 0..10 {
        engine.tick();
    }

    let flag = Arc::new(AtomicBool::new(false));
    let plan_b = planner.build(two_source_graph(DropSpy::new(flag.clone()))).unwrap();

    // Find spy's buffer slot from new_modules before adopting.
    let spy_pool_idx = plan_b
        .new_modules
        .iter()
        .find(|(_, m)| m.as_any().downcast_ref::<DropSpy>().is_some())
        .map(|(idx, _)| *idx)
        .expect("DropSpy not found in plan_b.new_modules");
    let spy_buf = plan_b
        .slots
        .iter()
        .find(|s| s.pool_index == spy_pool_idx)
        .expect("no slot with spy's pool_index")
        .output_buffers[0];

    assert!(
        plan_b.to_zero.contains(&spy_buf),
        "newly allocated slot {spy_buf} must appear in plan_b.to_zero (got {:?})",
        plan_b.to_zero
    );

    engine.buffer_pool[spy_buf] = [99.0; 2];

    engine.adopt_plan(plan_b);

    assert_eq!(
        engine.pool_slot(spy_buf),
        [0.0; 2],
        "newly allocated slot {spy_buf} must be zeroed on plan adoption"
    );

    for _ in 0..100 {
        engine.tick();
    }
    assert_eq!(engine.last_left(), 1.0, "spy (constant 1.0) must be audible on the left channel");
    assert!(engine.last_right().abs() > 0.0, "sine must produce non-zero output on the right channel");
}
