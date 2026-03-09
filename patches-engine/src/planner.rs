use patches_core::{AudioEnvironment, ControlSignal, InstanceId, ModuleGraph, NodeId, Registry};

use patches_core::PlannerState;

use crate::builder::{BuildError, ExecutionPlan, PatchBuilder};
use crate::engine::{EngineError, SoundEngine, DEFAULT_MODULE_POOL_CAPACITY};

/// Default cable buffer pool capacity.
///
/// 4096 slots accommodate up to 4096 concurrent output ports, which is more
/// than sufficient for all expected patch sizes. Each slot is 16 bytes
/// (`[f64; 2]`), so the pool is 64 KiB.
const DEFAULT_POOL_CAPACITY: usize = 4096;

/// Default number of audio samples between control-rate ticks.
///
/// At 48 kHz this gives a control rate of 750 Hz (~1.3 ms per tick).
const DEFAULT_CONTROL_PERIOD: usize = 64;

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
        let (plan, new_state) = self.builder.build_patch(graph, registry, env, &self.state)?;
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
    /// Constructs the underlying [`SoundEngine`] with the default control period
    /// (64 samples), but does not open the audio device or build a plan.
    /// Call [`start`](Self::start) to open the device and begin playback.
    pub fn new(registry: Registry) -> Result<Self, PatchEngineError> {
        Self::with_control_period(registry, DEFAULT_CONTROL_PERIOD)
    }

    /// Create a `PatchEngine` with a specific control period.
    ///
    /// `control_period` is the number of audio samples between control-rate
    /// ticks (signal dispatch). Must be greater than zero.
    pub fn with_control_period(
        registry: Registry,
        control_period: usize,
    ) -> Result<Self, PatchEngineError> {
        let planner = Planner::with_capacity(DEFAULT_POOL_CAPACITY);
        let engine = SoundEngine::new(
            DEFAULT_POOL_CAPACITY,
            DEFAULT_MODULE_POOL_CAPACITY,
            control_period,
        )?;
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
    pub fn start(&mut self, graph: &ModuleGraph) -> Result<(), PatchEngineError> {
        if self.env.is_some() {
            return Ok(()); // already started
        }

        let env = self.engine.open().map_err(PatchEngineError::Engine)?;
        let plan = self.planner.build(graph, &self.registry, &env)?;
        self.engine.start(plan).map_err(PatchEngineError::Engine)?;
        self.env = Some(env);
        Ok(())
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

    /// Enqueue a [`ControlSignal`] for delivery to the module identified by `id`.
    ///
    /// Delegates to [`SoundEngine::send_signal`]. Returns `Err(signal)` if the
    /// ring buffer is full; the caller may drop or retry.
    pub fn send_signal(
        &mut self,
        id: InstanceId,
        signal: ControlSignal,
    ) -> Result<(), ControlSignal> {
        self.engine.send_signal(id, signal)
    }

    /// Stop audio processing and close the device.
    pub fn stop(&mut self) {
        self.engine.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use patches_core::{
        AudioEnvironment, ControlSignal, InstanceId, Module, ModuleDescriptor, ModuleGraph,
        ModuleShape, NodeId, PortDescriptor, PortRef,
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

    // ── Signal dispatch tests ─────────────────────────────────────────────────

    /// Records how many signals it has received via an `Arc<AtomicUsize>` shared
    /// with the test.
    struct SignalReceiver {
        instance_id: InstanceId,
        descriptor: ModuleDescriptor,
        received_count: Arc<AtomicUsize>,
    }

    impl Module for SignalReceiver {
        fn describe(shape: &ModuleShape) -> ModuleDescriptor {
            ModuleDescriptor {
                module_name: "SignalReceiver",
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
                received_count: Arc::new(AtomicUsize::new(0)),
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
            outputs[0] = 0.0;
        }

        fn receive_signal(&mut self, _signal: ControlSignal) {
            self.received_count.fetch_add(1, Ordering::SeqCst);
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn receiver_graph() -> ModuleGraph {
        let recv_desc = SignalReceiver::describe(&ModuleShape { channels: 0, length: 0 });
        let out_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
        let mut g = ModuleGraph::new();
        g.add_module("recv", recv_desc, &ParameterMap::new()).unwrap();
        g.add_module("out", out_desc, &ParameterMap::new()).unwrap();
        g.connect(&NodeId::from("recv"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
        g.connect(&NodeId::from("recv"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
        g
    }

    #[test]
    fn signal_delivered_at_control_tick_not_before() {
        let mut registry = patches_modules::default_registry();
        registry.register::<SignalReceiver>();
        let env = AudioEnvironment { sample_rate: 44100.0 };
        let mut planner = Planner::new();

        let graph = receiver_graph();
        let mut plan = planner.build(&graph, &registry, &env).unwrap();
        let recv_id = planner.instance_id(&NodeId::from("recv")).unwrap();

        let mut pool = ModulePool::new(64);
        let mut received_count: Option<Arc<AtomicUsize>> = None;
        for (idx, module) in plan.new_modules.drain(..) {
            if let Some(recv) = module.as_any().downcast_ref::<SignalReceiver>() {
                received_count = Some(Arc::clone(&recv.received_count));
            }
            pool.install(idx, module);
        }
        let received_count = received_count.expect("SignalReceiver not found in new_modules");

        let mut buffer_pool = make_buffer_pool(256);
        for i in 0..3usize {
            plan.tick(&mut pool, &mut buffer_pool, i % 2);
        }

        assert_eq!(
            received_count.load(Ordering::SeqCst),
            0,
            "signal must not arrive before dispatch_signal is called"
        );

        plan.dispatch_signal(
            recv_id,
            ControlSignal::Float { name: "test", value: 0.0 },
            &mut pool,
        );

        assert_eq!(
            received_count.load(Ordering::SeqCst),
            1,
            "signal must arrive after dispatch_signal is called"
        );
    }

    #[test]
    fn signal_for_unknown_id_is_silently_dropped() {
        let mut registry = patches_modules::default_registry();
        registry.register::<SignalReceiver>();
        let env = AudioEnvironment { sample_rate: 44100.0 };
        let mut planner = Planner::new();

        let graph = receiver_graph();
        let mut plan = planner.build(&graph, &registry, &env).unwrap();

        let mut pool = ModulePool::new(64);
        let mut received_count: Option<Arc<AtomicUsize>> = None;
        for (idx, module) in plan.new_modules.drain(..) {
            if let Some(recv) = module.as_any().downcast_ref::<SignalReceiver>() {
                received_count = Some(Arc::clone(&recv.received_count));
            }
            pool.install(idx, module);
        }
        let received_count = received_count.expect("SignalReceiver not found in new_modules");

        let unknown_id = InstanceId::next();
        plan.dispatch_signal(
            unknown_id,
            ControlSignal::Float { name: "test", value: 0.0 },
            &mut pool,
        );

        assert_eq!(
            received_count.load(Ordering::SeqCst),
            0,
            "signal for unknown InstanceId must be silently dropped"
        );
    }

    #[test]
    fn send_signal_returns_err_on_full_buffer() {
        let recv_id = InstanceId::next();
        let mut engine =
            SoundEngine::new(256, 64, 64).expect("SoundEngine::new should succeed");

        for i in 0..64u64 {
            engine
                .send_signal(
                    recv_id,
                    ControlSignal::Float { name: "frequency", value: i as f64 },
                )
                .expect("push should succeed while buffer has space");
        }

        let overflow = ControlSignal::Float { name: "frequency", value: 999.0 };
        let result = engine.send_signal(recv_id, overflow);
        assert!(result.is_err(), "send_signal must return Err when the buffer is full");
    }
}
