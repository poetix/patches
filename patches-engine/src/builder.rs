use std::collections::{HashMap, HashSet};
use std::fmt;

use patches_core::{InstanceId, Module, ModuleDescriptor, ModuleGraph, NodeId};

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
        }
    }
}

impl std::error::Error for BuildError {}

/// Stable buffer index allocation state threaded across successive [`build_patch`] calls.
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

/// Stable module slot allocation state threaded across successive [`build_patch`] calls.
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

/// A fully resolved, allocation-free execution structure produced by [`build_patch`].
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
    pub signal_dispatch: Box<[(InstanceId, usize)]>,
    /// New modules to install into the audio-thread module pool when this plan
    /// is adopted. Each entry is `(pool_index, Box<dyn Module>)`.
    ///
    /// The audio callback drains this vec into the pool on plan adoption. Modules
    /// are initialised (via [`Module::initialise`]) in [`SoundEngine::swap_plan`]
    /// before the plan is pushed to the channel.
    pub new_modules: Vec<(usize, Box<dyn Module>)>,
    /// Pool indices of modules removed from the graph.
    ///
    /// The audio callback calls `pool[idx].take()` for each entry, dropping the
    /// `Box<dyn Module>` and freeing the slot.
    pub tombstones: Vec<usize>,
}

impl ExecutionPlan {
    /// Process one sample across all modules in execution order.
    ///
    /// `module_pool` is the audio-thread-owned module pool.
    /// `buffer_pool` is the externally-owned cable buffer pool (see [`SoundEngine`]).
    /// `wi` is the write slot index (0 or 1); the read slot is `1 - wi`.
    /// Callers must alternate between `wi = 0` and `wi = 1` on successive calls.
    ///
    /// Does not allocate.
    pub fn tick(
        &mut self,
        module_pool: &mut [Option<Box<dyn Module>>],
        buffer_pool: &mut [[f64; 2]],
        wi: usize,
    ) {
        let ri = 1 - wi;

        for slot in self.slots.iter_mut() {
            for (j, &buf_idx) in slot.input_buffers.iter().enumerate() {
                slot.input_scratch[j] = buffer_pool[buf_idx][ri] * slot.input_scales[j];
            }
            module_pool[slot.pool_index]
                .as_mut()
                .unwrap()
                .process(&slot.input_scratch, &mut slot.output_scratch);
            for (j, &buf_idx) in slot.output_buffers.iter().enumerate() {
                buffer_pool[buf_idx][wi] = slot.output_scratch[j];
            }
        }
    }

    /// Left-channel sample produced during the most recent [`tick`](Self::tick).
    pub fn last_left(&self, pool: &[Option<Box<dyn Module>>]) -> f64 {
        pool[self.slots[self.audio_out_index].pool_index]
            .as_ref()
            .and_then(|m| m.as_sink())
            .map_or(0.0, |s| s.last_left())
    }

    /// Right-channel sample produced during the most recent [`tick`](Self::tick).
    pub fn last_right(&self, pool: &[Option<Box<dyn Module>>]) -> f64 {
        pool[self.slots[self.audio_out_index].pool_index]
            .as_ref()
            .and_then(|m| m.as_sink())
            .map_or(0.0, |s| s.last_right())
    }
}

/// Consume a [`ModuleGraph`] and produce an [`ExecutionPlan`] with updated
/// [`BufferAllocState`] and [`ModuleAllocState`].
///
/// Validates that exactly one `AudioOut` node is present, orders modules by
/// ascending [`NodeId`], and resolves per-module input/output buffer assignments.
/// Unconnected input ports are assigned the permanent-zero buffer (index 0);
/// output ports get a dedicated buffer from the pool.
///
/// ## Stable buffer allocation
///
/// Buffer indices are assigned via `alloc`:
/// - If the `(NodeId, output_port_index)` key already exists in `alloc.output_buf`,
///   the same index is reused and is **not** added to `to_zero`.
/// - If the key is new, an index is popped from `alloc.freelist`; if the freelist
///   is empty, `alloc.next_hwm` is used and incremented. If the index would equal
///   or exceed `pool_capacity`, [`BuildError::PoolExhausted`] is returned.
/// - Output ports present in `alloc.output_buf` but absent from the new graph have
///   their indices pushed onto the returned freelist and appended to `to_zero`.
///
/// ## Module pool allocation
///
/// Module pool slots are assigned via `module_alloc`:
/// - Surviving modules (same [`InstanceId`] in both old and new graph) reuse their
///   existing pool slot and appear in neither `new_modules` nor `tombstones`.
/// - New modules receive a slot from the freelist or high-water mark and are placed
///   in `ExecutionPlan::new_modules` for installation by the audio callback.
/// - Removed modules have their slots tombstoned; indices appear in
///   `ExecutionPlan::tombstones` for the audio callback to free.
pub fn build_patch(
    graph: ModuleGraph,
    alloc: &BufferAllocState,
    module_alloc: &ModuleAllocState,
    pool_capacity: usize,
    module_pool_capacity: usize,
) -> Result<(ExecutionPlan, BufferAllocState, ModuleAllocState), BuildError> {
    let node_ids = graph.node_ids();
    let edges = graph.edge_list();

    // Snapshot descriptors and instance IDs before consuming the graph.
    struct NodeMeta {
        descriptor: ModuleDescriptor,
        instance_id: InstanceId,
        is_sink: bool,
    }
    let meta: HashMap<NodeId, NodeMeta> = node_ids
        .iter()
        .map(|id| {
            graph
                .get_module(id)
                .ok_or_else(|| {
                    BuildError::InternalError(format!("node {id:?} missing from graph"))
                })
                .map(|m| {
                    (
                        id.clone(),
                        NodeMeta {
                            descriptor: m.descriptor().clone(),
                            instance_id: m.instance_id(),
                            is_sink: m.as_sink().is_some(),
                        },
                    )
                })
        })
        .collect::<Result<HashMap<_, _>, _>>()?;

    // Identify sink nodes via the Sink trait.
    let audio_out_ids: Vec<NodeId> = node_ids
        .iter()
        .filter(|id| meta[id].is_sink)
        .cloned()
        .collect();

    let audio_out_node = match audio_out_ids.len() {
        0 => return Err(BuildError::NoAudioOut),
        1 => audio_out_ids[0].clone(),
        _ => return Err(BuildError::MultipleAudioOut),
    };

    // Execution order: ascending NodeId (insertion order). The 1-sample cable
    // delay makes any ordering correct; ascending NodeId gives stable, deterministic output.
    let mut order = node_ids.clone();
    order.sort_unstable();

    let audio_out_index = order
        .iter()
        .position(|id| *id == audio_out_node)
        .ok_or_else(|| {
            BuildError::InternalError("audio_out node missing from order".to_string())
        })?;

    // Consume the graph's modules.
    let mut modules = graph.into_modules();

    // Stable buffer index allocation.
    let mut new_freelist: Vec<usize> = alloc.freelist.clone();
    let mut new_hwm: usize = alloc.next_hwm;
    let mut to_zero: Vec<usize> = Vec::new();

    let mut output_buf: HashMap<(NodeId, usize), usize> = HashMap::new();

    for id in &order {
        let desc = &meta[id].descriptor;
        for (port_idx, _) in desc.outputs.iter().enumerate() {
            let key = (id.clone(), port_idx);
            if let Some(&existing) = alloc.output_buf.get(&key) {
                output_buf.insert(key, existing);
            } else {
                let idx = if let Some(recycled) = new_freelist.pop() {
                    recycled
                } else {
                    let idx = new_hwm;
                    new_hwm += 1;
                    idx
                };
                if idx >= pool_capacity {
                    return Err(BuildError::PoolExhausted);
                }
                to_zero.push(idx);
                output_buf.insert(key, idx);
            }
        }
    }

    // Deallocation: ports present in the old alloc that are no longer in the new graph.
    for (key, &buf_idx) in &alloc.output_buf {
        if !output_buf.contains_key(key) {
            to_zero.push(buf_idx);
            new_freelist.push(buf_idx);
        }
    }

    // Stable module pool allocation.
    let new_ids: HashSet<InstanceId> = node_ids.iter().map(|id| meta[id].instance_id).collect();
    let module_diff = module_alloc.diff(&new_ids, module_pool_capacity)?;

    // Build the module slots in execution order, collecting new module instances.
    let mut slots: Vec<ModuleSlot> = Vec::with_capacity(order.len());
    let mut new_modules: Vec<(usize, Box<dyn Module>)> = Vec::new();

    for id in &order {
        let desc = &meta[id].descriptor;

        let (input_buffers, input_scales): (Vec<usize>, Vec<f64>) = desc
            .inputs
            .iter()
            .map(|port| {
                let (buf_idx, scale) = edges
                    .iter()
                    .find(|(_, _, _, to, in_name, in_idx, _)| {
                        *to == *id && in_name == port.name && *in_idx == port.index
                    })
                    .map(|(from, out_name, out_idx, _, _, _, scale)| -> Result<(usize, f64), BuildError> {
                        let from_desc = &meta[from].descriptor;
                        let out_port_idx = from_desc
                            .outputs
                            .iter()
                            .position(|p| p.name == out_name && p.index == *out_idx)
                            .ok_or_else(|| {
                                BuildError::InternalError(format!(
                                    "output port {:?}/{} not found on node {from:?}",
                                    out_name, out_idx
                                ))
                            })?;
                        Ok((output_buf[&(from.clone(), out_port_idx)], *scale))
                    })
                    .transpose()?
                    .unwrap_or((0, 1.0));
                Ok((buf_idx, scale))
            })
            .collect::<Result<Vec<_>, BuildError>>()?
            .into_iter()
            .unzip();

        let output_buffers: Vec<usize> = desc
            .outputs
            .iter()
            .enumerate()
            .map(|(port_idx, _)| output_buf[&(id.clone(), port_idx)])
            .collect();

        let n_in = desc.inputs.len();
        let n_out = desc.outputs.len();

        let instance_id = meta[id].instance_id;
        let pool_index = module_diff.slot_map[&instance_id];

        // Take the fresh module from the graph.
        let fresh = modules.remove(id).ok_or_else(|| {
            BuildError::InternalError(format!("module {id:?} missing from map"))
        })?;

        // New modules go into new_modules; surviving modules stay in the pool untouched
        // (the fresh instance from the graph is dropped).
        if !module_alloc.pool_map.contains_key(&instance_id) {
            new_modules.push((pool_index, fresh));
        }
        // If surviving, `fresh` is dropped here — the stateful instance remains in the pool.

        slots.push(ModuleSlot {
            pool_index,
            input_buffers,
            input_scales,
            output_buffers,
            input_scratch: vec![0.0; n_in],
            output_scratch: vec![0.0; n_out],
        });
    }

    let tombstones = module_diff.tombstoned;

    let new_alloc = BufferAllocState {
        output_buf,
        freelist: new_freelist,
        next_hwm: new_hwm,
    };

    let new_module_alloc = ModuleAllocState {
        pool_map: module_diff.slot_map,
        freelist: module_diff.freelist,
        next_hwm: module_diff.next_hwm,
    };

    // Build the signal_dispatch sorted array: (InstanceId → pool_index).
    // Sorted by InstanceId so the audio callback can binary-search in O(log M).
    let mut dispatch: Vec<(InstanceId, usize)> = slots
        .iter()
        .zip(order.iter())
        .map(|(slot, id)| (meta[id].instance_id, slot.pool_index))
        .collect();
    dispatch.sort_unstable_by_key(|(id, _)| *id);
    let signal_dispatch = dispatch.into_boxed_slice();

    Ok((
        ExecutionPlan {
            slots,
            to_zero,
            audio_out_index,
            signal_dispatch,
            new_modules,
            tombstones,
        },
        new_alloc,
        new_module_alloc,
    ))
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use patches_core::{AudioEnvironment, Module, NodeId, PortRef};
    use patches_modules::{AudioOut, SineOscillator};

    fn p(name: &'static str) -> PortRef {
        PortRef { name, index: 0 }
    }

    fn sine_to_audio_out_graph() -> (ModuleGraph, NodeId, NodeId) {
        let mut graph = ModuleGraph::new();
        let sine_id = NodeId::from("a_sine");
        let out_id = NodeId::from("b_out");
        graph.add_module(sine_id.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
        graph.add_module(out_id.clone(), Box::new(AudioOut::new())).unwrap();
        graph.connect(&sine_id, p("out"), &out_id, p("left"), 1.0).unwrap();
        graph.connect(&sine_id, p("out"), &out_id, p("right"), 1.0).unwrap();
        (graph, sine_id, out_id)
    }

    fn make_buffer_pool(capacity: usize) -> Vec<[f64; 2]> {
        vec![[0.0; 2]; capacity]
    }

    fn make_module_pool(capacity: usize) -> Vec<Option<Box<dyn Module>>> {
        (0..capacity).map(|_| None).collect()
    }

    /// Build a plan from scratch (no prior alloc state) and install its modules.
    fn default_build(
        graph: ModuleGraph,
    ) -> (ExecutionPlan, BufferAllocState, ModuleAllocState, Vec<Option<Box<dyn Module>>>) {
        let mut plan = build_patch(
            graph,
            &BufferAllocState::default(),
            &ModuleAllocState::default(),
            256,
            256,
        )
        .expect("build should succeed");

        // Simulate SoundEngine: install new_modules into module pool and initialise.
        let mut module_pool = make_module_pool(256);
        let env = AudioEnvironment { sample_rate: 44100.0 };
        for (idx, mut m) in plan.0.new_modules.drain(..) {
            m.initialise(&env);
            module_pool[idx] = Some(m);
        }

        (plan.0, plan.1, plan.2, module_pool)
    }

    #[test]
    fn builds_minimal_plan_with_correct_order() {
        let (graph, _, _) = sine_to_audio_out_graph();
        let (plan, _, _, module_pool) = default_build(graph);

        let audio_out_idx = plan.audio_out_index;
        let sine_idx = plan
            .slots
            .iter()
            .position(|s| {
                module_pool[s.pool_index]
                    .as_ref()
                    .and_then(|m| m.as_any().downcast_ref::<SineOscillator>())
                    .is_some()
            })
            .expect("sine slot not found");

        assert!(sine_idx < audio_out_idx, "sine must execute before AudioOut");
    }

    #[test]
    fn fanout_buffer_shared_between_both_inputs() {
        let (graph, _, _) = sine_to_audio_out_graph();
        let (plan, _, _, _) = default_build(graph);

        let audio_out_idx = plan.audio_out_index;
        let sine_idx = plan
            .slots
            .iter()
            .position(|s| {
                // Find by pool_index: sine is not AudioOut, check by slot position
                // (sine is at lower index than AudioOut per ascending-NodeId order)
                s.pool_index != plan.slots[audio_out_idx].pool_index
            })
            .unwrap();

        let sine_out_buf = plan.slots[sine_idx].output_buffers[0];
        let left_buf = plan.slots[audio_out_idx].input_buffers[0];
        let right_buf = plan.slots[audio_out_idx].input_buffers[1];

        assert_eq!(sine_out_buf, left_buf, "left input must use sine output buffer");
        assert_eq!(sine_out_buf, right_buf, "right input must use sine output buffer");
    }

    #[test]
    fn tick_produces_bounded_audio_output() {
        let (graph, _, _) = sine_to_audio_out_graph();
        let (mut plan, _, _, mut module_pool) = default_build(graph);
        let mut buffer_pool = make_buffer_pool(256);

        for i in 0..1000 {
            plan.tick(&mut module_pool, &mut buffer_pool, i % 2);
        }

        assert!(plan.last_left(&module_pool).abs() <= 1.0);
        assert!(plan.last_right(&module_pool).abs() <= 1.0);
        assert!(plan.last_left(&module_pool).abs() > 0.0);
    }

    #[test]
    fn no_audio_out_returns_error() {
        let mut graph = ModuleGraph::new();
        graph.add_module("sine", Box::new(SineOscillator::new(440.0))).unwrap();
        assert!(matches!(
            build_patch(graph, &BufferAllocState::default(), &ModuleAllocState::default(), 256, 256),
            Err(BuildError::NoAudioOut)
        ));
    }

    #[test]
    fn multiple_audio_out_returns_error() {
        let mut graph = ModuleGraph::new();
        let sine_id = NodeId::from("sine");
        let out1 = NodeId::from("out1");
        let out2 = NodeId::from("out2");
        graph.add_module(sine_id.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
        graph.add_module(out1.clone(), Box::new(AudioOut::new())).unwrap();
        graph.add_module(out2.clone(), Box::new(AudioOut::new())).unwrap();
        graph.connect(&sine_id, p("out"), &out1, p("left"), 1.0).unwrap();
        graph.connect(&sine_id, p("out"), &out1, p("right"), 1.0).unwrap();
        graph.connect(&sine_id, p("out"), &out2, p("left"), 1.0).unwrap();
        graph.connect(&sine_id, p("out"), &out2, p("right"), 1.0).unwrap();
        assert!(matches!(
            build_patch(graph, &BufferAllocState::default(), &ModuleAllocState::default(), 256, 256),
            Err(BuildError::MultipleAudioOut)
        ));
    }

    #[test]
    fn input_scale_is_applied_at_tick_time() {
        let mut graph_half = ModuleGraph::new();
        graph_half.add_module("sine", Box::new(SineOscillator::new(440.0))).unwrap();
        graph_half.add_module("out", Box::new(patches_modules::AudioOut::new())).unwrap();
        let sine_h = NodeId::from("sine");
        let out_h = NodeId::from("out");
        graph_half.connect(&sine_h, p("out"), &out_h, p("left"), 0.5).unwrap();
        graph_half.connect(&sine_h, p("out"), &out_h, p("right"), 0.5).unwrap();

        let mut graph_full = ModuleGraph::new();
        graph_full.add_module("sine", Box::new(SineOscillator::new(440.0))).unwrap();
        graph_full.add_module("out", Box::new(patches_modules::AudioOut::new())).unwrap();
        let sine_f = NodeId::from("sine");
        let out_f = NodeId::from("out");
        graph_full.connect(&sine_f, p("out"), &out_f, p("left"), 1.0).unwrap();
        graph_full.connect(&sine_f, p("out"), &out_f, p("right"), 1.0).unwrap();

        let (mut plan_half, _, _, mut pool_half) = default_build(graph_half);
        let (mut plan_full, _, _, mut pool_full) = default_build(graph_full);
        let mut buffer_half = make_buffer_pool(256);
        let mut buffer_full = make_buffer_pool(256);

        for i in 0..100 {
            plan_half.tick(&mut pool_half, &mut buffer_half, i % 2);
            plan_full.tick(&mut pool_full, &mut buffer_full, i % 2);
        }

        let half = plan_half.last_left(&pool_half);
        let full = plan_full.last_left(&pool_full);

        if full.abs() > 1e-6 {
            let ratio = half / full;
            assert!(
                (ratio - 0.5).abs() < 1e-9,
                "expected half ≈ full * 0.5, got half={half}, full={full}, ratio={ratio}"
            );
        }
    }

    // ── Acceptance criteria: T-0025 ──────────────────────────────────────────

    #[test]
    fn stable_buffer_index_for_unchanged_module_across_replan() {
        let pool_capacity = 256;
        let alloc0 = BufferAllocState::default();
        let module_alloc0 = ModuleAllocState::default();

        let mut graph_a = ModuleGraph::new();
        graph_a.add_module("sine_a", Box::new(SineOscillator::new(440.0))).unwrap();
        graph_a.add_module("sine_b", Box::new(SineOscillator::new(880.0))).unwrap();
        graph_a.add_module("out", Box::new(AudioOut::new())).unwrap();
        let sine_a = NodeId::from("sine_a");
        let sine_b = NodeId::from("sine_b");
        let out_a = NodeId::from("out");
        graph_a.connect(&sine_a, p("out"), &out_a, p("left"), 1.0).unwrap();
        graph_a.connect(&sine_b, p("out"), &out_a, p("right"), 1.0).unwrap();

        let (_plan_a, alloc_a, module_alloc_a) =
            build_patch(graph_a, &alloc0, &module_alloc0, pool_capacity, pool_capacity).unwrap();

        let buf_a = alloc_a.output_buf[&(NodeId::from("sine_a"), 0)];

        let mut graph_b = ModuleGraph::new();
        graph_b.add_module("sine_a", Box::new(SineOscillator::new(440.0))).unwrap();
        graph_b.add_module("out", Box::new(AudioOut::new())).unwrap();
        let sine_a_b = NodeId::from("sine_a");
        let out_b = NodeId::from("out");
        graph_b.connect(&sine_a_b, p("out"), &out_b, p("left"), 1.0).unwrap();
        graph_b.connect(&sine_a_b, p("out"), &out_b, p("right"), 1.0).unwrap();

        let (plan_b, alloc_b, _) =
            build_patch(graph_b, &alloc_a, &module_alloc_a, pool_capacity, pool_capacity).unwrap();

        let buf_b = alloc_b.output_buf[&(NodeId::from("sine_a"), 0)];

        assert_eq!(buf_a, buf_b, "sine_a output buffer must be identical across re-plan");

        let freed_buf = alloc_a.output_buf[&(NodeId::from("sine_b"), 0)];
        assert!(
            plan_b.to_zero.contains(&freed_buf),
            "freed buffer index {freed_buf} must appear in plan_b.to_zero"
        );
    }

    #[test]
    fn freelist_recycles_indices_preventing_hwm_growth() {
        let pool_capacity = 256;
        let module_alloc0 = ModuleAllocState::default();

        let build_two = |alloc: &BufferAllocState, module_alloc: &ModuleAllocState| {
            let mut g = ModuleGraph::new();
            let s1 = NodeId::from("s1");
            let s2 = NodeId::from("s2");
            let out = NodeId::from("out");
            g.add_module(s1.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
            g.add_module(s2.clone(), Box::new(SineOscillator::new(880.0))).unwrap();
            g.add_module(out.clone(), Box::new(AudioOut::new())).unwrap();
            g.connect(&s1, p("out"), &out, p("left"), 1.0).unwrap();
            g.connect(&s2, p("out"), &out, p("right"), 1.0).unwrap();
            let (_, a, ma) = build_patch(g, alloc, module_alloc, pool_capacity, pool_capacity).unwrap();
            (a, ma)
        };

        let build_one = |alloc: &BufferAllocState, module_alloc: &ModuleAllocState| {
            let mut g = ModuleGraph::new();
            let s = NodeId::from("s1");
            let out = NodeId::from("out");
            g.add_module(s.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
            g.add_module(out.clone(), Box::new(AudioOut::new())).unwrap();
            g.connect(&s, p("out"), &out, p("left"), 1.0).unwrap();
            g.connect(&s, p("out"), &out, p("right"), 1.0).unwrap();
            let (_, a, ma) = build_patch(g, alloc, module_alloc, pool_capacity, pool_capacity).unwrap();
            (a, ma)
        };

        let (alloc_a, module_alloc_a) = build_two(&BufferAllocState::default(), &module_alloc0);
        let hwm_after_first_two = alloc_a.next_hwm;

        let mut current_alloc = alloc_a;
        let mut current_module_alloc = module_alloc_a;
        for _ in 0..20 {
            let (a, ma) = build_one(&current_alloc, &current_module_alloc);
            current_alloc = a;
            current_module_alloc = ma;
            let (a, ma) = build_two(&current_alloc, &current_module_alloc);
            current_alloc = a;
            current_module_alloc = ma;
        }

        assert_eq!(
            current_alloc.next_hwm, hwm_after_first_two,
            "hwm grew from {hwm_after_first_two} to {}: freelist should have prevented new allocations",
            current_alloc.next_hwm
        );
    }

    #[test]
    fn pool_exhausted_error_when_capacity_exceeded() {
        let mut graph = ModuleGraph::new();
        let sine = NodeId::from("sine");
        let out = NodeId::from("out");
        graph.add_module(sine.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
        graph.add_module(out.clone(), Box::new(AudioOut::new())).unwrap();
        graph.connect(&sine, p("out"), &out, p("left"), 1.0).unwrap();
        graph.connect(&sine, p("out"), &out, p("right"), 1.0).unwrap();

        assert!(matches!(
            build_patch(graph, &BufferAllocState::default(), &ModuleAllocState::default(), 1, 256),
            Err(BuildError::PoolExhausted)
        ));
    }

    // ── ModuleAllocState unit tests ───────────────────────────────────────────

    fn make_ids(n: u64) -> Vec<InstanceId> {
        (0..n)
            .map(|_| SineOscillator::new(440.0).instance_id())
            .collect()
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
}
