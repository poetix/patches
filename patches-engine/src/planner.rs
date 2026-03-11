use std::collections::HashSet;
use std::sync::Arc;

use patches_core::{AudioEnvironment, InstanceId, ModuleGraph, NodeId, Registry};

use patches_core::PlannerState;

use crate::builder::{BuildError, ExecutionPlan, PatchBuilder};
use crate::engine::{EngineError, SoundEngine, DEFAULT_MODULE_POOL_CAPACITY};
use crate::midi::{AudioClock, EventQueueConsumer};

/// Default cable buffer pool capacity.
///
/// 4096 slots accommodate up to 4096 concurrent output ports, which is more
/// than sufficient for all expected patch sizes. Each slot is 16 bytes
/// (`[f64; 2]`), so the pool is 64 KiB.
const DEFAULT_POOL_CAPACITY: usize = 4096;

/// Converts a [`ModuleGraph`] into an [`ExecutionPlan`] with stable buffer and
/// module pool allocation.
///
/// `Planner` carries [`PlannerState`] forward across successive
/// [`build`](Self::build) calls so that:
/// - Cables that share a `(NodeId, output_port_index)` key across re-plans reuse
///   the same buffer pool slot.
/// - Modules that share a [`NodeId`] and `module_name` across re-plans are
///   treated as surviving: they reuse their existing module pool slot and are
///   not reinstantiated.
///
/// # State preservation
///
/// Surviving modules remain in the audio-thread module pool between plan swaps.
/// The `Planner` assigns and tracks `InstanceId`s — surviving nodes keep the
/// same `InstanceId` so the audio thread continues to use the live instance.
pub struct Planner {
    state: PlannerState,
    builder: PatchBuilder,
    /// Instance IDs of modules that implement [`ReceivesMidi`] in the most
    /// recently built plan.  Carried forward across rebuilds: surviving
    /// instances remain in the set; tombstoned instances are dropped naturally
    /// because they no longer appear in `new_state.module_alloc.pool_map`.
    midi_receiver_instance_ids: HashSet<InstanceId>,
}

impl Default for Planner {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_POOL_CAPACITY)
    }
}

impl Planner {
    /// Create a new `Planner` with the default pool capacities.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new `Planner` with a specific buffer pool capacity.
    ///
    /// `pool_capacity` must match the capacity of the [`SoundEngine`]'s buffer
    /// pool so that [`BuildError::PoolExhausted`] is detected at plan-build time
    /// rather than at index-access time.
    ///
    /// The module pool capacity defaults to [`DEFAULT_MODULE_POOL_CAPACITY`].
    pub fn with_capacity(pool_capacity: usize) -> Self {
        Self {
            state: PlannerState::empty(),
            builder: PatchBuilder::new(pool_capacity, DEFAULT_MODULE_POOL_CAPACITY),
            midi_receiver_instance_ids: HashSet::new(),
        }
    }

    /// Build an [`ExecutionPlan`] from `graph`, updating internal allocation state.
    ///
    /// Surviving nodes (same [`NodeId`] and `module_name` as in the previous build)
    /// reuse their module pool slot; their state is preserved by the audio-thread pool.
    ///
    /// New and type-changed nodes are instantiated via `registry`. Removed nodes
    /// appear in `ExecutionPlan::tombstones` for the engine to free.
    pub fn build(
        &mut self,
        graph: &ModuleGraph,
        registry: &Registry,
        env: &AudioEnvironment,
    ) -> Result<ExecutionPlan, BuildError> {
        let (mut plan, new_state) = self.builder.build_patch(graph, registry, env, &self.state)?;

        // ── Populate midi_receiver_indices ────────────────────────────────────
        // Surviving MIDI receivers: their InstanceId is still present in the
        // new allocation map (tombstoned ones are absent and drop naturally).
        let mut new_midi_ids: HashSet<InstanceId> = self
            .midi_receiver_instance_ids
            .iter()
            .filter(|id| new_state.module_alloc.pool_map.contains_key(id))
            .copied()
            .collect();

        // Freshly installed modules: check via as_midi_receiver.
        for (_, m) in plan.new_modules.iter_mut() {
            if m.as_midi_receiver().is_some() {
                new_midi_ids.insert(m.instance_id());
            }
        }

        // Build the index list from the new InstanceId → pool-slot map.
        let mut midi_receiver_indices: Vec<usize> = new_midi_ids
            .iter()
            .filter_map(|id| new_state.module_alloc.pool_map.get(id).copied())
            .collect();
        midi_receiver_indices.sort_unstable();
        plan.midi_receiver_indices = midi_receiver_indices;

        self.midi_receiver_instance_ids = new_midi_ids;
        self.state = new_state;
        Ok(plan)
    }

    /// Return the [`InstanceId`] assigned to `node` in the most recent build.
    ///
    /// Returns `None` if `node` was not present in the last built graph.
    pub fn instance_id(&self, node: &NodeId) -> Option<InstanceId> {
        self.state.nodes.get(node).map(|ns| ns.instance_id)
    }
}

/// Coordinates patch planning (with state preservation) and audio execution.
///
/// `PatchEngine` ties together a [`Planner`], a [`SoundEngine`], and a
/// [`Registry`].
///
/// ## Normal flow
///
/// 1. [`new`](Self::new) creates the `PatchEngine` with a registry.
/// 2. [`start`](Self::start) opens the audio device, builds the initial plan
///    with the real [`AudioEnvironment`], and starts the audio thread.
/// 3. Each [`update`](Self::update) builds a new plan and pushes it to the
///    engine via [`swap_plan`](SoundEngine::swap_plan).
///
/// ## Channel-full path
///
/// If [`SoundEngine::swap_plan`] returns `Err` (channel full), `update` returns
/// [`PatchEngineError::ChannelFull`] immediately. The caller is responsible for
/// retrying with the same or an updated graph.
pub struct PatchEngine {
    planner: Planner,
    engine: SoundEngine,
    registry: Registry,
    /// The `AudioEnvironment` obtained from [`open`](SoundEngine::open).
    /// `None` until [`start`](Self::start) succeeds.
    env: Option<AudioEnvironment>,
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
    /// Retry [`update`](PatchEngine::update) after one buffer period (~10 ms).
    ChannelFull,
    /// [`update`](PatchEngine::update) was called before
    /// [`start`](PatchEngine::start).
    NotStarted,
}

impl std::fmt::Display for PatchEngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchEngineError::Build(e) => write!(f, "plan build error: {e}"),
            PatchEngineError::Engine(e) => write!(f, "engine error: {e}"),
            PatchEngineError::ChannelFull => {
                write!(f, "engine channel full; retry after one buffer period (~10 ms)")
            }
            PatchEngineError::NotStarted => {
                write!(f, "update() called before start(); call start() first")
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
    /// Create a `PatchEngine` with the given registry.
    ///
    /// Does not open the audio device or build a plan.
    /// Call [`start`](Self::start) to open the device and begin playback.
    pub fn new(registry: Registry) -> Result<Self, PatchEngineError> {
        let planner = Planner::with_capacity(DEFAULT_POOL_CAPACITY);
        let engine = SoundEngine::new(DEFAULT_POOL_CAPACITY, DEFAULT_MODULE_POOL_CAPACITY)?;
        Ok(Self {
            planner,
            engine,
            registry,
            env: None,
        })
    }

    /// Open the audio device, build the initial plan, and begin processing.
    ///
    /// Opens the device to obtain the real sample rate, builds the initial
    /// [`ExecutionPlan`] from `graph`, and starts the audio thread.
    ///
    /// Subsequent calls are no-ops if the engine is already running.
    pub fn start(
        &mut self,
        graph: &ModuleGraph,
        event_queue: Option<EventQueueConsumer>,
    ) -> Result<(), PatchEngineError> {
        if self.env.is_some() {
            return Ok(()); // already started
        }

        let env = self.engine.open().map_err(PatchEngineError::Engine)?;
        let plan = self.planner.build(graph, &self.registry, &env)?;
        self.engine.start(plan, event_queue).map_err(PatchEngineError::Engine)?;
        self.env = Some(env);
        Ok(())
    }

    /// Return the sample rate established when the engine was opened.
    ///
    /// `None` if [`start`](Self::start) has not yet been called.
    pub fn sample_rate(&self) -> Option<f64> {
        self.env.as_ref().map(|e| e.sample_rate)
    }

    /// Return a clone of the shared [`AudioClock`].
    ///
    /// Pass this to [`MidiConnector::open`](crate::MidiConnector::open) so
    /// that the MIDI callback can compute sample-accurate event positions.
    pub fn clock(&self) -> Arc<AudioClock> {
        self.engine.clock()
    }

    /// Apply an updated graph.
    ///
    /// Builds a new [`ExecutionPlan`] from `graph` and pushes it to the
    /// [`SoundEngine`] via [`swap_plan`](SoundEngine::swap_plan). Surviving
    /// modules retain their state via the audio-thread pool.
    ///
    /// Returns [`PatchEngineError::NotStarted`] if called before
    /// [`start`](Self::start).
    /// Returns [`PatchEngineError::ChannelFull`] if the engine's channel is
    /// already occupied. The caller is responsible for retrying.
    pub fn update(&mut self, graph: &ModuleGraph) -> Result<(), PatchEngineError> {
        let env = self.env.as_ref().ok_or(PatchEngineError::NotStarted)?;
        let new_plan = self.planner.build(graph, &self.registry, env)?;

        match self.engine.swap_plan(new_plan) {
            Ok(()) => Ok(()),
            Err(_returned_plan) => Err(PatchEngineError::ChannelFull),
        }
    }

    /// Return the [`InstanceId`] assigned to `node` in the most recent build.
    ///
    /// Returns `None` if `node` was not present in the last built graph.
    pub fn instance_id(&self, node: &NodeId) -> Option<InstanceId> {
        self.planner.instance_id(node)
    }

    /// Stop audio processing and close the device.
    pub fn stop(&mut self) {
        self.engine.stop();
    }
}

#[cfg(test)]
mod tests {
    use patches_core::{
        AudioEnvironment, InstanceId, MidiEvent, Module, ModuleDescriptor, ModuleGraph,
        ModuleShape, NodeId, PortDescriptor, PortRef, ReceivesMidi,
    };
    use patches_core::parameter_map::{ParameterMap, ParameterValue};
    use patches_modules::{AudioOut, Oscillator};

    use super::*;
    use crate::builder::ExecutionPlan;
    use crate::pool::ModulePool;

    fn p(name: &'static str) -> PortRef {
        PortRef { name, index: 0 }
    }

    fn simple_graph(freq: f64) -> ModuleGraph {
        let mut graph = ModuleGraph::new();
        let osc_desc = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
        let out_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
        let mut pm = ParameterMap::new();
        pm.insert("frequency".to_string(), ParameterValue::Float(freq));
        graph.add_module("osc", osc_desc, &pm).unwrap();
        graph.add_module("out", out_desc, &ParameterMap::new()).unwrap();
        graph.connect(&NodeId::from("osc"), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();
        graph.connect(&NodeId::from("osc"), p("sine"), &NodeId::from("out"), p("right"), 1.0).unwrap();
        graph
    }

    // ── Counter: a stateful stub module that counts process() calls ──────────

    struct Counter {
        instance_id: InstanceId,
        descriptor: ModuleDescriptor,
        count: u64,
    }

    impl Module for Counter {
        fn describe(shape: &ModuleShape) -> ModuleDescriptor {
            ModuleDescriptor {
                module_name: "Counter",
                shape: shape.clone(),
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
                parameters: vec![],
                is_sink: false,
            }
        }

        fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
            Self {
                instance_id,
                descriptor,
                count: 0,
            }
        }

        fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

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

    fn counter_graph() -> ModuleGraph {
        let counter_desc = Counter::describe(&ModuleShape { channels: 0, length: 0 });
        let out_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
        let mut g = ModuleGraph::new();
        g.add_module("counter", counter_desc, &ParameterMap::new()).unwrap();
        g.add_module("out", out_desc, &ParameterMap::new()).unwrap();
        g.connect(&NodeId::from("counter"), p("out"), &NodeId::from("out"), p("left"), 1.0)
            .unwrap();
        g.connect(&NodeId::from("counter"), p("out"), &NodeId::from("out"), p("right"), 1.0)
            .unwrap();
        g
    }

    fn make_buffer_pool(capacity: usize) -> Vec<[f64; 2]> {
        vec![[0.0; 2]; capacity]
    }

    /// Install a plan's new_modules into `pool` and process tombstones,
    /// simulating what SoundEngine does on plan adoption.
    fn adopt_plan(plan: &mut ExecutionPlan, pool: &mut ModulePool) {
        for &idx in &plan.tombstones {
            pool.tombstone(idx);
        }
        for (idx, m) in plan.new_modules.drain(..) {
            pool.install(idx, m);
        }
    }

    /// Return the output buffer index for the Counter slot in `plan`.
    fn counter_output_buf(plan: &ExecutionPlan) -> usize {
        let ao_pool = plan.slots[plan.audio_out_index].pool_index;
        let counter_slot = plan.slots.iter().find(|s| s.pool_index != ao_pool).unwrap();
        counter_slot.output_buffers[0]
    }

    #[test]
    fn planner_reuses_module_instance_across_rebuild() {
        let mut registry = patches_modules::default_registry();
        registry.register::<Counter>();
        let env = AudioEnvironment { sample_rate: 44100.0 };
        let mut planner = Planner::new();
        let mut pool = ModulePool::new(64);

        let graph = counter_graph();
        let mut plan_a = planner.build(&graph, &registry, &env).unwrap();
        adopt_plan(&mut plan_a, &mut pool);

        let mut buffer_pool = make_buffer_pool(256);
        for i in 0..5 {
            plan_a.tick(&mut pool, &mut buffer_pool, i % 2);
        }
        // After 5 ticks (wi sequence 0,1,0,1,0), Counter wrote 5.0 into wi=0 slot.

        // Build graph_b with same graph — counter is a surviving module.
        let mut plan_b = planner.build(&graph, &registry, &env).unwrap();

        // Counter must NOT appear in new_modules (it is surviving).
        assert!(
            plan_b.new_modules.is_empty(),
            "surviving Counter must not appear in new_modules"
        );

        adopt_plan(&mut plan_b, &mut pool);

        // Continue from wi=1 (plan_a last wi=0, so plan_b ticks at wi=1).
        plan_b.tick(&mut pool, &mut buffer_pool, 1);

        // Counter wrote its new count (6) into the wi=1 buffer slot.
        let buf = counter_output_buf(&plan_b);
        assert_eq!(
            buffer_pool[buf][1], 6.0,
            "state must be preserved: count was 5, ticked once → 6"
        );
    }

    #[test]
    fn planner_uses_fresh_modules_when_no_prev_plan() {
        let mut registry = patches_modules::default_registry();
        registry.register::<Counter>();
        let env = AudioEnvironment { sample_rate: 44100.0 };
        let mut planner = Planner::new();
        let mut pool = ModulePool::new(64);

        let graph = counter_graph();
        let mut plan = planner.build(&graph, &registry, &env).unwrap();
        adopt_plan(&mut plan, &mut pool);

        let mut buffer_pool = make_buffer_pool(256);
        plan.tick(&mut pool, &mut buffer_pool, 0);

        let buf = counter_output_buf(&plan);
        assert_eq!(buffer_pool[buf][0], 1.0, "fresh plan: count starts at 0, ticked once → 1");
    }

    #[test]
    fn planner_build_succeeds_for_valid_graph() {
        let registry = patches_modules::default_registry();
        let env = AudioEnvironment { sample_rate: 44100.0 };
        let mut planner = Planner::new();
        assert!(planner.build(&simple_graph(440.0), &registry, &env).is_ok());
    }

    #[test]
    fn planner_build_fails_for_empty_graph() {
        let registry = patches_modules::default_registry();
        let env = AudioEnvironment { sample_rate: 44100.0 };
        let mut planner = Planner::new();
        assert!(planner.build(&ModuleGraph::new(), &registry, &env).is_err());
    }

    // ── MidiReceiver: a stub module that implements ReceivesMidi ─────────────

    struct MidiReceiver {
        instance_id: InstanceId,
        descriptor: ModuleDescriptor,
        pub received: Vec<MidiEvent>,
    }

    impl Module for MidiReceiver {
        fn describe(shape: &ModuleShape) -> ModuleDescriptor {
            ModuleDescriptor {
                module_name: "MidiReceiver",
                shape: shape.clone(),
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
                parameters: vec![],
                is_sink: false,
            }
        }

        fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
            Self { instance_id, descriptor, received: Vec::new() }
        }

        fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

        fn descriptor(&self) -> &ModuleDescriptor {
            &self.descriptor
        }

        fn instance_id(&self) -> InstanceId {
            self.instance_id
        }

        fn process(&mut self, _inputs: &[f64], outputs: &mut [f64]) {
            outputs[0] = 0.0;
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn as_midi_receiver(&mut self) -> Option<&mut dyn ReceivesMidi> {
            Some(self)
        }
    }

    impl ReceivesMidi for MidiReceiver {
        fn receive_midi(&mut self, event: MidiEvent) {
            self.received.push(event);
        }
    }

    /// Build a graph with one MidiReceiver and one non-MIDI Counter wired to AudioOut.
    fn mixed_midi_graph() -> ModuleGraph {
        let recv_desc = MidiReceiver::describe(&ModuleShape { channels: 0, length: 0 });
        let counter_desc = Counter::describe(&ModuleShape { channels: 0, length: 0 });
        let out_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
        let mut g = ModuleGraph::new();
        g.add_module("recv", recv_desc, &ParameterMap::new()).unwrap();
        g.add_module("counter", counter_desc, &ParameterMap::new()).unwrap();
        g.add_module("out", out_desc, &ParameterMap::new()).unwrap();
        g.connect(&NodeId::from("recv"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
        g.connect(&NodeId::from("counter"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
        g
    }

    #[test]
    fn planner_midi_receiver_indices_contains_only_midi_capable_modules() {
        let mut registry = patches_modules::default_registry();
        registry.register::<MidiReceiver>();
        registry.register::<Counter>();
        let env = AudioEnvironment { sample_rate: 44100.0 };
        let mut planner = Planner::new();

        let graph = mixed_midi_graph();
        let plan = planner.build(&graph, &registry, &env).unwrap();

        // Exactly one module in this plan is a MIDI receiver.
        assert_eq!(
            plan.midi_receiver_indices.len(), 1,
            "only the MidiReceiver module should be in midi_receiver_indices"
        );

        // The index in the list must not be the AudioOut slot.
        let ao_pool = plan.slots[plan.audio_out_index].pool_index;
        let midi_idx = plan.midi_receiver_indices[0];
        assert_ne!(midi_idx, ao_pool, "AudioOut must not be a MIDI receiver");

        // The index must correspond to MidiReceiver, not Counter.
        // Verify by confirming Counter's pool slot is absent.
        let counter_pool = plan.slots.iter()
            .find(|s| s.pool_index != ao_pool && s.pool_index != midi_idx)
            .map(|s| s.pool_index);
        assert!(
            counter_pool.is_some(),
            "Counter must occupy a distinct slot not listed as MIDI receiver"
        );
    }

    #[test]
    fn planner_midi_receiver_indices_survive_rebuild() {
        let mut registry = patches_modules::default_registry();
        registry.register::<MidiReceiver>();
        registry.register::<Counter>();
        let env = AudioEnvironment { sample_rate: 44100.0 };
        let mut planner = Planner::new();

        let graph = mixed_midi_graph();
        let plan_a = planner.build(&graph, &registry, &env).unwrap();
        assert_eq!(plan_a.midi_receiver_indices.len(), 1, "first build: one MIDI receiver");

        // Rebuild the same graph — MidiReceiver survives as an Update node.
        let plan_b = planner.build(&graph, &registry, &env).unwrap();
        assert_eq!(
            plan_b.midi_receiver_indices, plan_a.midi_receiver_indices,
            "midi_receiver_indices must be stable across a no-change rebuild"
        );
    }

}
