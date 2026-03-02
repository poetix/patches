//! Integration tests: module state preservation across replans.
//!
//! Validates two properties of the replanning lifecycle:
//!
//! 1. **State preserved** — a module that survives a re-plan (same `InstanceId`
//!    in both the old and new graph) retains its internal state. The old
//!    (stateful) instance is pulled from the `prev_plan` registry and placed
//!    into the new plan in place of the fresh placeholder.
//!
//! 2. **Fresh state** — a module replaced by a new instance of the same type
//!    (different `InstanceId`) starts from its default state. The builder
//!    cannot find the fresh InstanceId in the old registry, so the fresh
//!    instance is used as-is.
//!
//! ## Why `prev_plan = Some(old_plan)` is used here
//!
//! The lifecycle tests in `replan_integration.rs` mirror the real engine flow
//! (`prev_plan = None`) because the running plan lives on the audio thread and
//! is inaccessible to the control thread. These tests exercise the
//! `prev_plan = Some(...)` path, which is used by [`PatchEngine::update`] via
//! its `held_plan` field — the plan most recently built but not yet consumed
//! by the audio thread. Passing `prev_plan` is what triggers module reuse.
//!
//! ## Test fixture: StatefulCounter
//!
//! `StatefulCounter` is a minimal module whose only state is a `u64` count
//! incremented on each `process` call. Unlike `SineOscillator`, it supports
//! construction with a predetermined `InstanceId` (`with_id`), which is
//! required to create "placeholder" instances for the identical-graph scenario.

use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleGraph, NodeId, PortDescriptor,
    PortRef,
};
use patches_engine::{ExecutionPlan, Planner};
use patches_modules::AudioOut;

// ── StatefulCounter ───────────────────────────────────────────────────────────

/// A module that outputs its call count as `f64` and supports construction
/// with a predetermined `InstanceId`.
///
/// Used to verify that the `Planner` correctly reuses old (stateful) instances
/// when the new graph contains a module with a matching `InstanceId`.
struct StatefulCounter {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    count: u64,
}

impl StatefulCounter {
    fn new() -> Self {
        Self::with_id(InstanceId::next())
    }

    /// Create a counter with a specific `InstanceId` and count starting at 0.
    ///
    /// Used to build "placeholder" graph modules whose `InstanceId` matches an
    /// existing instance in the registry so the builder substitutes the old
    /// (stateful) instance at plan-build time.
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

fn find_counter(plan: &ExecutionPlan) -> &StatefulCounter {
    plan.slots
        .iter()
        .find_map(|s| s.module.as_any().downcast_ref::<StatefulCounter>())
        .expect("StatefulCounter not found in plan")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// A module surviving a re-plan must retain its internal state and `InstanceId`.
///
/// Timeline:
///   1. Build plan A with `StatefulCounter` (id=X, count=0).
///   2. Tick 10 samples → count=10.
///   3. Build graph B with a placeholder counter (id=X, count=0) — same
///      `InstanceId`, zeroed state.
///   4. Build plan B passing plan A as `prev_plan`:
///      - The builder extracts plan A's modules into a registry keyed by
///        `InstanceId`.
///      - For the "counter" node, the new graph's module has id=X, which
///        matches the registry entry → the old (stateful) instance (count=10)
///        replaces the placeholder (count=0).
///   5. plan B contains the old counter: id=X, count=10.
#[test]
fn replan_preserves_state_for_surviving_instance() {
    let mut planner = Planner::with_capacity(POOL_CAPACITY);

    let counter_a = StatefulCounter::new();
    let id_a = counter_a.instance_id();

    let mut plan_a = planner.build(counter_graph(counter_a), None).unwrap();
    plan_a.initialise(&ENV);

    let mut pool = vec![[0.0f64; 2]; POOL_CAPACITY];
    for i in 0..10usize {
        plan_a.tick(&mut pool, i % 2);
    }

    assert_eq!(find_counter(&plan_a).count, 10, "counter must be 10 after 10 ticks");

    // Placeholder: same InstanceId as the old counter, but fresh state (count=0).
    // The builder will find id_a in the registry and substitute the stateful instance.
    let placeholder = StatefulCounter::with_id(id_a);
    let plan_b = planner.build(counter_graph(placeholder), Some(plan_a)).unwrap();

    let counter_b = find_counter(&plan_b);

    assert_eq!(
        counter_b.instance_id(),
        id_a,
        "surviving module must have the same InstanceId"
    );
    assert_eq!(
        counter_b.count,
        10,
        "surviving module's state (count=10) must be preserved across the replan"
    );
}

/// A module replaced by a fresh instance of the same type must start from its
/// default state and have a different `InstanceId`.
///
/// Timeline:
///   1. Build plan A with `StatefulCounter` (id=X, count=0).
///   2. Tick 10 samples → count=10.
///   3. Build graph B with a completely fresh counter (id=Y≠X, count=0).
///   4. Build plan B passing plan A as `prev_plan`:
///      - The builder looks up id=Y in the registry — not found.
///      - The fresh instance (count=0) is used as-is.
///   5. plan B contains the fresh counter: id=Y, count=0.
#[test]
fn replan_fresh_instance_starts_from_default_state() {
    let mut planner = Planner::with_capacity(POOL_CAPACITY);

    let counter_a = StatefulCounter::new();
    let id_a = counter_a.instance_id();

    let mut plan_a = planner.build(counter_graph(counter_a), None).unwrap();
    plan_a.initialise(&ENV);

    let mut pool = vec![[0.0f64; 2]; POOL_CAPACITY];
    for i in 0..10usize {
        plan_a.tick(&mut pool, i % 2);
    }

    // Fresh counter: new InstanceId (Y), count=0. Its InstanceId will not match
    // anything in the registry extracted from plan A.
    let fresh_counter = StatefulCounter::new();
    let id_b = fresh_counter.instance_id();

    assert_ne!(id_a, id_b, "precondition: fresh counter must have a different InstanceId");

    let plan_b = planner.build(counter_graph(fresh_counter), Some(plan_a)).unwrap();

    let counter_b = find_counter(&plan_b);

    assert_eq!(
        counter_b.instance_id(),
        id_b,
        "replacement module must carry the fresh InstanceId"
    );
    assert_ne!(
        counter_b.instance_id(),
        id_a,
        "replacement module must not be the old instance"
    );
    assert_eq!(
        counter_b.count,
        0,
        "fresh replacement instance must start from its default state (count=0)"
    );
}
