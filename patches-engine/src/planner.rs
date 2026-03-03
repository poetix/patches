use patches_core::{ControlSignal, InstanceId, ModuleGraph};

use crate::builder::{build_patch, BufferAllocState, BuildError, ExecutionPlan, ModuleAllocState};
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
/// `Planner` carries [`BufferAllocState`] and [`ModuleAllocState`] forward across
/// successive [`build`](Self::build) calls so that:
/// - Cables that share a `(NodeId, output_port_index)` key across re-plans reuse
///   the same buffer pool slot.
/// - Modules that share an [`InstanceId`](patches_core::InstanceId) across re-plans
///   are assigned the same module pool slot. Surviving modules stay in the pool
///   untouched; only new modules appear in `ExecutionPlan::new_modules`, and only
///   removed modules appear in `ExecutionPlan::tombstones`.
///
/// # State preservation
///
/// Module state (e.g. oscillator phase) is preserved across re-plans because
/// surviving modules remain in the audio-thread module pool between swaps. The
/// control thread does not need access to the running plan.
pub struct Planner {
    alloc_state: BufferAllocState,
    module_alloc_state: ModuleAllocState,
    pool_capacity: usize,
    module_pool_capacity: usize,
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
            alloc_state: BufferAllocState::default(),
            module_alloc_state: ModuleAllocState::default(),
            pool_capacity,
            module_pool_capacity: DEFAULT_MODULE_POOL_CAPACITY,
        }
    }

    /// Build an [`ExecutionPlan`] from `graph`, updating internal allocation state.
    ///
    /// Surviving modules (same [`InstanceId`](patches_core::InstanceId) in both the
    /// old and new graph) reuse their module pool slot; their state is preserved by
    /// the audio-thread pool without any `prev_plan` argument.
    ///
    /// New modules are placed in `ExecutionPlan::new_modules` for the engine to
    /// install. Removed modules appear in `ExecutionPlan::tombstones` for the engine
    /// to drop.
    pub fn build(&mut self, graph: ModuleGraph) -> Result<ExecutionPlan, BuildError> {
        let (plan, new_alloc, new_module_alloc) = build_patch(
            graph,
            &self.alloc_state,
            &self.module_alloc_state,
            self.pool_capacity,
            self.module_pool_capacity,
        )?;
        self.alloc_state = new_alloc;
        self.module_alloc_state = new_module_alloc;
        Ok(plan)
    }
}

/// Coordinates patch planning (with state preservation) and audio execution.
///
/// `PatchEngine` ties together a [`Planner`] and a [`SoundEngine`].
///
/// ## Normal flow
///
/// 1. [`new`](Self::new) builds the initial plan and hands it to `SoundEngine`.
/// 2. [`start`](Self::start) opens the audio device.
/// 3. Each [`update`](Self::update) builds a new plan and pushes it to the engine
///    via [`swap_plan`](SoundEngine::swap_plan).
///
/// ## Channel-full path
///
/// If [`SoundEngine::swap_plan`] returns `Err` (channel full), `update` returns
/// [`PatchEngineError::ChannelFull`] immediately. The caller is responsible for
/// retrying with the same or an updated graph. Module state is preserved by the
/// audio-thread pool regardless of retries.
pub struct PatchEngine {
    planner: Planner,
    engine: SoundEngine,
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
    /// Builds the first plan and constructs the underlying [`SoundEngine`]
    /// with the default control period (64 samples), but does not open the
    /// audio device. Call [`start`](Self::start) to begin playback.
    pub fn new(graph: ModuleGraph) -> Result<Self, PatchEngineError> {
        Self::with_control_period(graph, DEFAULT_CONTROL_PERIOD)
    }

    /// Create a `PatchEngine` from an initial graph with a specific control period.
    ///
    /// `control_period` is the number of audio samples between control-rate
    /// ticks (signal dispatch). Must be greater than zero.
    pub fn with_control_period(
        graph: ModuleGraph,
        control_period: usize,
    ) -> Result<Self, PatchEngineError> {
        let mut planner = Planner::with_capacity(DEFAULT_POOL_CAPACITY);
        let plan = planner.build(graph)?;
        let engine = SoundEngine::new(
            plan,
            DEFAULT_POOL_CAPACITY,
            DEFAULT_MODULE_POOL_CAPACITY,
            control_period,
        )?;
        Ok(Self {
            planner,
            engine,
        })
    }

    /// Open the audio device and begin processing.
    ///
    /// Subsequent calls are no-ops if the engine is already running.
    pub fn start(&mut self) -> Result<(), PatchEngineError> {
        self.engine.start().map_err(PatchEngineError::Engine)
    }

    /// Apply an updated graph.
    ///
    /// Builds a new [`ExecutionPlan`] from `graph` and pushes it to the
    /// [`SoundEngine`] via [`swap_plan`](SoundEngine::swap_plan). Surviving
    /// modules retain their state via the audio-thread pool.
    ///
    /// Returns [`PatchEngineError::ChannelFull`] if the engine's channel is
    /// already occupied. The caller is responsible for retrying with the same
    /// or an updated graph.
    pub fn update(&mut self, graph: ModuleGraph) -> Result<(), PatchEngineError> {
        let new_plan = self.planner.build(graph)?;
        match self.engine.swap_plan(new_plan) {
            Ok(()) => Ok(()),
            Err(_returned_plan) => Err(PatchEngineError::ChannelFull),
        }
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
        AudioEnvironment, ControlSignal, InstanceId, Module, ModuleDescriptor, ModuleGraph, NodeId,
        PortDescriptor, PortRef,
    };
    use patches_modules::{AudioOut, SineOscillator};

    use super::*;
    use crate::pool::ModulePool;

    fn p(name: &'static str) -> PortRef {
        PortRef { name, index: 0 }
    }

    fn simple_graph(freq: f64) -> ModuleGraph {
        let mut graph = ModuleGraph::new();
        let osc = NodeId::from("osc");
        let out = NodeId::from("out");
        graph.add_module(osc.clone(), Box::new(SineOscillator::new(freq))).unwrap();
        graph.add_module(out.clone(), Box::new(AudioOut::new())).unwrap();
        graph.connect(&osc, p("out"), &out, p("left"), 1.0).unwrap();
        graph.connect(&osc, p("out"), &out, p("right"), 1.0).unwrap();
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
    }

    fn counter_graph(counter: Counter) -> ModuleGraph {
        let mut graph = ModuleGraph::new();
        let c = NodeId::from("counter");
        let out = NodeId::from("out");
        graph.add_module(c.clone(), Box::new(counter)).unwrap();
        graph.add_module(out.clone(), Box::new(AudioOut::new())).unwrap();
        graph.connect(&c, p("out"), &out, p("left"), 1.0).unwrap();
        graph.connect(&c, p("out"), &out, p("right"), 1.0).unwrap();
        graph
    }

    fn make_buffer_pool(capacity: usize) -> Vec<[f64; 2]> {
        vec![[0.0; 2]; capacity]
    }

    /// Install a plan's new_modules into `pool` and process tombstones,
    /// simulating what SoundEngine does on plan adoption. Optionally initialises
    /// new modules with `env`.
    fn adopt_plan(plan: &mut ExecutionPlan, pool: &mut ModulePool, env: Option<&AudioEnvironment>) {
        for &idx in &plan.tombstones {
            pool.tombstone(idx);
        }
        for (idx, mut m) in plan.new_modules.drain(..) {
            if let Some(e) = env {
                m.initialise(e);
            }
            pool.install(idx, m);
        }
    }

    /// Return the output buffer index for the Counter slot in `plan`.
    ///
    /// Counter is the non-AudioOut slot. Its output buffer carries the count value,
    /// which lets us verify state without downcasting through `ModulePool`.
    fn counter_output_buf(plan: &ExecutionPlan) -> usize {
        let ao_pool = plan.slots[plan.audio_out_index].pool_index;
        let counter_slot = plan.slots.iter().find(|s| s.pool_index != ao_pool).unwrap();
        counter_slot.output_buffers[0]
    }

    #[test]
    fn planner_reuses_module_instance_across_rebuild() {
        let mut planner = Planner::new();
        let mut pool = ModulePool::new(64);
        let env = AudioEnvironment { sample_rate: 44100.0 };

        let counter_a = Counter::new();
        let counter_id = counter_a.instance_id();
        let mut plan_a = planner.build(counter_graph(counter_a)).unwrap();
        adopt_plan(&mut plan_a, &mut pool, Some(&env));

        let mut buffer_pool = make_buffer_pool(256);
        for i in 0..5 {
            plan_a.tick(&mut pool, &mut buffer_pool, i % 2);
        }
        // After 5 ticks (wi sequence 0,1,0,1,0), Counter wrote 5.0 into buffer slot [0].

        // Build graph_b with same InstanceId — counter is a surviving module.
        let placeholder = Counter::with_id(counter_id);
        let graph_b = counter_graph(placeholder);
        let mut plan_b = planner.build(graph_b).unwrap();

        // Counter must NOT appear in new_modules (it is surviving).
        assert!(
            plan_b.new_modules.iter().all(|(_, m)| m.as_any().downcast_ref::<Counter>().is_none()),
            "surviving Counter must not appear in new_modules"
        );

        adopt_plan(&mut plan_b, &mut pool, Some(&env));

        // wi=1 continues the alternating sequence (plan_a had 5 ticks, last wi=0).
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
        let mut planner = Planner::new();
        let mut pool = ModulePool::new(64);

        let counter = Counter::new();
        let mut plan = planner.build(counter_graph(counter)).unwrap();
        adopt_plan(&mut plan, &mut pool, None);

        let mut buffer_pool = make_buffer_pool(256);
        plan.tick(&mut pool, &mut buffer_pool, 0);

        // Counter wrote its count (1) into the wi=0 buffer slot.
        let buf = counter_output_buf(&plan);
        assert_eq!(buffer_pool[buf][0], 1.0, "fresh plan: count starts at 0, ticked once → 1");
    }

    #[test]
    fn planner_build_succeeds_for_valid_graph() {
        let mut planner = Planner::new();
        assert!(planner.build(simple_graph(440.0)).is_ok());
    }

    #[test]
    fn planner_build_fails_for_empty_graph() {
        let mut planner = Planner::new();
        assert!(planner.build(ModuleGraph::new()).is_err());
    }

    // ── Signal dispatch tests (T-0038) ────────────────────────────────────────

    /// Records how many signals it has received via an `Arc<AtomicUsize>` shared
    /// with the test. Using an atomic avoids any pool-access gymnastics while
    /// keeping the counter observable from outside.
    struct SignalReceiver {
        instance_id: InstanceId,
        descriptor: ModuleDescriptor,
        received_count: Arc<AtomicUsize>,
    }

    impl SignalReceiver {
        fn new() -> (Self, Arc<AtomicUsize>) {
            let received_count = Arc::new(AtomicUsize::new(0));
            let receiver = Self {
                instance_id: InstanceId::next(),
                descriptor: ModuleDescriptor {
                    inputs: vec![],
                    outputs: vec![PortDescriptor { name: "out", index: 0 }],
                },
                received_count: Arc::clone(&received_count),
            };
            (receiver, received_count)
        }
    }

    impl Module for SignalReceiver {
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

    fn receiver_graph() -> (ModuleGraph, InstanceId, Arc<AtomicUsize>) {
        let (receiver, received_count) = SignalReceiver::new();
        let id = receiver.instance_id();
        let mut graph = ModuleGraph::new();
        graph.add_module(NodeId::from("recv"), Box::new(receiver)).unwrap();
        graph.add_module(NodeId::from("out"), Box::new(AudioOut::new())).unwrap();
        graph
            .connect(&NodeId::from("recv"), p("out"), &NodeId::from("out"), p("left"), 1.0)
            .unwrap();
        graph
            .connect(&NodeId::from("recv"), p("out"), &NodeId::from("out"), p("right"), 1.0)
            .unwrap();
        (graph, id, received_count)
    }

    #[test]
    fn signal_delivered_at_control_tick_not_before() {
        let mut planner = Planner::new();
        let (graph, recv_id, received_count) = receiver_graph();
        let mut plan = planner.build(graph).unwrap();

        let mut pool = ModulePool::new(64);
        adopt_plan(&mut plan, &mut pool, None);

        let mut buffer_pool = make_buffer_pool(256);

        let (mut producer, mut consumer) =
            rtrb::RingBuffer::<(InstanceId, ControlSignal)>::new(64);
        producer
            .push((recv_id, ControlSignal::Float { name: "freq", value: 440.0 }))
            .unwrap();

        for i in 0..3usize {
            plan.tick(&mut pool, &mut buffer_pool, i % 2);
        }

        assert_eq!(
            received_count.load(Ordering::SeqCst),
            0,
            "signal must not arrive before the control tick"
        );

        while let Ok((id, signal)) = consumer.pop() {
            plan.dispatch_signal(id, signal, &mut pool);
        }

        assert_eq!(
            received_count.load(Ordering::SeqCst),
            1,
            "signal must arrive after the control tick"
        );
    }

    #[test]
    fn signal_for_unknown_id_is_silently_dropped() {
        let mut planner = Planner::new();
        let (graph, _recv_id, received_count) = receiver_graph();
        let mut plan = planner.build(graph).unwrap();
        let mut pool = ModulePool::new(64);
        adopt_plan(&mut plan, &mut pool, None);

        let unknown_id = InstanceId::next();
        let signal = ControlSignal::Float { name: "freq", value: 440.0 };

        plan.dispatch_signal(unknown_id, signal, &mut pool);
        assert_eq!(
            received_count.load(Ordering::SeqCst),
            0,
            "signal for unknown InstanceId must be silently dropped"
        );
    }

    #[test]
    fn send_signal_returns_err_on_full_buffer() {
        let mut planner = Planner::new();
        let (graph, recv_id, _) = receiver_graph();
        let plan = planner.build(graph).unwrap();

        let mut engine =
            SoundEngine::new(plan, 256, 64, 64).expect("SoundEngine::new should succeed");

        for i in 0..64u64 {
            engine
                .send_signal(recv_id, ControlSignal::Float { name: "freq", value: i as f64 })
                .expect("push should succeed while buffer has space");
        }

        let overflow = ControlSignal::Float { name: "freq", value: 999.0 };
        let result = engine.send_signal(recv_id, overflow);
        assert!(result.is_err(), "send_signal must return Err when the buffer is full");
    }
}
