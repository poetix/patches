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
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::NoAudioOut => write!(f, "patch graph has no AudioOut node"),
            BuildError::MultipleAudioOut => {
                write!(f, "patch graph has more than one AudioOut node")
            }
            BuildError::InternalError(msg) => write!(f, "internal builder error: {msg}"),
        }
    }
}

impl std::error::Error for BuildError {}

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
/// alternating `wi = 0` and `wi = 1` on successive calls.
/// After each tick, retrieve the stereo output via [`last_left`](ExecutionPlan::last_left)
/// and [`last_right`](ExecutionPlan::last_right).
pub struct ExecutionPlan {
    pub slots: Vec<ModuleSlot>,
    /// Flat cable buffer pool. Each element is a 2-element ring `[slot0, slot1]`.
    pub buffers: Vec<[f64; 2]>,
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
    /// `wi` is the write slot index (0 or 1); the read slot is `1 - wi`.
    /// Callers must alternate between `wi = 0` and `wi = 1` on successive calls.
    ///
    /// Does not allocate.
    pub fn tick(&mut self, wi: usize) {
        let ri = 1 - wi;
        let Self { slots, buffers, .. } = self;

        // Per slot: read inputs → process → write outputs.
        // Reading uses `ri` (previous tick's slot); writing uses `wi` (this tick's slot).
        // Because ri ≠ wi, reads and writes never alias within a tick.
        for slot in slots.iter_mut() {
            for (j, &buf_idx) in slot.input_buffers.iter().enumerate() {
                slot.input_scratch[j] = buffers[buf_idx][ri] * slot.input_scales[j];
            }
            slot.module
                .process(&slot.input_scratch, &mut slot.output_scratch);
            for (j, &buf_idx) in slot.output_buffers.iter().enumerate() {
                buffers[buf_idx][wi] = slot.output_scratch[j];
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

/// Consume a [`ModuleGraph`] and produce an [`ExecutionPlan`].
///
/// Validates that exactly one `AudioOut` node is present, orders modules by
/// ascending [`NodeId`], allocates one buffer per output port, and resolves
/// per-module input/output buffer assignments. Unconnected input ports are
/// assigned a permanent-zero buffer; all output ports (connected or not) get a
/// dedicated buffer. The 1-sample cable delay makes any module ordering produce
/// correct output.
///
/// If `registry` is `Some`, for each module in the new graph the registry is
/// checked for an existing instance with the same [`InstanceId`]. If found, the
/// old (stateful) instance is used instead of the graph's fresh instance. Modules
/// not matched in the registry are left in it and will be dropped with the registry.
pub fn build_patch(
    graph: ModuleGraph,
    mut registry: Option<&mut ModuleInstanceRegistry>,
) -> Result<ExecutionPlan, BuildError> {
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
        .map(|&id| {
            graph
                .get_module(id)
                .ok_or_else(|| {
                    BuildError::InternalError(format!("node {id:?} missing from graph"))
                })
                .map(|m| {
                    (
                        id,
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
        .filter(|&&id| meta[&id].is_sink)
        .copied()
        .collect();

    let audio_out_node = match audio_out_ids.len() {
        0 => return Err(BuildError::NoAudioOut),
        1 => audio_out_ids[0],
        _ => return Err(BuildError::MultipleAudioOut),
    };

    // Execution order: ascending NodeId (insertion order). The 1-sample cable
    // delay makes any ordering correct; ascending NodeId gives stable, deterministic output.
    let mut order = node_ids.clone();
    order.sort_unstable();

    let audio_out_index = order
        .iter()
        .position(|&id| id == audio_out_node)
        .ok_or_else(|| {
            BuildError::InternalError("audio_out node missing from order".to_string())
        })?;

    // Consume the graph's modules.
    let mut modules = graph.into_modules();

    // Buffer pool.
    // Index 0: permanent-zero buffer (for unconnected input ports — never written to).
    // Indices 1..: one buffer per output port of each module, in execution order.
    let mut buffers: Vec<[f64; 2]> = vec![[0.0; 2]];

    // Map (NodeId, output_port_index) → buffer pool index.
    let mut output_buf: HashMap<(NodeId, usize), usize> = HashMap::new();

    for &id in &order {
        let desc = &meta[&id].descriptor;
        for (port_idx, _) in desc.outputs.iter().enumerate() {
            let buf_idx = buffers.len();
            buffers.push([0.0; 2]);
            output_buf.insert((id, port_idx), buf_idx);
        }
    }

    // Build the module slots in execution order.
    let mut slots: Vec<ModuleSlot> = Vec::with_capacity(order.len());

    for &id in &order {
        let desc = &meta[&id].descriptor;

        // Resolve (buffer_index, scale) for each input port.
        let (input_buffers, input_scales): (Vec<usize>, Vec<f64>) = desc
            .inputs
            .iter()
            .map(|port| {
                // Find the edge that drives this input port.
                let (buf_idx, scale) = edges
                    .iter()
                    .find(|(_, _, to, input, _)| *to == id && input == port.name)
                    .map(|(from, out_name, _, _, scale)| -> Result<(usize, f64), BuildError> {
                        // Resolve the driving output's buffer index.
                        let from_desc = &meta[from].descriptor;
                        let out_port_idx = from_desc
                            .outputs
                            .iter()
                            .position(|p| p.name == out_name)
                            .ok_or_else(|| {
                                BuildError::InternalError(format!(
                                    "output port {out_name:?} not found on node {from:?}"
                                ))
                            })?;
                        Ok((output_buf[&(*from, out_port_idx)], *scale))
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
            .map(|(port_idx, _)| output_buf[&(id, port_idx)])
            .collect();

        let n_in = desc.inputs.len();
        let n_out = desc.outputs.len();

        // Prefer an old (stateful) instance from the registry when available.
        let fresh = modules.remove(&id).ok_or_else(|| {
            BuildError::InternalError(format!("module {id:?} missing from map"))
        })?;
        let module = if let Some(reg) = registry.as_deref_mut() {
            reg.take(meta[&id].instance_id).unwrap_or(fresh)
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

    Ok(ExecutionPlan {
        slots,
        buffers,
        audio_out_index,
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use patches_modules::{AudioOut, SineOscillator};

    fn sine_to_audio_out_graph() -> (ModuleGraph, NodeId, NodeId) {
        let mut graph = ModuleGraph::new();
        let sine_id = graph.add_module(Box::new(SineOscillator::new(440.0)));
        let out_id = graph.add_module(Box::new(AudioOut::new()));
        graph.connect(sine_id, "out", out_id, "left", 1.0).unwrap();
        graph.connect(sine_id, "out", out_id, "right", 1.0).unwrap();
        (graph, sine_id, out_id)
    }

    #[test]
    fn builds_minimal_plan_with_correct_order() {
        let (graph, _, _) = sine_to_audio_out_graph();
        let plan = build_patch(graph, None).expect("build should succeed");

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
        let plan = build_patch(graph, None).expect("build should succeed");

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
        let mut plan = build_patch(graph, None).expect("build should succeed");
        plan.initialise(&patches_core::AudioEnvironment { sample_rate: 44100.0 });

        for i in 0..1000 {
            plan.tick(i % 2);
        }

        assert!(plan.last_left().abs() <= 1.0);
        assert!(plan.last_right().abs() <= 1.0);
        // After enough samples a 440 Hz sine will have produced non-zero output.
        assert!(plan.last_left().abs() > 0.0);
    }

    #[test]
    fn no_audio_out_returns_error() {
        let mut graph = ModuleGraph::new();
        graph.add_module(Box::new(SineOscillator::new(440.0)));
        assert!(matches!(build_patch(graph, None), Err(BuildError::NoAudioOut)));
    }

    #[test]
    fn multiple_audio_out_returns_error() {
        let mut graph = ModuleGraph::new();
        let sine_id = graph.add_module(Box::new(SineOscillator::new(440.0)));
        let out1 = graph.add_module(Box::new(AudioOut::new()));
        let out2 = graph.add_module(Box::new(AudioOut::new()));
        graph.connect(sine_id, "out", out1, "left", 1.0).unwrap();
        graph.connect(sine_id, "out", out1, "right", 1.0).unwrap();
        graph.connect(sine_id, "out", out2, "left", 1.0).unwrap();
        graph.connect(sine_id, "out", out2, "right", 1.0).unwrap();
        assert!(matches!(
            build_patch(graph, None),
            Err(BuildError::MultipleAudioOut)
        ));
    }

    #[test]
    fn input_scale_is_applied_at_tick_time() {
        // Build a graph with scale = 0.5 on both connections from the sine oscillator
        // to AudioOut. The output should be half what it would be with scale = 1.0.
        let mut graph_half = ModuleGraph::new();
        let sine_h = graph_half.add_module(Box::new(SineOscillator::new(440.0)));
        let out_h = graph_half.add_module(Box::new(patches_modules::AudioOut::new()));
        graph_half.connect(sine_h, "out", out_h, "left", 0.5).unwrap();
        graph_half.connect(sine_h, "out", out_h, "right", 0.5).unwrap();

        let mut graph_full = ModuleGraph::new();
        let sine_f = graph_full.add_module(Box::new(SineOscillator::new(440.0)));
        let out_f = graph_full.add_module(Box::new(patches_modules::AudioOut::new()));
        graph_full.connect(sine_f, "out", out_f, "left", 1.0).unwrap();
        graph_full.connect(sine_f, "out", out_f, "right", 1.0).unwrap();

        let env = patches_core::AudioEnvironment { sample_rate: 44100.0 };
        let mut plan_half = build_patch(graph_half, None).unwrap();
        let mut plan_full = build_patch(graph_full, None).unwrap();
        plan_half.initialise(&env);
        plan_full.initialise(&env);

        // Tick both plans the same number of times so they are in phase.
        for i in 0..100 {
            plan_half.tick(i % 2);
            plan_full.tick(i % 2);
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
}
