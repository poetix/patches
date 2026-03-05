use std::collections::{HashMap, HashSet};
use std::fmt;

use patches_core::{
    AudioEnvironment, ControlSignal, InstanceId, Module, ModuleGraph, NodeId, Registry,
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

/// Stable buffer index allocation state threaded across successive [`PatchBuilder::build_patch`] calls.
///
/// `BufferAllocState` allows cables that share a `(NodeId, output_port_index)` key
/// across re-plans to reuse the same pool slot, so the audio thread reads/writes the
/// same memory before and after a plan swap.
///
/// The `Default` implementation starts the high-water mark at `1`, reserving slot `0`
/// as the permanent-zero slot.
pub struct BufferAllocState {
    /// Maps `(NodeId, output_port_index)` to a stable buffer pool index.
    pub output_buf: HashMap<(NodeId, usize), usize>,
    /// Recycled buffer indices available for reuse (LIFO via [`Vec::pop`]).
    pub freelist: Vec<usize>,
    /// High-water mark: the next index to allocate when the freelist is empty.
    /// Starts at `1` so that index `0` remains the permanent-zero slot.
    pub next_hwm: usize,
}

impl Default for BufferAllocState {
    fn default() -> Self {
        Self {
            output_buf: HashMap::new(),
            freelist: Vec::new(),
            next_hwm: 1,
        }
    }
}

/// Stable module slot allocation state threaded across successive [`PatchBuilder::build_patch`] calls.
///
/// `ModuleAllocState` is the control-thread mirror of the audio thread's module pool,
/// analogous to [`BufferAllocState`] for the buffer pool. It tracks which pool slot each
/// [`InstanceId`] occupies so that surviving modules reuse their slots across re-plans.
///
/// The `Default` implementation starts the high-water mark at `0` (no permanent-zero slot
/// is needed for modules).
#[derive(Default)]
pub struct ModuleAllocState {
    /// Maps [`InstanceId`] to the pool slot index currently holding that module.
    pub pool_map: HashMap<InstanceId, usize>,
    /// Recycled slot indices available for reuse (LIFO via [`Vec::pop`]).
    pub freelist: Vec<usize>,
    /// High-water mark: the next index to allocate when the freelist is empty.
    /// Starts at `0`.
    pub next_hwm: usize,
}

/// Result of [`ModuleAllocState::diff`]: the new pool map and freelist after applying
/// the module set for the next graph.
#[derive(Debug)]
pub struct ModuleAllocDiff {
    /// Slot index for each [`InstanceId`] in the new graph (surviving + newly allocated).
    pub slot_map: HashMap<InstanceId, usize>,
    /// Updated freelist (surviving freelisted indices + newly tombstoned slots).
    pub freelist: Vec<usize>,
    /// New high-water mark.
    pub next_hwm: usize,
    /// Slot indices that were tombstoned (freed) by this diff.
    pub tombstoned: Vec<usize>,
}

impl ModuleAllocState {
    /// Compute allocation changes given the set of [`InstanceId`]s for the incoming graph.
    ///
    /// - **Surviving** entries: already in `pool_map` → reuse their existing slot index.
    /// - **New** entries: not in `pool_map` → acquired from `freelist` (LIFO) or `next_hwm`.
    ///   Returns [`BuildError::ModulePoolExhausted`] if the index would reach `capacity`.
    /// - **Tombstoned** entries: in `pool_map` but not in `new_ids` → slot returned to freelist.
    pub fn diff(
        &self,
        new_ids: &HashSet<InstanceId>,
        capacity: usize,
    ) -> Result<ModuleAllocDiff, BuildError> {
        let mut slot_map: HashMap<InstanceId, usize> = HashMap::new();
        let mut freelist: Vec<usize> = self.freelist.clone();
        let mut next_hwm: usize = self.next_hwm;
        let mut tombstoned: Vec<usize> = Vec::new();

        // Tombstone: entries in the old pool_map that are not in the new set.
        for (&id, &slot) in &self.pool_map {
            if !new_ids.contains(&id) {
                freelist.push(slot);
                tombstoned.push(slot);
            }
        }

        // Allocate: surviving entries reuse their slot; new entries get a fresh one.
        for &id in new_ids {
            if let Some(&existing) = self.pool_map.get(&id) {
                slot_map.insert(id, existing);
            } else {
                let idx = if let Some(recycled) = freelist.pop() {
                    recycled
                } else {
                    let idx = next_hwm;
                    next_hwm += 1;
                    idx
                };
                if idx >= capacity {
                    return Err(BuildError::ModulePoolExhausted);
                }
                slot_map.insert(id, idx);
            }
        }

        Ok(ModuleAllocDiff {
            slot_map,
            freelist,
            next_hwm,
            tombstoned,
        })
    }
}

/// Per-node identity and parameter state carried across successive builds.
pub struct NodeState {
    /// The module type name (from `ModuleDescriptor::module_name`).
    pub module_name: &'static str,
    /// Stable identity assigned by the planner when this node first appeared.
    pub instance_id: InstanceId,
    /// The module pool slot assigned to this node.
    pub pool_index: usize,
    /// The parameter map applied to this node during the last build.
    pub parameter_map: ParameterMap,
}

/// Planning state threaded across successive [`PatchBuilder::build_patch`] calls.
///
/// `PlannerState` records node identity, buffer allocation, and module slot
/// allocation. Passing the previous build's state into the next call enables
/// graph diffing: surviving nodes reuse their `InstanceId` and pool slot;
/// only added and type-changed nodes trigger module instantiation.
pub struct PlannerState {
    /// Maps each [`NodeId`] to its last-known identity and parameters.
    pub nodes: HashMap<NodeId, NodeState>,
    /// Stable buffer index allocation carried across builds.
    pub buffer_alloc: BufferAllocState,
    /// Stable module slot allocation carried across builds.
    pub module_alloc: ModuleAllocState,
}

impl PlannerState {
    /// Return an empty state for the first build.
    ///
    /// Using an empty state causes every node in the graph to be treated as
    /// new: each receives a fresh [`InstanceId`] and a new module is
    /// instantiated via the registry.
    pub fn empty() -> Self {
        Self {
            nodes: HashMap::new(),
            buffer_alloc: BufferAllocState::default(),
            module_alloc: ModuleAllocState::default(),
        }
    }
}

/// One entry in the execution plan: a module pool reference together with its pre-resolved
/// input and output buffer indices and pre-allocated scratch storage.
pub struct ModuleSlot {
    /// Index into the audio-thread-owned module pool (`[Option<Box<dyn Module>>]`).
    pub pool_index: usize,
    /// Indices into the [`ExecutionPlan`] buffer pool — one per input port.
    pub input_buffers: Vec<usize>,
    /// Scaling factors applied to each input at read-time — one per input port.
    ///
    /// `1.0` for unconnected inputs; the edge's scale for connected inputs.
    pub input_scales: Vec<f64>,
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
    /// Sorted array mapping `InstanceId` to pool index, used for O(log M)
    /// signal dispatch at control-rate ticks.
    ///
    /// Built at plan construction time (off the audio thread) so that the
    /// audio callback can binary-search without allocating.
    signal_dispatch: Box<[(InstanceId, usize)]>,
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
            for (j, &buf_idx) in slot.input_buffers.iter().enumerate() {
                slot.input_scratch[j] = buffer_pool[buf_idx][ri] * slot.input_scales[j];
            }
            pool.process(slot.pool_index, &slot.input_scratch, &mut slot.output_scratch);
            for (j, &buf_idx) in slot.output_buffers.iter().enumerate() {
                buffer_pool[buf_idx][wi] = slot.output_scratch[j];
            }
        }
    }

    /// Deliver `signal` to the module identified by `id`, if it is present in this plan.
    ///
    /// Performs a binary search on `signal_dispatch` (O(log M)) and calls
    /// [`ModulePool::receive_signal`] on the resolved pool slot. Does nothing if
    /// `id` is not in this plan.
    pub fn dispatch_signal(&self, id: InstanceId, signal: ControlSignal, pool: &mut ModulePool) {
        if let Ok(idx) = self.signal_dispatch.binary_search_by_key(&id, |(k, _)| *k) {
            let pool_idx = self.signal_dispatch[idx].1;
            pool.receive_signal(pool_idx, signal);
        }
    }
}

// ── Intermediate build results ────────────────────────────────────────────────

/// Maps each [`NodeId`] to its assigned [`InstanceId`] and, for new or
/// type-changed nodes, the freshly-instantiated module (absent for survivors).
type NodeIdentityMap = HashMap<NodeId, (InstanceId, Option<Box<dyn Module>>)>;

/// Result of Phase 4 (buffer allocation), passed into Phase 6 (slot building).
struct BufferAllocation {
    output_buf: HashMap<(NodeId, usize), usize>,
    to_zero: Vec<usize>,
    freelist: Vec<usize>,
    next_hwm: usize,
}

/// Result of Phase 6 (slot building).
struct SlotBuildResult {
    slots: Vec<ModuleSlot>,
    new_modules: Vec<(usize, Box<dyn Module>)>,
    parameter_updates: Vec<(usize, ParameterMap)>,
    node_states: HashMap<NodeId, NodeState>,
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
        let node_ids = graph.node_ids();

        // Phase 1 – assign stable InstanceIds; instantiate new/type-changed modules.
        let mut node_identity =
            self.assign_instance_ids(graph, &node_ids, prev_state, registry, env)?;

        // Phase 2 – locate the single sink node.
        let sink = Self::find_sink(graph, &node_ids)?;

        // Phase 3 – sort into execution order and record the sink's position.
        let (order, audio_out_index) = Self::compute_order(&node_ids, &sink)?;

        // Phase 4 – assign stable cable buffer indices.
        let buf_alloc = self.allocate_buffers(graph, &order, &prev_state.buffer_alloc)?;

        // Phase 5 – assign stable module pool slots.
        let new_ids: HashSet<InstanceId> =
            node_ids.iter().map(|id| node_identity[id].0).collect();
        let module_diff = prev_state.module_alloc.diff(&new_ids, self.module_pool_capacity)?;

        // Phase 6 – build ModuleSlots, collect new modules, record node states.
        let SlotBuildResult { slots, new_modules, parameter_updates, node_states: new_node_states } = Self::build_slots(
            graph,
            &order,
            &mut node_identity,
            &buf_alloc.output_buf,
            &module_diff,
            prev_state,
        )?;

        let tombstones = module_diff.tombstoned;

        // Build the signal_dispatch sorted array: (InstanceId → pool_index).
        // Sorted by InstanceId so the audio callback can binary-search in O(log M).
        let mut dispatch: Vec<(InstanceId, usize)> = slots
            .iter()
            .zip(order.iter())
            .map(|(slot, id)| (node_identity[id].0, slot.pool_index))
            .collect();
        dispatch.sort_unstable_by_key(|(id, _)| *id);
        let signal_dispatch = dispatch.into_boxed_slice();

        Ok((
            ExecutionPlan {
                slots,
                to_zero: buf_alloc.to_zero,
                audio_out_index,
                signal_dispatch,
                new_modules,
                tombstones,
                parameter_updates,
            },
            PlannerState {
                nodes: new_node_states,
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

    /// Phase 1: Walk every node in the graph, decide whether it is surviving or
    /// new/type-changed, and (where needed) instantiate a fresh module via `registry`.
    ///
    /// Returns a [`NodeIdentityMap`] from `NodeId` to `(InstanceId, Option<fresh module>)`.
    fn assign_instance_ids(
        &self,
        graph: &ModuleGraph,
        node_ids: &[NodeId],
        prev_state: &PlannerState,
        registry: &Registry,
        env: &AudioEnvironment,
    ) -> Result<NodeIdentityMap, BuildError> {
        let mut node_identity = HashMap::with_capacity(node_ids.len());

        for id in node_ids {
            let node = graph.get_node(id).ok_or_else(|| {
                BuildError::InternalError(format!("node {id:?} missing from graph"))
            })?;
            let module_name = node.module_descriptor.module_name;

            let (instance_id, fresh) = if let Some(prev) = prev_state.nodes.get(id) {
                if prev.module_name == module_name {
                    // Surviving: same NodeId, same module type — reuse identity.
                    (prev.instance_id, None)
                } else {
                    // Type-changed: same NodeId, different type — instantiate new.
                    let m = registry
                        .create(module_name, env, &node.module_descriptor.shape, &node.parameter_map)
                        .map_err(|e| BuildError::ModuleCreationError(e.to_string()))?;
                    let id = m.instance_id();
                    (id, Some(m))
                }
            } else {
                // New node: instantiate.
                let m = registry
                    .create(module_name, env, &node.module_descriptor.shape, &node.parameter_map)
                    .map_err(|e| BuildError::ModuleCreationError(e.to_string()))?;
                let id = m.instance_id();
                (id, Some(m))
            };

            node_identity.insert(id.clone(), (instance_id, fresh));
        }

        Ok(node_identity)
    }

    /// Phase 2: Find exactly one sink node (`is_sink == true`) in the graph.
    ///
    /// Returns [`BuildError::NoAudioOut`] or [`BuildError::MultipleAudioOut`] if
    /// the sink count is not exactly one.
    fn find_sink(graph: &ModuleGraph, node_ids: &[NodeId]) -> Result<NodeId, BuildError> {
        let sinks: Vec<NodeId> = node_ids
            .iter()
            .filter(|id| {
                graph.get_node(id).map(|n| n.module_descriptor.is_sink).unwrap_or(false)
            })
            .cloned()
            .collect();

        match sinks.len() {
            0 => Err(BuildError::NoAudioOut),
            1 => Ok(sinks.into_iter().next().unwrap()),
            _ => Err(BuildError::MultipleAudioOut),
        }
    }

    /// Phase 3: Sort `node_ids` into ascending [`NodeId`] order and return the
    /// sorted vec together with the index of `sink` within it.
    fn compute_order(
        node_ids: &[NodeId],
        sink: &NodeId,
    ) -> Result<(Vec<NodeId>, usize), BuildError> {
        let mut order = node_ids.to_vec();
        order.sort_unstable();
        let audio_out_index = order
            .iter()
            .position(|id| id == sink)
            .ok_or_else(|| BuildError::InternalError("sink node missing from order".to_string()))?;
        Ok((order, audio_out_index))
    }

    /// Phase 4: Assign stable cable buffer pool indices.
    ///
    /// Reuses any `(NodeId, port_idx)` key already present in `prev_alloc`.
    /// New keys are filled from the freelist (LIFO) or the high-water mark.
    /// Old keys absent from the new graph are returned to the freelist and marked
    /// for zeroing on plan adoption.
    fn allocate_buffers(
        &self,
        graph: &ModuleGraph,
        order: &[NodeId],
        prev_alloc: &BufferAllocState,
    ) -> Result<BufferAllocation, BuildError> {
        let mut freelist = prev_alloc.freelist.clone();
        let mut next_hwm = prev_alloc.next_hwm;
        let mut to_zero = Vec::new();
        let mut output_buf: HashMap<(NodeId, usize), usize> = HashMap::new();

        for id in order {
            let desc = &graph
                .get_node(id)
                .ok_or_else(|| BuildError::InternalError(format!("node {id:?} missing from graph")))?
                .module_descriptor;

            for (port_idx, _) in desc.outputs.iter().enumerate() {
                let key = (id.clone(), port_idx);
                if let Some(&existing) = prev_alloc.output_buf.get(&key) {
                    output_buf.insert(key, existing);
                } else {
                    let idx = freelist.pop().unwrap_or_else(|| {
                        let i = next_hwm;
                        next_hwm += 1;
                        i
                    });
                    if idx >= self.pool_capacity {
                        return Err(BuildError::PoolExhausted);
                    }
                    to_zero.push(idx);
                    output_buf.insert(key, idx);
                }
            }
        }

        // Deallocate ports present in the old alloc that are not in the new graph.
        for (key, &buf_idx) in &prev_alloc.output_buf {
            if !output_buf.contains_key(key) {
                to_zero.push(buf_idx);
                freelist.push(buf_idx);
            }
        }

        Ok(BufferAllocation { output_buf, to_zero, freelist, next_hwm })
    }

    /// Phase 6: Build one [`ModuleSlot`] per node, collect fresh modules for
    /// installation, and record updated [`NodeState`]s.
    fn build_slots(
        graph: &ModuleGraph,
        order: &[NodeId],
        node_identity: &mut NodeIdentityMap,
        output_buf: &HashMap<(NodeId, usize), usize>,
        module_diff: &ModuleAllocDiff,
        prev_state: &PlannerState,
    ) -> Result<SlotBuildResult, BuildError> {
        let edges = graph.edge_list();
        let mut slots = Vec::with_capacity(order.len());
        let mut new_modules: Vec<(usize, Box<dyn Module>)> = Vec::new();
        let mut parameter_updates: Vec<(usize, ParameterMap)> = Vec::new();
        let mut node_states: HashMap<NodeId, NodeState> = HashMap::with_capacity(order.len());

        for id in order {
            let node = graph.get_node(id).ok_or_else(|| {
                BuildError::InternalError(format!("node {id:?} missing from graph"))
            })?;
            let desc = &node.module_descriptor;
            let instance_id = node_identity[id].0;
            let pool_index = *module_diff.slot_map.get(&instance_id).ok_or_else(|| {
                BuildError::InternalError(format!(
                    "instance {instance_id:?} missing from module_diff slot_map"
                ))
            })?;

            // Build input buffer assignments from the edge list.
            let (input_buffers, input_scales): (Vec<usize>, Vec<f64>) = desc
                .inputs
                .iter()
                .map(|port| {
                    let result = edges
                        .iter()
                        .find(|(_, _, _, to, in_name, in_idx, _)| {
                            *to == *id && in_name == port.name && *in_idx == port.index
                        })
                        .map(|(from, out_name, out_idx, _, _, _, scale)| {
                            let from_node = graph.get_node(from).ok_or_else(|| {
                                BuildError::InternalError(format!(
                                    "node {from:?} missing from graph"
                                ))
                            })?;
                            let from_desc = &from_node.module_descriptor;
                            let out_port_idx = from_desc
                                .outputs
                                .iter()
                                .position(|p| {
                                    p.name == out_name.as_str() && p.index == *out_idx
                                })
                                .ok_or_else(|| {
                                    BuildError::InternalError(format!(
                                        "output port {:?}/{} not found on node {from:?}",
                                        out_name, out_idx
                                    ))
                                })?;
                            let buf = output_buf
                                .get(&(from.clone(), out_port_idx))
                                .copied()
                                .ok_or_else(|| {
                                    BuildError::InternalError(format!(
                                        "buffer for ({from:?}, {out_port_idx}) not found"
                                    ))
                                })?;
                            Ok((buf, *scale))
                        })
                        .transpose()?
                        .unwrap_or((0, 1.0));
                    Ok(result)
                })
                .collect::<Result<Vec<_>, BuildError>>()?
                .into_iter()
                .unzip();

            let output_buffers: Vec<usize> = desc
                .outputs
                .iter()
                .enumerate()
                .map(|(port_idx, _)| {
                    output_buf
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

            // If this node is new or type-changed, install the fresh module.
            // Surviving nodes stay in the pool untouched, but may need parameter updates.
            if !prev_state.module_alloc.pool_map.contains_key(&instance_id) {
                let (_, fresh_opt) = node_identity.get_mut(id).unwrap();
                let fresh = fresh_opt.take().ok_or_else(|| {
                    BuildError::InternalError(format!(
                        "fresh module for new/type-changed node {id:?} is missing"
                    ))
                })?;
                new_modules.push((pool_index, fresh));
            } else {
                // Surviving module: emit a parameter diff if any values changed.
                if let Some(prev_ns) = prev_state.nodes.get(id) {
                    let diff: ParameterMap = node
                        .parameter_map
                        .iter()
                        .filter(|(k, v)| prev_ns.parameter_map.get(*k) != Some(v))
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    if !diff.is_empty() {
                        parameter_updates.push((pool_index, diff));
                    }
                }
            }

            node_states.insert(
                id.clone(),
                NodeState {
                    module_name: desc.module_name,
                    instance_id,
                    pool_index,
                    parameter_map: node.parameter_map.clone(),
                },
            );

            slots.push(ModuleSlot {
                pool_index,
                input_buffers,
                input_scales,
                output_buffers,
                input_scratch: vec![0.0; n_in],
                output_scratch: vec![0.0; n_out],
            });
        }

        Ok(SlotBuildResult { slots, new_modules, parameter_updates, node_states })
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
    use patches_modules::{AudioOut, SawtoothOscillator, SineOscillator};

    fn p(name: &'static str) -> PortRef {
        PortRef { name, index: 0 }
    }

    fn sine_to_audio_out_graph() -> ModuleGraph {
        let mut graph = ModuleGraph::new();
        let sine_desc = SineOscillator::describe(&ModuleShape { channels: 0 });
        let out_desc = AudioOut::describe(&ModuleShape { channels: 0 });
        let mut sine_params = ParameterMap::new();
        sine_params.insert("frequency".to_string(), ParameterValue::Float(440.0));
        graph.add_module("a_sine", sine_desc, &sine_params).unwrap();
        graph.add_module("b_out", out_desc, &ParameterMap::new()).unwrap();
        graph
            .connect(&NodeId::from("a_sine"), p("out"), &NodeId::from("b_out"), p("left"), 1.0)
            .unwrap();
        graph
            .connect(&NodeId::from("a_sine"), p("out"), &NodeId::from("b_out"), p("right"), 1.0)
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
        let left_buf = plan.slots[audio_out_idx].input_buffers[0];
        let right_buf = plan.slots[audio_out_idx].input_buffers[1];

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
        let sine_desc = SineOscillator::describe(&ModuleShape { channels: 0 });
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
        let sine_desc = SineOscillator::describe(&ModuleShape { channels: 0 });
        let out1_desc = AudioOut::describe(&ModuleShape { channels: 0 });
        let out2_desc = AudioOut::describe(&ModuleShape { channels: 0 });
        let mut p_map = ParameterMap::new();
        p_map.insert("frequency".to_string(), ParameterValue::Float(440.0));
        graph.add_module("sine", sine_desc, &p_map).unwrap();
        graph.add_module("out1", out1_desc, &ParameterMap::new()).unwrap();
        graph.add_module("out2", out2_desc, &ParameterMap::new()).unwrap();
        graph.connect(&NodeId::from("sine"), p("out"), &NodeId::from("out1"), p("left"), 1.0).unwrap();
        graph.connect(&NodeId::from("sine"), p("out"), &NodeId::from("out1"), p("right"), 1.0).unwrap();
        graph.connect(&NodeId::from("sine"), p("out"), &NodeId::from("out2"), p("left"), 1.0).unwrap();
        graph.connect(&NodeId::from("sine"), p("out"), &NodeId::from("out2"), p("right"), 1.0).unwrap();
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
            let sine_desc = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out_desc = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut p_map = ParameterMap::new();
            p_map.insert("frequency".to_string(), ParameterValue::Float(440.0));
            g.add_module("sine", sine_desc, &p_map).unwrap();
            g.add_module("out", out_desc, &ParameterMap::new()).unwrap();
            g.connect(&NodeId::from("sine"), p("out"), &NodeId::from("out"), p("left"), scale).unwrap();
            g.connect(&NodeId::from("sine"), p("out"), &NodeId::from("out"), p("right"), scale).unwrap();
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
            let sine_a = SineOscillator::describe(&ModuleShape { channels: 0 });
            let sine_b = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out_desc = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut pa = ParameterMap::new();
            pa.insert("frequency".to_string(), ParameterValue::Float(440.0));
            let mut pb = ParameterMap::new();
            pb.insert("frequency".to_string(), ParameterValue::Float(880.0));
            graph_a.add_module("sine_a", sine_a, &pa).unwrap();
            graph_a.add_module("sine_b", sine_b, &pb).unwrap();
            graph_a.add_module("out", out_desc, &ParameterMap::new()).unwrap();
            graph_a
                .connect(&NodeId::from("sine_a"), p("out"), &NodeId::from("out"), p("left"), 1.0)
                .unwrap();
            graph_a
                .connect(&NodeId::from("sine_b"), p("out"), &NodeId::from("out"), p("right"), 1.0)
                .unwrap();
        }

        let (_plan_a, state_a) =
            builder.build_patch(&graph_a, &registry, &env, &PlannerState::empty()).unwrap();

        let buf_a = state_a.buffer_alloc.output_buf[&(NodeId::from("sine_a"), 0)];

        // Graph B: only sine_a (sine_b removed)
        let mut graph_b = ModuleGraph::new();
        {
            let sine_a = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out_desc = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut pa = ParameterMap::new();
            pa.insert("frequency".to_string(), ParameterValue::Float(440.0));
            graph_b.add_module("sine_a", sine_a, &pa).unwrap();
            graph_b.add_module("out", out_desc, &ParameterMap::new()).unwrap();
            graph_b
                .connect(&NodeId::from("sine_a"), p("out"), &NodeId::from("out"), p("left"), 1.0)
                .unwrap();
            graph_b
                .connect(&NodeId::from("sine_a"), p("out"), &NodeId::from("out"), p("right"), 1.0)
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
            let s1 = SineOscillator::describe(&ModuleShape { channels: 0 });
            let s2 = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut p1 = ParameterMap::new();
            p1.insert("frequency".to_string(), ParameterValue::Float(440.0));
            let mut p2 = ParameterMap::new();
            p2.insert("frequency".to_string(), ParameterValue::Float(880.0));
            g.add_module("s1", s1, &p1).unwrap();
            g.add_module("s2", s2, &p2).unwrap();
            g.add_module("out", out, &ParameterMap::new()).unwrap();
            g.connect(&NodeId::from("s1"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            g.connect(&NodeId::from("s2"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
            let (_, new_state) = builder.build_patch(&g, &registry, &env, state).unwrap();
            new_state
        };

        let build_one = |state: &PlannerState| {
            let mut g = ModuleGraph::new();
            let s = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut pm = ParameterMap::new();
            pm.insert("frequency".to_string(), ParameterValue::Float(440.0));
            g.add_module("s1", s, &pm).unwrap();
            g.add_module("out", out, &ParameterMap::new()).unwrap();
            g.connect(&NodeId::from("s1"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            g.connect(&NodeId::from("s1"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
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
        let sine_desc = SineOscillator::describe(&ModuleShape { channels: 0 });
        let out_desc = AudioOut::describe(&ModuleShape { channels: 0 });
        let mut pm = ParameterMap::new();
        pm.insert("frequency".to_string(), ParameterValue::Float(440.0));
        graph.add_module("sine", sine_desc, &pm).unwrap();
        graph.add_module("out", out_desc, &ParameterMap::new()).unwrap();
        graph.connect(&NodeId::from("sine"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
        graph.connect(&NodeId::from("sine"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
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
            let s1 = SineOscillator::describe(&ModuleShape { channels: 0 });
            let s2 = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut p1 = ParameterMap::new();
            p1.insert("frequency".to_string(), ParameterValue::Float(440.0));
            let mut p2 = ParameterMap::new();
            p2.insert("frequency".to_string(), ParameterValue::Float(880.0));
            graph_a.add_module("s1", s1, &p1).unwrap();
            graph_a.add_module("s2", s2, &p2).unwrap();
            graph_a.add_module("out", out, &ParameterMap::new()).unwrap();
            graph_a.connect(&NodeId::from("s1"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            graph_a.connect(&NodeId::from("s2"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
        }
        let (_plan_a, state_a) =
            builder.build_patch(&graph_a, &registry, &env, &PlannerState::empty()).unwrap();
        let s2_slot = state_a.nodes[&NodeId::from("s2")].pool_index;

        // Graph with only s1.
        let mut graph_b = ModuleGraph::new();
        {
            let s1 = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut p1 = ParameterMap::new();
            p1.insert("frequency".to_string(), ParameterValue::Float(440.0));
            graph_b.add_module("s1", s1, &p1).unwrap();
            graph_b.add_module("out", out, &ParameterMap::new()).unwrap();
            graph_b.connect(&NodeId::from("s1"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            graph_b.connect(&NodeId::from("s1"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
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

        // Graph A: SineOscillator at "osc".
        let mut graph_a = ModuleGraph::new();
        {
            let sine = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut pm = ParameterMap::new();
            pm.insert("frequency".to_string(), ParameterValue::Float(440.0));
            graph_a.add_module("osc", sine, &pm).unwrap();
            graph_a.add_module("out", out, &ParameterMap::new()).unwrap();
            // SineOscillator has no inputs so we wire its "out" to both channels.
            graph_a.connect(&NodeId::from("osc"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
            graph_a.connect(&NodeId::from("osc"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
        }
        let (_plan_a, state_a) =
            builder.build_patch(&graph_a, &registry, &env, &PlannerState::empty()).unwrap();
        let old_osc_id = state_a.nodes[&NodeId::from("osc")].instance_id;
        let old_osc_slot = state_a.nodes[&NodeId::from("osc")].pool_index;

        // Graph B: SawtoothOscillator at "osc" (type changed).
        let mut graph_b = ModuleGraph::new();
        {
            let saw = SawtoothOscillator::describe(&ModuleShape { channels: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0 });
            graph_b.add_module("osc", saw, &ParameterMap::new()).unwrap();
            graph_b.add_module("out", out, &ParameterMap::new()).unwrap();
            // SawtoothOscillator has a "voct" input (wired to zero buffer implicitly).
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
        // Exactly one new module installed (the new SawtoothOscillator; AudioOut survives).
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
            matches!(result, Err(BuildError::ModulePoolExhausted)),
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
            let sine_desc = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out_desc = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut sine_params = ParameterMap::new();
            sine_params.insert("frequency".to_string(), ParameterValue::Float(880.0));
            graph_b.add_module("a_sine", sine_desc, &sine_params).unwrap();
            graph_b.add_module("b_out", out_desc, &ParameterMap::new()).unwrap();
            graph_b
                .connect(&NodeId::from("a_sine"), p("out"), &NodeId::from("b_out"), p("left"), 1.0)
                .unwrap();
            graph_b
                .connect(
                    &NodeId::from("a_sine"),
                    p("out"),
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
        let sine_slot = state_a.nodes[&NodeId::from("a_sine")].pool_index;
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
            let s_a = SineOscillator::describe(&ModuleShape { channels: 0 });
            let s_b = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut pa = ParameterMap::new();
            pa.insert("frequency".to_string(), ParameterValue::Float(440.0));
            let mut pb = ParameterMap::new();
            pb.insert("frequency".to_string(), ParameterValue::Float(880.0));
            graph_a.add_module("s_a", s_a, &pa).unwrap();
            graph_a.add_module("s_b", s_b, &pb).unwrap();
            graph_a.add_module("out", out, &ParameterMap::new()).unwrap();
            graph_a
                .connect(&NodeId::from("s_a"), p("out"), &NodeId::from("out"), p("left"), 1.0)
                .unwrap();
            graph_a
                .connect(&NodeId::from("s_b"), p("out"), &NodeId::from("out"), p("right"), 1.0)
                .unwrap();
        }
        let (_plan_a, state_a) =
            builder.build_patch(&graph_a, &registry, &env, &PlannerState::empty()).unwrap();
        let s_b_slot = state_a.nodes[&NodeId::from("s_b")].pool_index;

        // Graph B: sine_a (changed to 660 Hz) + new sine_c (1000 Hz), sine_b removed.
        let mut graph_b = ModuleGraph::new();
        {
            let s_a = SineOscillator::describe(&ModuleShape { channels: 0 });
            let s_c = SineOscillator::describe(&ModuleShape { channels: 0 });
            let out = AudioOut::describe(&ModuleShape { channels: 0 });
            let mut pa = ParameterMap::new();
            pa.insert("frequency".to_string(), ParameterValue::Float(660.0));
            let mut pc = ParameterMap::new();
            pc.insert("frequency".to_string(), ParameterValue::Float(1000.0));
            graph_b.add_module("s_a", s_a, &pa).unwrap();
            graph_b.add_module("s_c", s_c, &pc).unwrap();
            graph_b.add_module("out", out, &ParameterMap::new()).unwrap();
            graph_b
                .connect(&NodeId::from("s_a"), p("out"), &NodeId::from("out"), p("left"), 1.0)
                .unwrap();
            graph_b
                .connect(&NodeId::from("s_c"), p("out"), &NodeId::from("out"), p("right"), 1.0)
                .unwrap();
        }
        let (plan_b, _state_b) =
            builder.build_patch(&graph_b, &registry, &env, &state_a).unwrap();

        // s_b was removed → tombstoned.
        assert!(plan_b.tombstones.contains(&s_b_slot), "s_b must be tombstoned");
        // s_c is new → appears in new_modules (pool_index may vary; just check count).
        // s_a is surviving with changed param → appears in parameter_updates.
        let s_a_slot = state_a.nodes[&NodeId::from("s_a")].pool_index;
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
            plan_b.new_modules.iter().filter(|(_, m)| m.descriptor().module_name == "SineOscillator").count(),
            1,
            "exactly one new SineOscillator (s_c) in new_modules"
        );
    }
}
