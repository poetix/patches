use std::collections::{HashMap, HashSet};
use std::fmt;

use patches_core::{
    make_decisions, PlanDecisions,
    AudioEnvironment, BufferAllocState, InstanceId,
    Module, ModuleAllocState, ModuleGraph, NodeDecision, NodeId, NodeState, PlanError,
    PlannerState, PortConnectivity, Registry, ResolvedGraph,
};
use patches_core::parameter_map::ParameterMap;

use crate::pool::ModulePool;

/// Errors that can occur when building an [`ExecutionPlan`].
#[derive(Debug)]
pub enum BuildError {
    /// The graph contains no `AudioOut` node.
    NoAudioOut,
    /// The graph contains more than one `AudioOut` node.
    MultipleAudioOut,
    /// An internal consistency invariant was violated (indicates a bug in the builder).
    InternalError(String),
    /// The number of output ports would exceed the buffer pool capacity.
    PoolExhausted,
    /// The number of modules would exceed the module pool capacity.
    ModulePoolExhausted,
    /// Module creation failed (unknown module name or parameter validation error).
    ModuleCreationError(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::NoAudioOut => write!(f, "patch graph has no AudioOut node"),
            BuildError::MultipleAudioOut => {
                write!(f, "patch graph has more than one AudioOut node")
            }
            BuildError::InternalError(msg) => write!(f, "internal builder error: {msg}"),
            BuildError::PoolExhausted => write!(f, "buffer pool exhausted: too many output ports"),
            BuildError::ModulePoolExhausted => write!(f, "module pool exhausted: too many modules"),
            BuildError::ModuleCreationError(msg) => write!(f, "module creation failed: {msg}"),
        }
    }
}

impl std::error::Error for BuildError {}

impl From<PlanError> for BuildError {
    fn from(e: PlanError) -> Self {
        match e {
            PlanError::NoSink => BuildError::NoAudioOut,
            PlanError::MultipleSinks => BuildError::MultipleAudioOut,
            PlanError::BufferPoolExhausted => BuildError::PoolExhausted,
            PlanError::ModulePoolExhausted => BuildError::ModulePoolExhausted,
            PlanError::Internal(msg) => BuildError::InternalError(msg),
        }
    }
}

/// One entry in the execution plan: a module pool reference together with its pre-resolved
/// input and output buffer indices and pre-allocated scratch storage.
pub struct ModuleSlot {
    /// Index into the audio-thread-owned module pool (`[Option<Box<dyn Module>>]`).
    pub pool_index: usize,
    /// Inputs whose cable scale is exactly `1.0`: `(scratch_index, buf_index)`.
    ///
    /// Segregated at plan-build time so [`ExecutionPlan::tick`] can copy these
    /// without a multiply.  Unconnected ports (which read from the permanent-zero
    /// buffer slot 0 with an implicit scale of 1.0) are included here.
    pub unscaled_inputs: Vec<(usize, usize)>,
    /// Inputs whose cable scale differs from `1.0`: `(scratch_index, buf_index, scale)`.
    ///
    /// Segregated at plan-build time so the multiply-accumulate path in
    /// [`ExecutionPlan::tick`] operates on a compact, branch-free list.
    pub scaled_inputs: Vec<(usize, usize, f64)>,
    /// Indices into the [`ExecutionPlan`] buffer pool — one per output port.
    pub output_buffers: Vec<usize>,
    /// Pre-allocated scratch space for reading input values before `process`.
    pub input_scratch: Vec<f64>,
    /// Pre-allocated scratch space for `process` to write output values into.
    pub output_scratch: Vec<f64>,
}

/// A fully resolved, allocation-free execution structure produced by [`PatchBuilder::build_patch`].
///
/// Modules are **not** owned by the plan; they live in an externally-owned module pool
/// (a `[Option<Box<dyn Module>>]` slice managed by [`SoundEngine`]). Each
/// [`ModuleSlot`] holds a `pool_index` pointing into that pool.
///
/// Call [`tick`](ExecutionPlan::tick) once per sample on the audio thread,
/// passing both the module pool and the cable buffer pool, alternating `wi = 0`
/// and `wi = 1` on successive calls.
/// After each tick, retrieve the stereo output via [`last_left`](ExecutionPlan::last_left)
/// and [`last_right`](ExecutionPlan::last_right).
pub struct ExecutionPlan {
    pub slots: Vec<ModuleSlot>,
    /// Buffer pool indices that the audio thread must zero when this plan is
    /// first adopted (before the first `tick`).
    ///
    /// Contains only newly allocated and freed (recycled) indices. Stable
    /// connections whose buffer index is unchanged across a re-plan are absent,
    /// so the audio thread does not disturb their in-flight values.
    pub to_zero: Vec<usize>,
    pub audio_out_index: usize,
    /// New modules to install into the audio-thread module pool when this plan
    /// is adopted. Each entry is `(pool_index, Box<dyn Module>)`.
    ///
    /// The audio callback drains this vec into the pool on plan adoption.
    pub new_modules: Vec<(usize, Box<dyn Module>)>,
    /// Pool indices of modules removed from the graph.
    ///
    /// The audio callback calls `pool[idx].take()` for each entry, dropping the
    /// `Box<dyn Module>` and freeing the slot.
    pub tombstones: Vec<usize>,
    /// Parameter diffs to apply to surviving modules on plan adoption.
    ///
    /// Each entry is `(pool_index, diff_map)` where `diff_map` contains only the
    /// keys whose value changed since the previous build. Applied via
    /// [`ModulePool::update_parameters`] on the audio thread — infallible.
    ///
    /// New modules (in `new_modules`) do not appear here; their parameters are
    /// set during construction. Empty when no surviving module changed parameters.
    pub parameter_updates: Vec<(usize, ParameterMap)>,
    /// Connectivity diffs to apply to surviving modules on plan adoption.
    ///
    /// Each entry is `(pool_index, new_connectivity)` for a surviving module
    /// whose port connectivity changed since the previous build. Applied via
    /// [`ModulePool::set_connectivity`] on the audio thread — infallible.
    ///
    /// New modules (in `new_modules`) do not appear here; their connectivity is
    /// set during construction. Empty when wiring is unchanged.
    pub connectivity_updates: Vec<(usize, PortConnectivity)>,
    /// Pool indices of modules that receive MIDI events in the current plan.
    ///
    /// The audio callback delivers MIDI events to each of these slots via
    /// [`ModulePool::receive_midi`] once per 64-sample sub-block. Empty until
    /// modules implementing the `ReceiveMidi` trait (T-0111) are added to the graph.
    pub midi_receiver_indices: Vec<usize>,
}

impl ExecutionPlan {
    /// Process one sample across all modules in execution order.
    ///
    /// `pool` is the audio-thread-owned [`ModulePool`].
    /// `buffer_pool` is the externally-owned cable buffer pool (see [`SoundEngine`]).
    /// `wi` is the write slot index (0 or 1); the read slot is `1 - wi`.
    /// Callers must alternate between `wi = 0` and `wi = 1` on successive calls.
    ///
    /// Does not allocate.
    pub fn tick(&mut self, pool: &mut ModulePool, buffer_pool: &mut [[f64; 2]], wi: usize) {
        let ri = 1 - wi;

        for slot in self.slots.iter_mut() {
            for &(j, buf_idx) in &slot.unscaled_inputs {
                slot.input_scratch[j] = buffer_pool[buf_idx][ri];
            }
            for &(j, buf_idx, scale) in &slot.scaled_inputs {
                slot.input_scratch[j] = buffer_pool[buf_idx][ri] * scale;
            }
            pool.process(slot.pool_index, &slot.input_scratch, &mut slot.output_scratch);
            for (j, &buf_idx) in slot.output_buffers.iter().enumerate() {
                buffer_pool[buf_idx][wi] = slot.output_scratch[j];
            }
        }
    }

}

// ── Decision-phase helpers ────────────────────────────────────────────────────

type PartitionedInputs = (Vec<(usize, usize)>, Vec<(usize, usize, f64)>);

/// Partition resolved `(buffer_index, scale)` pairs into unscaled and scaled lists.
///
/// Entries with `scale == 1.0` go into the unscaled list as `(scratch_index, buf_index)`.
/// Entries with any other scale go into the scaled list as `(scratch_index, buf_index, scale)`.
/// The scratch index is the position of each entry in `resolved` (0-based).
fn partition_inputs(resolved: Vec<(usize, f64)>) -> PartitionedInputs {
    let mut unscaled = Vec::new();
    let mut scaled = Vec::new();
    for (j, (buf_idx, scale)) in resolved.into_iter().enumerate() {
        if scale == 1.0 {
            unscaled.push((j, buf_idx));
        } else {
            scaled.push((j, buf_idx, scale));
        }
    }
    (unscaled, scaled)
}

// ── PatchBuilder ──────────────────────────────────────────────────────────────

/// Produces [`ExecutionPlan`]s from [`ModuleGraph`]s, diffing against the
/// previous [`PlannerState`] to achieve stable buffer and module-pool allocation
/// across successive builds.
///
/// `PatchBuilder` captures the pool capacity constraints and delegates each
/// logical build phase to a focused helper method. Construct one with
/// [`new`](Self::new), then call [`build_patch`](Self::build_patch).
pub struct PatchBuilder {
    /// Buffer pool slot capacity; must match the [`SoundEngine`]'s pool so that
    /// [`BuildError::PoolExhausted`] is detected at plan-build time.
    pub pool_capacity: usize,
    /// Module pool slot capacity; must match the [`SoundEngine`]'s pool so that
    /// [`BuildError::ModulePoolExhausted`] is detected at plan-build time.
    pub module_pool_capacity: usize,
}

impl PatchBuilder {
    pub fn new(pool_capacity: usize, module_pool_capacity: usize) -> Self {
        Self { pool_capacity, module_pool_capacity }
    }

    /// Build an [`ExecutionPlan`] from `graph`, diffing against `prev_state`.
    ///
    /// Returns the new plan and the updated [`PlannerState`] to pass into the
    /// next call. Pass [`PlannerState::empty`] on the first build.
    pub fn build_patch(
        &self,
        graph: &ModuleGraph,
        registry: &Registry,
        env: &AudioEnvironment,
        prev_state: &PlannerState,
    ) -> Result<(ExecutionPlan, PlannerState), BuildError> {
        // ── Decision phase ───────────────────────────────────────────────────
        let PlanDecisions { index, order, audio_out_index, buf_alloc, decisions } =
            make_decisions(graph, prev_state, self.pool_capacity).map_err(BuildError::from)?;

        // ── Action phase ─────────────────────────────────────────────────────

        // Step A – mint InstanceIds for Install nodes and instantiate fresh modules.
        let mut instance_ids: HashMap<NodeId, InstanceId> =
            HashMap::with_capacity(decisions.len());
        let mut fresh_modules: HashMap<NodeId, Box<dyn Module>> =
            HashMap::with_capacity(decisions.len());

        for (id, decision) in &decisions {
            match decision {
                NodeDecision::Install { module_name, shape, params } => {
                    let new_id = InstanceId::next();
                    let m = registry
                        .create(module_name, env, shape, params, new_id)
                        .map_err(|e| BuildError::ModuleCreationError(e.to_string()))?;
                    instance_ids.insert(id.clone(), new_id);
                    fresh_modules.insert(id.clone(), m);
                }
                NodeDecision::Update { instance_id, .. } => {
                    instance_ids.insert(id.clone(), *instance_id);
                }
            }
        }

        // Step B – assign stable module pool slots.
        let new_ids: HashSet<InstanceId> = instance_ids.values().copied().collect();
        let module_diff = prev_state
            .module_alloc
            .diff(&new_ids, self.module_pool_capacity)
            .map_err(BuildError::from)?;

        // Build resolved graph: extend index with input-buffer map.
        let resolved = ResolvedGraph::build(&index, &buf_alloc.output_buf)?;

        // Step C – assemble ModuleSlots, NodeStates, and collect diff vectors.
        let mut slots: Vec<ModuleSlot> = Vec::with_capacity(order.len());
        let mut new_modules: Vec<(usize, Box<dyn Module>)> = Vec::new();
        let mut parameter_updates: Vec<(usize, ParameterMap)> = Vec::new();
        let mut connectivity_updates: Vec<(usize, PortConnectivity)> = Vec::new();
        let mut node_states: HashMap<NodeId, NodeState> = HashMap::with_capacity(order.len());

        for (id, decision) in decisions {
            let node = index.get_node(&id).ok_or_else(|| {
                BuildError::InternalError(format!("node {id:?} missing from graph"))
            })?;
            let desc = &node.module_descriptor;
            let instance_id = instance_ids[&id];
            let pool_index = *module_diff.slot_map.get(&instance_id).ok_or_else(|| {
                BuildError::InternalError(format!(
                    "instance {instance_id:?} missing from module_diff slot_map"
                ))
            })?;

            let resolved_inputs = resolved.resolve_input_buffers(desc, &id);
            let (unscaled_inputs, scaled_inputs) = partition_inputs(resolved_inputs);

            let output_buffers: Vec<usize> = desc
                .outputs
                .iter()
                .enumerate()
                .map(|(port_idx, _)| {
                    buf_alloc
                        .output_buf
                        .get(&(id.clone(), port_idx))
                        .copied()
                        .ok_or_else(|| {
                            BuildError::InternalError(format!(
                                "buffer for ({id:?}, {port_idx}) not found"
                            ))
                        })
                })
                .collect::<Result<_, _>>()?;

            let n_in = desc.inputs.len();
            let n_out = desc.outputs.len();

            let connectivity = match &decision {
                NodeDecision::Install { .. } => {
                    let c = index.compute_connectivity(desc, &id);
                    let mut fresh = fresh_modules.remove(&id).ok_or_else(|| {
                        BuildError::InternalError(format!(
                            "fresh module for install node {id:?} is missing"
                        ))
                    })?;
                    fresh.set_connectivity(c.clone());
                    new_modules.push((pool_index, fresh));
                    c
                }
                NodeDecision::Update { param_diff, connectivity_changed, .. } => {
                    if !param_diff.is_empty() {
                        parameter_updates.push((pool_index, param_diff.clone()));
                    }
                    if *connectivity_changed {
                        let c = index.compute_connectivity(desc, &id);
                        connectivity_updates.push((pool_index, c.clone()));
                        c
                    } else {
                        prev_state.nodes[&id].connectivity.clone()
                    }
                }
            };

            node_states.insert(
                id.clone(),
                NodeState {
                    module_name: desc.module_name,
                    instance_id,
                    parameter_map: node.parameter_map.clone(),
                    shape: desc.shape.clone(),
                    connectivity,
                },
            );

            slots.push(ModuleSlot {
                pool_index,
                unscaled_inputs,
                scaled_inputs,
                output_buffers,
                input_scratch: vec![0.0; n_in],
                output_scratch: vec![0.0; n_out],
            });
        }

        let tombstones = module_diff.tombstoned;

        Ok((
            ExecutionPlan {
                slots,
                to_zero: buf_alloc.to_zero,
                audio_out_index,
                new_modules,
                tombstones,
                parameter_updates,
                connectivity_updates,
                midi_receiver_indices: Vec::new(),
            },
            PlannerState {
                nodes: node_states,
                buffer_alloc: BufferAllocState {
                    output_buf: buf_alloc.output_buf,
                    freelist: buf_alloc.freelist,
                    next_hwm: buf_alloc.next_hwm,
                },
                module_alloc: ModuleAllocState {
                    pool_map: module_diff.slot_map,
                    freelist: module_diff.freelist,
                    next_hwm: module_diff.next_hwm,
                },
            },
        ))
    }

}

/// Convenience wrapper around [`PatchBuilder::build_patch`].
///
/// Constructs a temporary [`PatchBuilder`] with the given capacities and
/// delegates to [`PatchBuilder::build_patch`]. Prefer constructing a
/// [`PatchBuilder`] directly when the same capacities are reused across calls.
pub fn build_patch(
    graph: &ModuleGraph,
    registry: &Registry,
    env: &AudioEnvironment,
    prev_state: &PlannerState,
    pool_capacity: usize,
    module_pool_capacity: usize,
) -> Result<(ExecutionPlan, PlannerState), BuildError> {
    PatchBuilder::new(pool_capacity, module_pool_capacity)
        .build_patch(graph, registry, env, prev_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{AudioEnvironment, InstanceId, Module, ModuleShape, NodeId, PortRef};
    use patches_core::parameter_map::{ParameterMap, ParameterValue};
    use patches_modules::{AudioOut, Oscillator, Sum};

    fn p(name: &'static str) -> PortRef {
        PortRef { name, index: 0 }
    }

    fn pool_index_for(state: &PlannerState, node_id: &NodeId) -> usize {
        let ns = &state.nodes[node_id];
        state.module_alloc.pool_map[&ns.instance_id]
    }

    fn sine_to_audio_out_graph() -> ModuleGraph {
        let mut graph = ModuleGraph::new();
        let sine_desc = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
        let out_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
        let mut sine_params = ParameterMap::new();
        sine_params.insert("frequency".to_string(), ParameterValue::Float(440.0));
        graph.add_module("a_sine", sine_desc, &sine_params).unwrap();
        graph.add_module("b_out", out_desc, &ParameterMap::new()).unwrap();
        graph
            .connect(&NodeId::from("a_sine"), p("sine"), &NodeId::from("b_out"), p("left"), 1.0)
            .unwrap();
        graph
            .connect(&NodeId::from("a_sine"), p("sine"), &NodeId::from("b_out"), p("right"), 1.0)
            .unwrap();
        graph
    }

    fn make_buffer_pool(capacity: usize) -> Vec<[f64; 2]> {
        vec![[0.0; 2]; capacity]
    }

    fn default_registry() -> Registry {
        patches_modules::default_registry()
    }

    fn default_env() -> AudioEnvironment {
        AudioEnvironment { sample_rate: 44100.0 }
    }

    fn default_builder() -> PatchBuilder {
        PatchBuilder::new(256, 256)
    }

    /// Build a plan from scratch (no prior state) and install its modules.
    fn default_build(graph: &ModuleGraph) -> (ExecutionPlan, PlannerState, ModulePool) {
        let registry = default_registry();
        let env = default_env();
        let (mut plan, state) = default_builder()
            .build_patch(graph, &registry, &env, &PlannerState::empty())
            .expect("build should succeed");
        let mut module_pool = ModulePool::new(256);
        for (idx, m) in plan.new_modules.drain(..) {
            module_pool.install(idx, m);
        }
        (plan, state, module_pool)
    }

    #[test]
    fn builds_minimal_plan_with_correct_order() {
        let graph = sine_to_audio_out_graph();
        let (plan, _, _) = default_build(&graph);

        let audio_out_idx = plan.audio_out_index;
        let sine_idx = plan
            .slots
            .iter()
            .position(|s| s.pool_index != plan.slots[audio_out_idx].pool_index)
            .expect("sine slot not found");

        assert!(sine_idx < audio_out_idx, "sine must execute before AudioOut");
    }

    #[test]
    fn fanout_buffer_shared_between_both_inputs() {
        let graph = sine_to_audio_out_graph();
        let (plan, _, _) = default_build(&graph);

        let audio_out_idx = plan.audio_out_index;
        let sine_idx = plan
            .slots
            .iter()
            .position(|s| s.pool_index != plan.slots[audio_out_idx].pool_index)
            .unwrap();

        let sine_out_buf = plan.slots[sine_idx].output_buffers[0];
        // Both inputs are scale=1.0 fanouts; find them in unscaled_inputs by scratch index.
        let ao = &plan.slots[audio_out_idx];
        let left_buf = ao.unscaled_inputs.iter().find(|&&(j, _)| j == 0).unwrap().1;
        let right_buf = ao.unscaled_inputs.iter().find(|&&(j, _)| j == 1).unwrap().1;

        assert_eq!(sine_out_buf, left_buf, "left input must use sine output buffer");
        assert_eq!(sine_out_buf, right_buf, "right input must use sine output buffer");
    }

    #[test]
    fn tick_produces_bounded_audio_output() {
        let graph = sine_to_audio_out_graph();
        let (mut plan, _, mut module_pool) = default_build(&graph);
        let mut buffer_pool = make_buffer_pool(256);

        for i in 0..1000 {
            plan.tick(&mut module_pool, &mut buffer_pool, i % 2);
        }

        assert!(module_pool.read_sink_left().abs() <= 1.0);
        assert!(module_pool.read_sink_right().abs() <= 1.0);
        assert!(module_pool.read_sink_left().abs() > 0.0);
    }

    #[test]
    fn no_audio_out_returns_error() {
        let mut graph = ModuleGraph::new();
        let sine_desc = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
        let mut p_map = ParameterMap::new();
        p_map.insert("frequency".to_string(), ParameterValue::Float(440.0));
        graph.add_module("sine", sine_desc, &p_map).unwrap();
        let registry = default_registry();
        let env = default_env();
        assert!(matches!(
            default_builder().build_patch(&graph, &registry, &env, &PlannerState::empty()),
            Err(BuildError::NoAudioOut)
        ));
    }

    #[test]
    fn multiple_audio_out_returns_error() {
        let mut graph = ModuleGraph::new();
        let sine_desc = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
        let out1_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
        let out2_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
        let mut p_map = ParameterMap::new();
        p_map.insert("frequency".to_string(), ParameterValue::Float(440.0));
        graph.add_module("sine", sine_desc, &p_map).unwrap();
        graph.add_module("out1", out1_desc, &ParameterMap::new()).unwrap();
        graph.add_module("out2", out2_desc, &ParameterMap::new()).unwrap();
        graph.connect(&NodeId::from("sine"), p("sine"), &NodeId::from("out1"), p("left"), 1.0).unwrap();
        graph.connect(&NodeId::from("sine"), p("sine"), &NodeId::from("out1"), p("right"), 1.0).unwrap();
        graph.connect(&NodeId::from("sine"), p("sine"), &NodeId::from("out2"), p("left"), 1.0).unwrap();
        graph.connect(&NodeId::from("sine"), p("sine"), &NodeId::from("out2"), p("right"), 1.0).unwrap();
        let registry = default_registry();
        let env = default_env();
        assert!(matches!(
            default_builder().build_patch(&graph, &registry, &env, &PlannerState::empty()),
            Err(BuildError::MultipleAudioOut)
        ));
    }

    #[test]
    fn input_scale_is_applied_at_tick_time() {
        let make_graph = |scale: f64| {
            let mut g = ModuleGraph::new();
            let sine_desc = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut p_map = ParameterMap::new();
            p_map.insert("frequency".to_string(), ParameterValue::Float(440.0));
            g.add_module("sine", sine_desc, &p_map).unwrap();
            g.add_module("out", out_desc, &ParameterMap::new()).unwrap();
            g.connect(&NodeId::from("sine"), p("sine"), &NodeId::from("out"), p("left"), scale).unwrap();
            g.connect(&NodeId::from("sine"), p("sine"), &NodeId::from("out"), p("right"), scale).unwrap();
            g
        };

        let graph_half = make_graph(0.5);
        let graph_full = make_graph(1.0);
        let (mut plan_half, _, mut pool_half) = default_build(&graph_half);
        let (mut plan_full, _, mut pool_full) = default_build(&graph_full);
        let mut buf_half = make_buffer_pool(256);
        let mut buf_full = make_buffer_pool(256);

        for i in 0..100 {
            plan_half.tick(&mut pool_half, &mut buf_half, i % 2);
            plan_full.tick(&mut pool_full, &mut buf_full, i % 2);
        }

        let half = pool_half.read_sink_left();
        let full = pool_full.read_sink_left();
        if full.abs() > 1e-6 {
            let ratio = half / full;
            assert!(
                (ratio - 0.5).abs() < 1e-9,
                "expected half ≈ full * 0.5, got half={half}, full={full}, ratio={ratio}"
            );
        }
    }

    // ── Acceptance criteria: stable allocation across replan ─────────────────

    #[test]
    fn stable_buffer_index_for_unchanged_module_across_replan() {
        let registry = default_registry();
        let env = default_env();
        let builder = default_builder();

        // Graph A: two sines → AudioOut
        let mut graph_a = ModuleGraph::new();
        {
            let sine_a = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let sine_b = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut pa = ParameterMap::new();
            pa.insert("frequency".to_string(), ParameterValue::Float(440.0));
            let mut pb = ParameterMap::new();
            pb.insert("frequency".to_string(), ParameterValue::Float(880.0));
            graph_a.add_module("sine_a", sine_a, &pa).unwrap();
            graph_a.add_module("sine_b", sine_b, &pb).unwrap();
            graph_a.add_module("out", out_desc, &ParameterMap::new()).unwrap();
            graph_a
                .connect(&NodeId::from("sine_a"), p("sine"), &NodeId::from("out"), p("left"), 1.0)
                .unwrap();
            graph_a
                .connect(&NodeId::from("sine_b"), p("sine"), &NodeId::from("out"), p("right"), 1.0)
                .unwrap();
        }

        let (_plan_a, state_a) =
            builder.build_patch(&graph_a, &registry, &env, &PlannerState::empty()).unwrap();

        let buf_a = state_a.buffer_alloc.output_buf[&(NodeId::from("sine_a"), 0)];

        // Graph B: only sine_a (sine_b removed)
        let mut graph_b = ModuleGraph::new();
        {
            let sine_a = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut pa = ParameterMap::new();
            pa.insert("frequency".to_string(), ParameterValue::Float(440.0));
            graph_b.add_module("sine_a", sine_a, &pa).unwrap();
            graph_b.add_module("out", out_desc, &ParameterMap::new()).unwrap();
            graph_b
                .connect(&NodeId::from("sine_a"), p("sine"), &NodeId::from("out"), p("left"), 1.0)
                .unwrap();
            graph_b
                .connect(&NodeId::from("sine_a"), p("sine"), &NodeId::from("out"), p("right"), 1.0)
                .unwrap();
        }

        let (plan_b, state_b) = builder.build_patch(&graph_b, &registry, &env, &state_a).unwrap();

        let buf_b = state_b.buffer_alloc.output_buf[&(NodeId::from("sine_a"), 0)];
        assert_eq!(buf_a, buf_b, "sine_a output buffer must be identical across re-plan");

        let freed_buf = state_a.buffer_alloc.output_buf[&(NodeId::from("sine_b"), 0)];
        assert!(
            plan_b.to_zero.contains(&freed_buf),
            "freed buffer index {freed_buf} must appear in plan_b.to_zero"
        );
    }

    #[test]
    fn freelist_recycles_indices_preventing_hwm_growth() {
        let registry = default_registry();
        let env = default_env();
        let builder = default_builder();

        let build_two = |state: &PlannerState| {
            let mut g = ModuleGraph::new();
            let s1 = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let s2 = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut p1 = ParameterMap::new();
            p1.insert("frequency".to_string(), ParameterValue::Float(440.0));
            let mut p2 = ParameterMap::new();
            p2.insert("frequency".to_string(), ParameterValue::Float(880.0));
            g.add_module("s1", s1, &p1).unwrap();
            g.add_module("s2", s2, &p2).unwrap();
            g.add_module("out", out, &ParameterMap::new()).unwrap();
            g.connect(&NodeId::from("s1"), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            g.connect(&NodeId::from("s2"), p("sine"), &NodeId::from("out"), p("right"), 1.0).unwrap();
            let (_, new_state) = builder.build_patch(&g, &registry, &env, state).unwrap();
            new_state
        };

        let build_one = |state: &PlannerState| {
            let mut g = ModuleGraph::new();
            let s = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut pm = ParameterMap::new();
            pm.insert("frequency".to_string(), ParameterValue::Float(440.0));
            g.add_module("s1", s, &pm).unwrap();
            g.add_module("out", out, &ParameterMap::new()).unwrap();
            g.connect(&NodeId::from("s1"), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            g.connect(&NodeId::from("s1"), p("sine"), &NodeId::from("out"), p("right"), 1.0).unwrap();
            let (_, new_state) = builder.build_patch(&g, &registry, &env, state).unwrap();
            new_state
        };

        let state_a = build_two(&PlannerState::empty());
        let hwm_after_first_two = state_a.buffer_alloc.next_hwm;

        let mut current_state = state_a;
        for _ in 0..20 {
            current_state = build_one(&current_state);
            current_state = build_two(&current_state);
        }

        assert_eq!(
            current_state.buffer_alloc.next_hwm,
            hwm_after_first_two,
            "hwm grew: freelist should have prevented new allocations"
        );
    }

    #[test]
    fn pool_exhausted_error_when_capacity_exceeded() {
        let mut graph = ModuleGraph::new();
        let sine_desc = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
        let out_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
        let mut pm = ParameterMap::new();
        pm.insert("frequency".to_string(), ParameterValue::Float(440.0));
        graph.add_module("sine", sine_desc, &pm).unwrap();
        graph.add_module("out", out_desc, &ParameterMap::new()).unwrap();
        graph.connect(&NodeId::from("sine"), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();
        graph.connect(&NodeId::from("sine"), p("sine"), &NodeId::from("out"), p("right"), 1.0).unwrap();
        let registry = default_registry();
        let env = default_env();
        assert!(matches!(
            PatchBuilder::new(1, 256).build_patch(&graph, &registry, &env, &PlannerState::empty()),
            Err(BuildError::PoolExhausted)
        ));
    }

    // ── Diffing acceptance tests (T-0073) ─────────────────────────────────────

    #[test]
    fn new_node_all_modules_in_new_modules() {
        let graph = sine_to_audio_out_graph();
        let registry = default_registry();
        let env = default_env();
        let (plan, _state) = default_builder()
            .build_patch(&graph, &registry, &env, &PlannerState::empty())
            .unwrap();
        // All 2 nodes are new: sine + AudioOut → both should appear in new_modules.
        assert_eq!(
            plan.new_modules.len(),
            2,
            "all nodes are new on first build"
        );
    }

    #[test]
    fn surviving_node_no_new_modules_same_instance_id() {
        let graph = sine_to_audio_out_graph();
        let registry = default_registry();
        let env = default_env();
        let builder = default_builder();
        let (_plan_a, state_a) =
            builder.build_patch(&graph, &registry, &env, &PlannerState::empty()).unwrap();
        let id_sine_a = state_a.nodes[&NodeId::from("a_sine")].instance_id;
        let id_out_a = state_a.nodes[&NodeId::from("b_out")].instance_id;

        let (plan_b, state_b) = builder.build_patch(&graph, &registry, &env, &state_a).unwrap();
        // Same graph: all nodes are surviving → no new_modules.
        assert!(plan_b.new_modules.is_empty(), "no new modules on identical rebuild");
        // InstanceIds must be stable.
        assert_eq!(state_b.nodes[&NodeId::from("a_sine")].instance_id, id_sine_a);
        assert_eq!(state_b.nodes[&NodeId::from("b_out")].instance_id, id_out_a);
    }

    #[test]
    fn removed_node_tombstone() {
        let registry = default_registry();
        let env = default_env();
        let builder = default_builder();

        // Graph with two sines.
        let mut graph_a = ModuleGraph::new();
        {
            let s1 = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let s2 = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut p1 = ParameterMap::new();
            p1.insert("frequency".to_string(), ParameterValue::Float(440.0));
            let mut p2 = ParameterMap::new();
            p2.insert("frequency".to_string(), ParameterValue::Float(880.0));
            graph_a.add_module("s1", s1, &p1).unwrap();
            graph_a.add_module("s2", s2, &p2).unwrap();
            graph_a.add_module("out", out, &ParameterMap::new()).unwrap();
            graph_a.connect(&NodeId::from("s1"), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            graph_a.connect(&NodeId::from("s2"), p("sine"), &NodeId::from("out"), p("right"), 1.0).unwrap();
        }
        let (_plan_a, state_a) =
            builder.build_patch(&graph_a, &registry, &env, &PlannerState::empty()).unwrap();
        let s2_slot = pool_index_for(&state_a, &NodeId::from("s2"));

        // Graph with only s1.
        let mut graph_b = ModuleGraph::new();
        {
            let s1 = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut p1 = ParameterMap::new();
            p1.insert("frequency".to_string(), ParameterValue::Float(440.0));
            graph_b.add_module("s1", s1, &p1).unwrap();
            graph_b.add_module("out", out, &ParameterMap::new()).unwrap();
            graph_b.connect(&NodeId::from("s1"), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            graph_b.connect(&NodeId::from("s1"), p("sine"), &NodeId::from("out"), p("right"), 1.0).unwrap();
        }
        let (plan_b, _state_b) =
            builder.build_patch(&graph_b, &registry, &env, &state_a).unwrap();

        assert!(
            plan_b.tombstones.contains(&s2_slot),
            "removed s2 pool slot must be tombstoned"
        );
    }

    #[test]
    fn type_changed_node_tombstone_and_new_module() {
        let registry = default_registry();
        let env = default_env();
        let builder = default_builder();

        // Graph A: Oscillator at "osc" (sine output).
        let mut graph_a = ModuleGraph::new();
        {
            let sine = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut pm = ParameterMap::new();
            pm.insert("frequency".to_string(), ParameterValue::Float(440.0));
            graph_a.add_module("osc", sine, &pm).unwrap();
            graph_a.add_module("out", out, &ParameterMap::new()).unwrap();
            // Oscillator has a sine output; wire it to both channels.
            graph_a.connect(&NodeId::from("osc"), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            graph_a.connect(&NodeId::from("osc"), p("sine"), &NodeId::from("out"), p("right"), 1.0).unwrap();
        }
        let (_plan_a, state_a) =
            builder.build_patch(&graph_a, &registry, &env, &PlannerState::empty()).unwrap();
        let old_osc_id = state_a.nodes[&NodeId::from("osc")].instance_id;
        let old_osc_slot = pool_index_for(&state_a, &NodeId::from("osc"));

        // Graph B: Sum (1-channel) at "osc" (type changed from Oscillator).
        let mut graph_b = ModuleGraph::new();
        {
            let sum = Sum::describe(&ModuleShape { channels: 1, length: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            graph_b.add_module("osc", sum, &ParameterMap::new()).unwrap();
            graph_b.add_module("out", out, &ParameterMap::new()).unwrap();
            graph_b.connect(&NodeId::from("osc"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            graph_b.connect(&NodeId::from("osc"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
        }
        let (plan_b, state_b) =
            builder.build_patch(&graph_b, &registry, &env, &state_a).unwrap();

        let new_osc_id = state_b.nodes[&NodeId::from("osc")].instance_id;

        // InstanceId must have changed (new module, new identity).
        assert_ne!(new_osc_id, old_osc_id, "type-changed node must receive a new InstanceId");
        // Old slot must be tombstoned.
        assert!(
            plan_b.tombstones.contains(&old_osc_slot),
            "old osc pool slot must be tombstoned on type change"
        );
        // Exactly one new module installed (the new Sum; AudioOut survives).
        assert_eq!(plan_b.new_modules.len(), 1, "only the type-changed node should be new");
    }

    // ── ModuleAllocState unit tests ───────────────────────────────────────────

    fn make_ids(n: u64) -> Vec<InstanceId> {
        (0..n).map(|_| InstanceId::next()).collect()
    }

    fn ids_set(ids: &[InstanceId]) -> HashSet<InstanceId> {
        ids.iter().copied().collect()
    }

    #[test]
    fn module_alloc_fresh_advances_hwm() {
        let state = ModuleAllocState::default();
        let ids = make_ids(3);
        let new_ids = ids_set(&ids);
        let diff = state.diff(&new_ids, 64).expect("diff should succeed");

        assert_eq!(diff.next_hwm, 3, "hwm should advance by number of new modules");
        assert_eq!(diff.slot_map.len(), 3);
        assert!(diff.tombstoned.is_empty());
        assert!(diff.freelist.is_empty());

        let mut slots: Vec<usize> = diff.slot_map.values().copied().collect();
        slots.sort_unstable();
        assert_eq!(slots, vec![0, 1, 2]);
    }

    #[test]
    fn module_alloc_stable_reuses_slots() {
        let ids = make_ids(2);
        let new_ids = ids_set(&ids);

        let state0 = ModuleAllocState::default();
        let diff0 = state0.diff(&new_ids, 64).unwrap();

        let state1 = ModuleAllocState {
            pool_map: diff0.slot_map.clone(),
            freelist: diff0.freelist,
            next_hwm: diff0.next_hwm,
        };

        let diff1 = state1.diff(&new_ids, 64).unwrap();

        for id in &ids {
            assert_eq!(
                diff0.slot_map[id], diff1.slot_map[id],
                "slot for {id:?} must be identical across re-plan"
            );
        }

        assert_eq!(diff1.next_hwm, diff0.next_hwm, "hwm must not grow");
        assert!(diff1.tombstoned.is_empty());
    }

    #[test]
    fn module_alloc_tombstone_then_recycle() {
        let ids = make_ids(2);
        let id_a = ids[0];
        let id_b = ids[1];

        let state0 = ModuleAllocState::default();
        let diff0 = state0.diff(&ids_set(&ids), 64).unwrap();
        let slot_b = diff0.slot_map[&id_b];

        let state1 = ModuleAllocState {
            pool_map: diff0.slot_map,
            freelist: diff0.freelist,
            next_hwm: diff0.next_hwm,
        };
        let diff1 = state1.diff(&ids_set(&[id_a]), 64).unwrap();

        assert!(diff1.tombstoned.contains(&slot_b));
        assert!(diff1.freelist.contains(&slot_b));
        let hwm_after_remove = diff1.next_hwm;

        let id_c = make_ids(1)[0];
        let state2 = ModuleAllocState {
            pool_map: diff1.slot_map,
            freelist: diff1.freelist,
            next_hwm: diff1.next_hwm,
        };
        let diff2 = state2.diff(&ids_set(&[id_a, id_c]), 64).unwrap();

        assert_eq!(diff2.slot_map[&id_c], slot_b, "new module must reuse the recycled slot");
        assert_eq!(diff2.next_hwm, hwm_after_remove, "hwm must not grow when recycling");
    }

    #[test]
    fn module_alloc_pool_exhausted() {
        let state = ModuleAllocState::default();
        let ids = make_ids(3);
        let result = state.diff(&ids_set(&ids), 2);
        assert!(
            matches!(result, Err(PlanError::ModulePoolExhausted)),
            "expected ModulePoolExhausted, got {result:?}"
        );
    }

    // ── Parameter diff acceptance tests (T-0074) ──────────────────────────────

    /// Parameter-only change: surviving module, one key changed.
    /// Expect `parameter_updates` is non-empty, `new_modules` is empty.
    #[test]
    fn parameter_only_change_produces_parameter_updates_no_new_modules() {
        let registry = default_registry();
        let env = default_env();
        let builder = default_builder();

        // Build initial graph with sine at 440 Hz.
        let graph_a = sine_to_audio_out_graph();
        let (_plan_a, state_a) =
            builder.build_patch(&graph_a, &registry, &env, &PlannerState::empty()).unwrap();

        // Rebuild with same topology but different frequency.
        let mut graph_b = ModuleGraph::new();
        {
            let sine_desc = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out_desc = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut sine_params = ParameterMap::new();
            sine_params.insert("frequency".to_string(), ParameterValue::Float(880.0));
            graph_b.add_module("a_sine", sine_desc, &sine_params).unwrap();
            graph_b.add_module("b_out", out_desc, &ParameterMap::new()).unwrap();
            graph_b
                .connect(&NodeId::from("a_sine"), p("sine"), &NodeId::from("b_out"), p("left"), 1.0)
                .unwrap();
            graph_b
                .connect(
                    &NodeId::from("a_sine"),
                    p("sine"),
                    &NodeId::from("b_out"),
                    p("right"),
                    1.0,
                )
                .unwrap();
        }

        let (plan_b, _state_b) =
            builder.build_patch(&graph_b, &registry, &env, &state_a).unwrap();

        assert!(plan_b.new_modules.is_empty(), "parameter-only change must not produce new_modules");
        assert!(
            !plan_b.parameter_updates.is_empty(),
            "parameter-only change must produce parameter_updates"
        );

        // The diff should contain exactly the changed key.
        let sine_slot = pool_index_for(&state_a, &NodeId::from("a_sine"));
        let update = plan_b
            .parameter_updates
            .iter()
            .find(|(idx, _)| *idx == sine_slot)
            .expect("update entry for sine must be present");
        assert!(
            matches!(update.1.get("frequency"), Some(ParameterValue::Float(f)) if (*f - 880.0).abs() < 1e-9),
            "diff must contain updated frequency"
        );
    }

    /// Unchanged parameters: surviving module with same parameters.
    /// Expect `parameter_updates` is empty.
    #[test]
    fn unchanged_parameters_produce_empty_parameter_updates() {
        let registry = default_registry();
        let env = default_env();
        let builder = default_builder();

        let graph = sine_to_audio_out_graph();
        let (_plan_a, state_a) =
            builder.build_patch(&graph, &registry, &env, &PlannerState::empty()).unwrap();
        let (plan_b, _state_b) =
            builder.build_patch(&graph, &registry, &env, &state_a).unwrap();

        assert!(
            plan_b.parameter_updates.is_empty(),
            "unchanged parameters must produce empty parameter_updates"
        );
    }

    /// Topology change (add/remove node) works correctly alongside parameter diffs.
    /// Removed module is tombstoned; surviving module with a changed parameter
    /// appears in `parameter_updates`; new module appears in `new_modules`.
    #[test]
    fn topology_change_and_parameter_diff_coexist() {
        let registry = default_registry();
        let env = default_env();
        let builder = default_builder();

        // Graph A: sine_a (440 Hz) + sine_b (880 Hz) → AudioOut.
        let mut graph_a = ModuleGraph::new();
        {
            let s_a = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let s_b = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut pa = ParameterMap::new();
            pa.insert("frequency".to_string(), ParameterValue::Float(440.0));
            let mut pb = ParameterMap::new();
            pb.insert("frequency".to_string(), ParameterValue::Float(880.0));
            graph_a.add_module("s_a", s_a, &pa).unwrap();
            graph_a.add_module("s_b", s_b, &pb).unwrap();
            graph_a.add_module("out", out, &ParameterMap::new()).unwrap();
            graph_a
                .connect(&NodeId::from("s_a"), p("sine"), &NodeId::from("out"), p("left"), 1.0)
                .unwrap();
            graph_a
                .connect(&NodeId::from("s_b"), p("sine"), &NodeId::from("out"), p("right"), 1.0)
                .unwrap();
        }
        let (_plan_a, state_a) =
            builder.build_patch(&graph_a, &registry, &env, &PlannerState::empty()).unwrap();
        let s_b_slot = pool_index_for(&state_a, &NodeId::from("s_b"));

        // Graph B: sine_a (changed to 660 Hz) + new sine_c (1000 Hz), sine_b removed.
        let mut graph_b = ModuleGraph::new();
        {
            let s_a = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let s_c = Oscillator::describe(&ModuleShape { channels: 0, length: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0, length: 0 });
            let mut pa = ParameterMap::new();
            pa.insert("frequency".to_string(), ParameterValue::Float(660.0));
            let mut pc = ParameterMap::new();
            pc.insert("frequency".to_string(), ParameterValue::Float(1000.0));
            graph_b.add_module("s_a", s_a, &pa).unwrap();
            graph_b.add_module("s_c", s_c, &pc).unwrap();
            graph_b.add_module("out", out, &ParameterMap::new()).unwrap();
            graph_b
                .connect(&NodeId::from("s_a"), p("sine"), &NodeId::from("out"), p("left"), 1.0)
                .unwrap();
            graph_b
                .connect(&NodeId::from("s_c"), p("sine"), &NodeId::from("out"), p("right"), 1.0)
                .unwrap();
        }
        let (plan_b, _state_b) =
            builder.build_patch(&graph_b, &registry, &env, &state_a).unwrap();

        // s_b was removed → tombstoned.
        assert!(plan_b.tombstones.contains(&s_b_slot), "s_b must be tombstoned");
        // s_c is new → appears in new_modules (pool_index may vary; just check count).
        // s_a is surviving with changed param → appears in parameter_updates.
        let s_a_slot = pool_index_for(&state_a, &NodeId::from("s_a"));
        let has_s_a_update = plan_b
            .parameter_updates
            .iter()
            .any(|(idx, diff)| {
                *idx == s_a_slot
                    && matches!(diff.get("frequency"), Some(ParameterValue::Float(f)) if (*f - 660.0).abs() < 1e-9)
            });
        assert!(has_s_a_update, "s_a parameter update must appear in parameter_updates");
        // s_c must not appear in parameter_updates (it is new, not surviving).
        assert_eq!(
            plan_b.new_modules.iter().filter(|(_, m)| m.descriptor().module_name == "Oscillator").count(),
            1,
            "exactly one new Oscillator (s_c) must appear in new_modules"
        );
    }

    // ── resolve_input_buffers, build_input_buffer_map, and compute_connectivity
    // tests moved to patches-core (T-0103).

    // ── partition_inputs unit tests (T-0097) ──────────────────────────────────

    #[test]
    fn partition_empty_produces_two_empty_lists() {
        let (unscaled, scaled) = partition_inputs(vec![]);
        assert!(unscaled.is_empty());
        assert!(scaled.is_empty());
    }

    #[test]
    fn partition_scale_one_goes_to_unscaled() {
        let (unscaled, scaled) = partition_inputs(vec![(5, 1.0), (7, 1.0)]);
        assert_eq!(unscaled, vec![(0, 5), (1, 7)]);
        assert!(scaled.is_empty());
    }

    #[test]
    fn partition_non_one_scale_goes_to_scaled() {
        let (unscaled, scaled) = partition_inputs(vec![(3, 0.5)]);
        assert!(unscaled.is_empty());
        assert_eq!(scaled, vec![(0, 3, 0.5)]);
    }

    #[test]
    fn partition_mixed_produces_correct_split() {
        let (unscaled, scaled) = partition_inputs(vec![(2, 1.0), (4, 0.25), (6, 1.0), (8, -1.0)]);
        assert_eq!(unscaled, vec![(0, 2), (2, 6)]);
        assert_eq!(scaled, vec![(1, 4, 0.25), (3, 8, -1.0)]);
    }

}
