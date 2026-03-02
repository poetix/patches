use std::collections::HashMap;
use std::fmt;

use patches_core::{AudioEnvironment, Module, ModuleDescriptor, ModuleGraph, ModuleInstanceRegistry, NodeId};

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

/// One entry in the execution plan: a module together with its pre-resolved
/// input and output buffer indices and pre-allocated scratch storage.
pub struct ModuleSlot {
    pub module: Box<dyn Module>,
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
/// Call [`tick`](ExecutionPlan::tick) once per sample on the audio thread,
/// passing the externally-owned buffer pool and alternating `wi = 0` and `wi = 1`
/// on successive calls.
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
}

impl ExecutionPlan {
    /// Call `initialise` on every module in this plan with the given environment.
    ///
    /// Must be called once before the first [`tick`](Self::tick), and again
    /// whenever the plan is swapped into a running engine (e.g. after a hot-reload).
    pub fn initialise(&mut self, env: &AudioEnvironment) {
        for slot in &mut self.slots {
            slot.module.initialise(env);
        }
    }

    /// Process one sample across all modules in execution order.
    ///
    /// `pool` is the externally-owned cable buffer pool (see [`SoundEngine`]).
    /// `wi` is the write slot index (0 or 1); the read slot is `1 - wi`.
    /// Callers must alternate between `wi = 0` and `wi = 1` on successive calls.
    ///
    /// Does not allocate.
    pub fn tick(&mut self, pool: &mut [[f64; 2]], wi: usize) {
        let ri = 1 - wi;

        // Per slot: read inputs → process → write outputs.
        // Reading uses `ri` (previous tick's slot); writing uses `wi` (this tick's slot).
        // Because ri ≠ wi, reads and writes never alias within a tick.
        for slot in self.slots.iter_mut() {
            for (j, &buf_idx) in slot.input_buffers.iter().enumerate() {
                slot.input_scratch[j] = pool[buf_idx][ri] * slot.input_scales[j];
            }
            slot.module
                .process(&slot.input_scratch, &mut slot.output_scratch);
            for (j, &buf_idx) in slot.output_buffers.iter().enumerate() {
                pool[buf_idx][wi] = slot.output_scratch[j];
            }
        }
    }

    /// Consume this plan and move all module instances into a [`ModuleInstanceRegistry`].
    ///
    /// The registry can be passed to [`build_patch`] for the next plan so that
    /// module state (e.g. oscillator phase) is preserved across re-plans.
    pub fn into_registry(self) -> ModuleInstanceRegistry {
        let mut registry = ModuleInstanceRegistry::new();
        for slot in self.slots {
            registry.insert(slot.module);
        }
        registry
    }

    /// Left-channel sample produced during the most recent [`tick`](Self::tick).
    pub fn last_left(&self) -> f64 {
        self.slots[self.audio_out_index]
            .module
            .as_sink()
            .map_or(0.0, |s| s.last_left())
    }

    /// Right-channel sample produced during the most recent [`tick`](Self::tick).
    pub fn last_right(&self) -> f64 {
        self.slots[self.audio_out_index]
            .module
            .as_sink()
            .map_or(0.0, |s| s.last_right())
    }
}

/// Consume a [`ModuleGraph`] and produce an [`ExecutionPlan`] with updated [`BufferAllocState`].
///
/// Validates that exactly one `AudioOut` node is present, orders modules by
/// ascending [`NodeId`], and resolves per-module input/output buffer assignments.
/// Unconnected input ports are assigned the permanent-zero buffer (index 0);
/// output ports get a dedicated buffer from the pool.
///
/// ## Stable allocation
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
/// ## Module instance reuse
///
/// If `registry` is `Some`, for each module in the new graph the registry is
/// checked for an existing instance with the same [`InstanceId`]. If found, the
/// old (stateful) instance is used instead of the graph's fresh instance.
pub fn build_patch(
    graph: ModuleGraph,
    mut registry: Option<&mut ModuleInstanceRegistry>,
    alloc: &BufferAllocState,
    pool_capacity: usize,
) -> Result<(ExecutionPlan, BufferAllocState), BuildError> {
    let node_ids = graph.node_ids();
    let edges = graph.edge_list();

    // Snapshot descriptors and instance IDs before consuming the graph.
    struct NodeMeta {
        descriptor: ModuleDescriptor,
        instance_id: patches_core::InstanceId,
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
    //
    // Index 0: permanent-zero slot (never written to, used for unconnected inputs).
    // Indices 1..: cable buffers, allocated stably so that cables surviving a re-plan
    // keep the same index and require no zeroing.
    //
    // `new_freelist` begins as whatever was left over in `alloc.freelist` from
    // previous plans; indices freed in this plan are appended at the end.
    let mut new_freelist: Vec<usize> = alloc.freelist.clone();
    let mut new_hwm: usize = alloc.next_hwm;
    let mut to_zero: Vec<usize> = Vec::new();

    // Map (NodeId, output_port_index) → buffer pool index for this plan.
    let mut output_buf: HashMap<(NodeId, usize), usize> = HashMap::new();

    for id in &order {
        let desc = &meta[id].descriptor;
        for (port_idx, _) in desc.outputs.iter().enumerate() {
            let key = (id.clone(), port_idx);
            if let Some(&existing) = alloc.output_buf.get(&key) {
                // Stable connection: reuse the same buffer index — no zeroing needed.
                output_buf.insert(key, existing);
            } else {
                // New output port: try the freelist first (LIFO), then the hwm.
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

    // Build the module slots in execution order.
    let mut slots: Vec<ModuleSlot> = Vec::with_capacity(order.len());

    for id in &order {
        let desc = &meta[id].descriptor;

        // Resolve (buffer_index, scale) for each input port.
        let (input_buffers, input_scales): (Vec<usize>, Vec<f64>) = desc
            .inputs
            .iter()
            .map(|port| {
                // Find the edge that drives this input port (match on name AND index).
                let (buf_idx, scale) = edges
                    .iter()
                    .find(|(_, _, _, to, in_name, in_idx, _)| {
                        *to == *id && in_name == port.name && *in_idx == port.index
                    })
                    .map(|(from, out_name, out_idx, _, _, _, scale)| -> Result<(usize, f64), BuildError> {
                        // Resolve the driving output's buffer index.
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
                    .unwrap_or((0, 1.0)); // 0 = zero buffer, 1.0 = no scale for unconnected inputs
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

        // Prefer an old (stateful) instance from the registry when available.
        let fresh = modules.remove(id).ok_or_else(|| {
            BuildError::InternalError(format!("module {id:?} missing from map"))
        })?;
        let module = if let Some(reg) = registry.as_deref_mut() {
            reg.take(meta[id].instance_id).unwrap_or(fresh)
        } else {
            fresh
        };

        slots.push(ModuleSlot {
            module,
            input_buffers,
            input_scales,
            output_buffers,
            input_scratch: vec![0.0; n_in],
            output_scratch: vec![0.0; n_out],
        });
    }

    let new_alloc = BufferAllocState {
        output_buf,
        freelist: new_freelist,
        next_hwm: new_hwm,
    };

    Ok((
        ExecutionPlan {
            slots,
            to_zero,
            audio_out_index,
        },
        new_alloc,
    ))
}


#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{NodeId, PortRef};
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

    fn make_pool(pool_capacity: usize) -> Vec<[f64; 2]> {
        vec![[0.0; 2]; pool_capacity]
    }

    fn default_build(graph: ModuleGraph) -> (ExecutionPlan, BufferAllocState) {
        build_patch(graph, None, &BufferAllocState::default(), 256).expect("build should succeed")
    }

    #[test]
    fn builds_minimal_plan_with_correct_order() {
        let (graph, _, _) = sine_to_audio_out_graph();
        let (plan, _) = default_build(graph);

        // AudioOut must be last (after the sine oscillator).
        let audio_out_idx = plan.audio_out_index;
        let sine_idx = plan
            .slots
            .iter()
            .position(|s| {
                s.module
                    .as_any()
                    .downcast_ref::<SineOscillator>()
                    .is_some()
            })
            .expect("sine slot not found");

        assert!(sine_idx < audio_out_idx, "sine must execute before AudioOut");
    }

    #[test]
    fn fanout_buffer_shared_between_both_inputs() {
        let (graph, _, _) = sine_to_audio_out_graph();
        let (plan, _) = default_build(graph);

        let audio_out_idx = plan.audio_out_index;
        let sine_idx = plan
            .slots
            .iter()
            .position(|s| {
                s.module
                    .as_any()
                    .downcast_ref::<SineOscillator>()
                    .is_some()
            })
            .unwrap();

        // Both AudioOut inputs must share the same buffer as SineOscillator's output.
        let sine_out_buf = plan.slots[sine_idx].output_buffers[0];
        let left_buf = plan.slots[audio_out_idx].input_buffers[0];
        let right_buf = plan.slots[audio_out_idx].input_buffers[1];

        assert_eq!(sine_out_buf, left_buf, "left input must use sine output buffer");
        assert_eq!(sine_out_buf, right_buf, "right input must use sine output buffer");
    }

    #[test]
    fn tick_produces_bounded_audio_output() {
        let (graph, _, _) = sine_to_audio_out_graph();
        let (mut plan, _) = default_build(graph);
        plan.initialise(&patches_core::AudioEnvironment { sample_rate: 44100.0 });
        let mut pool = make_pool(256);

        for i in 0..1000 {
            plan.tick(&mut pool, i % 2);
        }

        assert!(plan.last_left().abs() <= 1.0);
        assert!(plan.last_right().abs() <= 1.0);
        // After enough samples a 440 Hz sine will have produced non-zero output.
        assert!(plan.last_left().abs() > 0.0);
    }

    #[test]
    fn no_audio_out_returns_error() {
        let mut graph = ModuleGraph::new();
        graph.add_module("sine", Box::new(SineOscillator::new(440.0))).unwrap();
        assert!(matches!(
            build_patch(graph, None, &BufferAllocState::default(), 256),
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
            build_patch(graph, None, &BufferAllocState::default(), 256),
            Err(BuildError::MultipleAudioOut)
        ));
    }

    #[test]
    fn input_scale_is_applied_at_tick_time() {
        // Build a graph with scale = 0.5 on both connections from the sine oscillator
        // to AudioOut. The output should be half what it would be with scale = 1.0.
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

        let env = patches_core::AudioEnvironment { sample_rate: 44100.0 };
        let (mut plan_half, _) = build_patch(graph_half, None, &BufferAllocState::default(), 256).unwrap();
        let (mut plan_full, _) = build_patch(graph_full, None, &BufferAllocState::default(), 256).unwrap();
        plan_half.initialise(&env);
        plan_full.initialise(&env);
        let mut pool_half = make_pool(256);
        let mut pool_full = make_pool(256);

        // Tick both plans the same number of times so they are in phase.
        for i in 0..100 {
            plan_half.tick(&mut pool_half, i % 2);
            plan_full.tick(&mut pool_full, i % 2);
        }

        let half = plan_half.last_left();
        let full = plan_full.last_left();

        // Both should be non-zero (avoid testing at a zero crossing).
        if full.abs() > 1e-6 {
            let ratio = half / full;
            assert!(
                (ratio - 0.5).abs() < 1e-9,
                "expected half ≈ full * 0.5, got half={half}, full={full}, ratio={ratio}"
            );
        }
    }

    // ── Acceptance criteria: T-0025 ──────────────────────────────────────────

    /// Build plan A then plan B (one module removed, one unchanged); assert:
    /// - The unchanged module's buffer index is identical in both plans.
    /// - The removed module's freed buffer index appears in plan B's `to_zero`.
    #[test]
    fn stable_buffer_index_for_unchanged_module_across_replan() {
        let pool_capacity = 256;
        let alloc0 = BufferAllocState::default();

        // Plan A: sine_a + sine_b + AudioOut.
        let mut graph_a = ModuleGraph::new();
        graph_a.add_module("sine_a", Box::new(SineOscillator::new(440.0))).unwrap();
        graph_a.add_module("sine_b", Box::new(SineOscillator::new(880.0))).unwrap();
        graph_a.add_module("out", Box::new(AudioOut::new())).unwrap();
        let sine_a = NodeId::from("sine_a");
        let sine_b = NodeId::from("sine_b");
        let out_a = NodeId::from("out");
        graph_a.connect(&sine_a, p("out"), &out_a, p("left"), 1.0).unwrap();
        graph_a.connect(&sine_b, p("out"), &out_a, p("right"), 1.0).unwrap();

        let (_plan_a, alloc_a) = build_patch(graph_a, None, &alloc0, pool_capacity).unwrap();

        // Find sine_a's buffer index in plan A via the alloc state.
        let buf_a = alloc_a.output_buf[&(NodeId::from("sine_a"), 0)];

        // Plan B: same sine_a (same NodeId) + AudioOut, but sine_b is gone.
        let mut graph_b = ModuleGraph::new();
        graph_b.add_module("sine_a", Box::new(SineOscillator::new(440.0))).unwrap();
        graph_b.add_module("out", Box::new(AudioOut::new())).unwrap();
        let sine_a_b = NodeId::from("sine_a");
        let out_b = NodeId::from("out");
        graph_b.connect(&sine_a_b, p("out"), &out_b, p("left"), 1.0).unwrap();
        graph_b.connect(&sine_a_b, p("out"), &out_b, p("right"), 1.0).unwrap();

        let (plan_b, alloc_b) = build_patch(graph_b, None, &alloc_a, pool_capacity).unwrap();

        let buf_b = alloc_b.output_buf[&(NodeId::from("sine_a"), 0)];

        assert_eq!(
            buf_a, buf_b,
            "sine_a output buffer must be identical across re-plan (stable allocation)"
        );

        // sine_b's buffer should be freed and appear in plan_b.to_zero.
        let freed_buf = alloc_a.output_buf[&(NodeId::from("sine_b"), 0)];
        assert!(
            plan_b.to_zero.contains(&freed_buf),
            "freed buffer index {freed_buf} must appear in plan_b.to_zero (got {:?})",
            plan_b.to_zero
        );
    }

    /// Run many re-plans that alternate between adding and removing a module.
    /// Assert that `next_hwm` does not grow unboundedly — the freelist recycles
    /// freed indices before the hwm is ever incremented.
    #[test]
    fn freelist_recycles_indices_preventing_hwm_growth() {
        let pool_capacity = 256;

        // Plan type A: two oscillators + AudioOut  → allocates 2 buffer indices.
        // Plan type B: one oscillator  + AudioOut  → one buffer freed to freelist.
        // Cycling A → B → A → B … must keep hwm constant after the first A.

        let build_two = |alloc: &BufferAllocState| {
            let mut g = ModuleGraph::new();
            let s1 = NodeId::from("s1");
            let s2 = NodeId::from("s2");
            let out = NodeId::from("out");
            g.add_module(s1.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
            g.add_module(s2.clone(), Box::new(SineOscillator::new(880.0))).unwrap();
            g.add_module(out.clone(), Box::new(AudioOut::new())).unwrap();
            g.connect(&s1, p("out"), &out, p("left"), 1.0).unwrap();
            g.connect(&s2, p("out"), &out, p("right"), 1.0).unwrap();
            build_patch(g, None, alloc, pool_capacity).unwrap().1
        };

        let build_one = |alloc: &BufferAllocState| {
            let mut g = ModuleGraph::new();
            let s = NodeId::from("s1");
            let out = NodeId::from("out");
            g.add_module(s.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
            g.add_module(out.clone(), Box::new(AudioOut::new())).unwrap();
            g.connect(&s, p("out"), &out, p("left"), 1.0).unwrap();
            g.connect(&s, p("out"), &out, p("right"), 1.0).unwrap();
            build_patch(g, None, alloc, pool_capacity).unwrap().1
        };

        let alloc_a = build_two(&BufferAllocState::default());
        let hwm_after_first_two = alloc_a.next_hwm; // should be 3

        let mut current = alloc_a;
        for _ in 0..20 {
            current = build_one(&current);  // free one index → freelist grows
            current = build_two(&current);  // reuse from freelist → hwm stays
        }

        assert_eq!(
            current.next_hwm, hwm_after_first_two,
            "hwm grew from {hwm_after_first_two} to {}: freelist should have prevented new allocations",
            current.next_hwm
        );
    }

    #[test]
    fn pool_exhausted_error_when_capacity_exceeded() {
        // Pool capacity of 2 leaves only index 1 (index 0 is the zero slot).
        // Building a graph with any output ports should exhaust it.
        let mut graph = ModuleGraph::new();
        let sine = NodeId::from("sine");
        let out = NodeId::from("out");
        graph.add_module(sine.clone(), Box::new(SineOscillator::new(440.0))).unwrap();
        graph.add_module(out.clone(), Box::new(AudioOut::new())).unwrap();
        graph.connect(&sine, p("out"), &out, p("left"), 1.0).unwrap();
        graph.connect(&sine, p("out"), &out, p("right"), 1.0).unwrap();

        // capacity=1 means only the zero slot exists; any allocation will fail.
        assert!(matches!(
            build_patch(graph, None, &BufferAllocState::default(), 1),
            Err(BuildError::PoolExhausted)
        ));
    }
}
