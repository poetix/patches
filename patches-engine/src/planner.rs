use patches_core::ModuleGraph;

use crate::builder::{build_patch, BuildError, ExecutionPlan};
use crate::engine::{EngineError, SoundEngine};

/// A stateless, audio-agnostic patch builder.
///
/// `Planner` converts a [`ModuleGraph`] into an [`ExecutionPlan`]. If a
/// previous plan is supplied, module instances with matching
/// [`InstanceId`](patches_core::InstanceId)s are reused — preserving any
/// internal state (e.g. oscillator phase) accumulated since the plan was built.
///
/// `Planner` itself is stateless; callers retain the previous plan and pass it
/// back at each re-plan. This makes planning fully testable without a running
/// audio device.
///
/// # State freshness
///
/// The state preserved in the new plan reflects the module state *at the time
/// the previous plan was built*, not the audio thread's live state at swap time.
/// For live-coding use cases (re-plans every few seconds) this difference is
/// negligible. See `adr/0003-planner-state-freshness.md` for the trade-off
/// record.
#[derive(Default)]
pub struct Planner;

impl Planner {
    /// Create a new `Planner`.
    pub fn new() -> Self {
        Self
    }

    /// Build an [`ExecutionPlan`] from `graph`.
    ///
    /// If `prev_plan` is `Some`, module instances with matching
    /// [`InstanceId`](patches_core::InstanceId)s are reused, preserving their
    /// internal state. Unmatched instances from the old plan are dropped.
    ///
    /// If `prev_plan` is `None`, all modules are taken fresh from `graph`.
    pub fn build(
        &self,
        graph: ModuleGraph,
        prev_plan: Option<ExecutionPlan>,
    ) -> Result<ExecutionPlan, BuildError> {
        let mut registry = prev_plan
            .map(|p| p.into_registry())
            .unwrap_or_default();
        build_patch(graph, Some(&mut registry))
    }
}

/// Coordinates patch planning (with state preservation) and audio execution.
///
/// `PatchEngine` ties together a [`Planner`] and a [`SoundEngine`].  It keeps
/// a *held plan* — the most recently built plan that has not yet been adopted by
/// the audio thread — so that rapid consecutive calls to [`update`](Self::update)
/// can preserve module state between rebuilds.
///
/// ## Normal flow
///
/// 1. [`new`](Self::new) builds the initial plan and hands it to `SoundEngine`.
/// 2. [`start`](Self::start) opens the audio device.
/// 3. Each [`update`](Self::update) builds a new plan (optionally reusing state
///    from the held plan) and pushes it to the engine via
///    [`swap_plan`](SoundEngine::swap_plan).
///
/// ## Channel-full / retry flow
///
/// [`SoundEngine::swap_plan`] uses a single-slot lock-free channel. If the
/// audio thread has not yet consumed the previous plan, `swap_plan` returns
/// the plan. In that case `update` stores the plan as the new held plan and
/// returns [`PatchEngineError::ChannelFull`]. The caller may retry after one
/// buffer period (~10 ms); the next `update` call will reuse the held plan's
/// module instances for state preservation.
///
/// ## State freshness
///
/// State preserved in a rebuilt plan comes from the module instances at the
/// time the *previous plan was built*, not from the engine's live audio state.
/// See `adr/0003-planner-state-freshness.md`.
pub struct PatchEngine {
    planner: Planner,
    engine: SoundEngine,
    /// Most recently built plan that has not yet been consumed by a build or
    /// sent to the engine. `None` in normal operation after each successful
    /// `swap_plan`; `Some` when a swap was rejected (channel full) so that
    /// the next `update` can reuse its module instances.
    held_plan: Option<ExecutionPlan>,
}

/// Errors returned by [`PatchEngine`] operations.
#[derive(Debug)]
pub enum PatchEngineError {
    /// An error occurred while building an [`ExecutionPlan`].
    Build(BuildError),
    /// An error occurred in the underlying [`SoundEngine`].
    Engine(EngineError),
    /// The new plan could not be sent because the engine's single-slot channel
    /// is already full.
    ///
    /// The plan has been stored internally as the held plan. Retry
    /// [`update`](PatchEngine::update) after one buffer period (~10 ms).
    ChannelFull,
}

impl std::fmt::Display for PatchEngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchEngineError::Build(e) => write!(f, "plan build error: {e}"),
            PatchEngineError::Engine(e) => write!(f, "engine error: {e}"),
            PatchEngineError::ChannelFull => {
                write!(f, "engine channel full; retry after one buffer period (~10 ms)")
            }
        }
    }
}

impl std::error::Error for PatchEngineError {}

impl From<BuildError> for PatchEngineError {
    fn from(e: BuildError) -> Self {
        Self::Build(e)
    }
}

impl From<EngineError> for PatchEngineError {
    fn from(e: EngineError) -> Self {
        Self::Engine(e)
    }
}

impl PatchEngine {
    /// Create a `PatchEngine` from an initial graph.
    ///
    /// Builds the first plan and constructs the underlying [`SoundEngine`], but
    /// does not open the audio device. Call [`start`](Self::start) to begin
    /// playback.
    pub fn new(graph: ModuleGraph) -> Result<Self, PatchEngineError> {
        let planner = Planner::new();
        let plan = planner.build(graph, None)?;
        let engine = SoundEngine::new(plan)?;
        Ok(Self {
            planner,
            engine,
            held_plan: None,
        })
    }

    /// Open the audio device and begin processing.
    ///
    /// Subsequent calls are no-ops if the engine is already running.
    pub fn start(&mut self) -> Result<(), PatchEngineError> {
        self.engine.start().map_err(PatchEngineError::Engine)
    }

    /// Apply an updated graph, reusing module state from the held plan.
    ///
    /// Builds a new [`ExecutionPlan`] from `graph`. If a held plan is present
    /// (from a previous failed `update` or pre-start build), its module instances
    /// are reused for state preservation. The new plan is then pushed to the
    /// [`SoundEngine`] via [`swap_plan`](SoundEngine::swap_plan).
    ///
    /// Returns [`PatchEngineError::ChannelFull`] if the engine's channel is
    /// already occupied. The new plan is retained as the held plan in this case;
    /// the caller may retry without losing the build result.
    pub fn update(&mut self, graph: ModuleGraph) -> Result<(), PatchEngineError> {
        let new_plan = self.planner.build(graph, self.held_plan.take())?;

        match self.engine.swap_plan(new_plan) {
            Ok(()) => Ok(()),
            Err(returned_plan) => {
                // Channel full: stash the plan so the next update can reuse its
                // module instances and the caller can retry.
                self.held_plan = Some(returned_plan);
                Err(PatchEngineError::ChannelFull)
            }
        }
    }

    /// Stop audio processing and close the device.
    pub fn stop(&mut self) {
        self.engine.stop();
    }
}

#[cfg(test)]
mod tests {
    use patches_core::{InstanceId, Module, ModuleDescriptor, ModuleGraph, PortDescriptor};
    use patches_modules::{AudioOut, SineOscillator};

    use super::*;

    /// Build a simple valid graph: one SineOscillator connected to an AudioOut.
    fn simple_graph(freq: f64) -> ModuleGraph {
        let mut graph = ModuleGraph::new();
        let osc = graph.add_module(Box::new(SineOscillator::new(freq)));
        let out = graph.add_module(Box::new(AudioOut::new()));
        graph.connect(osc, "out", out, "left", 1.0).unwrap();
        graph.connect(osc, "out", out, "right", 1.0).unwrap();
        graph
    }

    /// A stateful stub module that counts how many times `process` has been called.
    struct Counter {
        instance_id: InstanceId,
        descriptor: ModuleDescriptor,
        pub count: u64,
    }

    impl Counter {
        fn new() -> Self {
            Self::with_id(InstanceId::next())
        }

        /// Create a Counter with a predetermined InstanceId. Used in tests that
        /// need to match an existing registry entry (same ID, different object).
        fn with_id(id: InstanceId) -> Self {
            Self {
                instance_id: id,
                descriptor: ModuleDescriptor {
                    inputs: vec![],
                    outputs: vec![PortDescriptor { name: "out" }],
                },
                count: 0,
            }
        }
    }

    impl Module for Counter {
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

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    /// Build a graph with a Counter driving AudioOut.
    fn counter_graph() -> (ModuleGraph, InstanceId) {
        let counter = Counter::new();
        let id = counter.instance_id();
        let mut graph = ModuleGraph::new();
        let c = graph.add_module(Box::new(counter));
        let out = graph.add_module(Box::new(AudioOut::new()));
        graph.connect(c, "out", out, "left", 1.0).unwrap();
        graph.connect(c, "out", out, "right", 1.0).unwrap();
        (graph, id)
    }

    #[test]
    fn planner_reuses_module_instance_across_rebuild() {
        let planner = Planner::new();

        let (graph_a, counter_id) = counter_graph();
        let mut plan_a = planner.build(graph_a, None).unwrap();

        // Advance counter by ticking the plan.
        for i in 0..5 {
            plan_a.tick(i % 2);
        }

        // Build graph_b with a fresh Counter that deliberately shares the same
        // InstanceId as the one in plan_a. The Planner will find it in the
        // registry extracted from plan_a and replace the graph_b placeholder
        // with the old, stateful instance (count = 5).
        let mut graph_b = ModuleGraph::new();
        let placeholder = Counter::with_id(counter_id); // same ID, count = 0
        let c = graph_b.add_module(Box::new(placeholder));
        let out = graph_b.add_module(Box::new(AudioOut::new()));
        graph_b.connect(c, "out", out, "left", 1.0).unwrap();
        graph_b.connect(c, "out", out, "right", 1.0).unwrap();

        let mut plan_b = planner.build(graph_b, Some(plan_a)).unwrap();

        // The counter in plan_b is the old Counter with count=5; the next tick
        // increments it to 6. wi=1 continues the alternating sequence (plan_a had 5 ticks).
        plan_b.tick(1);

        let counter = plan_b
            .slots
            .iter_mut()
            .find_map(|s| s.module.as_any_mut().downcast_mut::<Counter>())
            .expect("Counter module not found in plan_b");

        assert_eq!(counter.count, 6, "state should be preserved: count was 5, ticked once → 6");
    }

    #[test]
    fn planner_uses_fresh_modules_when_no_prev_plan() {
        let planner = Planner::new();
        let (graph, _) = counter_graph();
        let mut plan = planner.build(graph, None).unwrap();
        plan.tick(0);

        let counter = plan
            .slots
            .iter_mut()
            .find_map(|s| s.module.as_any_mut().downcast_mut::<Counter>())
            .expect("Counter module not found");

        assert_eq!(counter.count, 1, "fresh plan: count starts at 0, ticked once → 1");
    }

    #[test]
    fn planner_build_succeeds_for_valid_graph() {
        let planner = Planner::new();
        assert!(planner.build(simple_graph(440.0), None).is_ok());
    }

    #[test]
    fn planner_build_fails_for_empty_graph() {
        let planner = Planner::new();
        assert!(planner.build(ModuleGraph::new(), None).is_err());
    }
}
