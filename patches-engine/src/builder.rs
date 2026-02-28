use std::collections::{HashMap, VecDeque};
use std::fmt;

use patches_core::{Module, ModuleDescriptor, ModuleGraph, NodeId, SampleBuffer};
use patches_modules::AudioOut;

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
    /// Indices into the [`ExecutionPlan`] buffer pool — one per output port.
    pub output_buffers: Vec<usize>,
    /// Pre-allocated scratch space for reading input values before `process`.
    pub input_scratch: Vec<f64>,
    /// Pre-allocated scratch space for `process` to write output values into.
    pub output_scratch: Vec<f64>,
}

/// A fully resolved, allocation-free execution structure produced by [`build_patch`].
///
/// Call [`tick`](ExecutionPlan::tick) once per sample on the audio thread.
/// After each tick, retrieve the stereo output via [`last_left`](ExecutionPlan::last_left)
/// and [`last_right`](ExecutionPlan::last_right).
pub struct ExecutionPlan {
    pub slots: Vec<ModuleSlot>,
    pub buffers: Vec<SampleBuffer>,
    pub audio_out_index: usize,
}

impl ExecutionPlan {
    /// Process one sample across all modules in execution order.
    ///
    /// Does not allocate.
    pub fn tick(&mut self, sample_rate: f64) {
        let Self { slots, buffers, .. } = self;

        // Phase 1: gather input values from the buffer pool into per-slot scratch.
        for slot in slots.iter_mut() {
            for (j, &buf_idx) in slot.input_buffers.iter().enumerate() {
                slot.input_scratch[j] = buffers[buf_idx].read();
            }
        }

        // Phase 2: run each module.
        for slot in slots.iter_mut() {
            slot.module
                .process(&slot.input_scratch, &mut slot.output_scratch, sample_rate);
        }

        // Phase 3: write output scratch values back into the buffer pool.
        for slot in slots.iter_mut() {
            for (j, &buf_idx) in slot.output_buffers.iter().enumerate() {
                buffers[buf_idx].write(slot.output_scratch[j]);
            }
        }

        // Phase 4: advance all buffers (rotate the write slot).
        for buf in buffers.iter_mut() {
            buf.advance();
        }
    }

    /// Left-channel sample produced during the most recent [`tick`](Self::tick).
    pub fn last_left(&self) -> f64 {
        self.slots[self.audio_out_index]
            .module
            .as_any()
            .downcast_ref::<AudioOut>()
            .map_or(0.0, |a| a.last_left())
    }

    /// Right-channel sample produced during the most recent [`tick`](Self::tick).
    pub fn last_right(&self) -> f64 {
        self.slots[self.audio_out_index]
            .module
            .as_any()
            .downcast_ref::<AudioOut>()
            .map_or(0.0, |a| a.last_right())
    }
}

/// Consume a [`ModuleGraph`] and produce an [`ExecutionPlan`].
///
/// Validates that exactly one `AudioOut` node is present, performs a
/// cycle-tolerant topological sort (Kahn's algorithm), allocates one
/// [`SampleBuffer`] per output port, and resolves per-module input/output
/// buffer assignments. Unconnected input ports are assigned a permanent-zero
/// buffer; all output ports (connected or not) get a dedicated buffer.
pub fn build_patch(graph: ModuleGraph) -> Result<ExecutionPlan, BuildError> {
    let node_ids = graph.node_ids();
    let edges = graph.edge_list();

    // Snapshot descriptors before consuming the graph.
    let descriptors: HashMap<NodeId, ModuleDescriptor> = node_ids
        .iter()
        .map(|&id| {
            graph
                .get_module(id)
                .ok_or_else(|| {
                    BuildError::InternalError(format!("node {id:?} missing from graph"))
                })
                .map(|m| (id, m.descriptor().clone()))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;

    // Identify AudioOut nodes by downcasting.
    let audio_out_ids: Vec<NodeId> = node_ids
        .iter()
        .filter(|&&id| {
            graph
                .get_module(id)
                .and_then(|m| m.as_any().downcast_ref::<AudioOut>())
                .is_some()
        })
        .copied()
        .collect();

    let audio_out_node = match audio_out_ids.len() {
        0 => return Err(BuildError::NoAudioOut),
        1 => audio_out_ids[0],
        _ => return Err(BuildError::MultipleAudioOut),
    };

    // Topological sort — cycle-tolerant via Kahn's algorithm.
    let order = kahn_toposort(&node_ids, &edges)?;

    let audio_out_index = order
        .iter()
        .position(|&id| id == audio_out_node)
        .ok_or_else(|| {
            BuildError::InternalError("audio_out node missing from toposort result".to_string())
        })?;

    // Consume the graph's modules.
    let mut modules = graph.into_modules();

    // Buffer pool.
    // Index 0: permanent-zero buffer (for unconnected input ports — never written to).
    // Indices 1..: one buffer per output port of each module, in execution order.
    let mut buffers: Vec<SampleBuffer> = vec![SampleBuffer::new()];

    // Map (NodeId, output_port_index) → buffer pool index.
    let mut output_buf: HashMap<(NodeId, usize), usize> = HashMap::new();

    for &id in &order {
        let desc = &descriptors[&id];
        for (port_idx, _) in desc.outputs.iter().enumerate() {
            let buf_idx = buffers.len();
            buffers.push(SampleBuffer::new());
            output_buf.insert((id, port_idx), buf_idx);
        }
    }

    // Build the module slots in execution order.
    let mut slots: Vec<ModuleSlot> = Vec::with_capacity(order.len());

    for &id in &order {
        let desc = &descriptors[&id];

        let input_buffers: Vec<usize> = desc
            .inputs
            .iter()
            .map(|port| {
                // Find the edge that drives this input port.
                let buf_idx = edges
                    .iter()
                    .find(|(_, _, to, input)| *to == id && input == port.name)
                    .map(|(from, out_name, _, _)| -> Result<usize, BuildError> {
                        // Resolve the driving output's buffer index.
                        let from_desc = &descriptors[from];
                        let out_port_idx = from_desc
                            .outputs
                            .iter()
                            .position(|p| p.name == out_name)
                            .ok_or_else(|| {
                                BuildError::InternalError(format!(
                                    "output port {out_name:?} not found on node {from:?}"
                                ))
                            })?;
                        Ok(output_buf[&(*from, out_port_idx)])
                    })
                    .transpose()?
                    .unwrap_or(0); // 0 = zero buffer for unconnected inputs
                Ok(buf_idx)
            })
            .collect::<Result<Vec<_>, BuildError>>()?;

        let output_buffers: Vec<usize> = desc
            .outputs
            .iter()
            .enumerate()
            .map(|(port_idx, _)| output_buf[&(id, port_idx)])
            .collect();

        let n_in = desc.inputs.len();
        let n_out = desc.outputs.len();
        let module = modules.remove(&id).ok_or_else(|| {
            BuildError::InternalError(format!("module {id:?} missing from map"))
        })?;

        slots.push(ModuleSlot {
            module,
            input_buffers,
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

/// Cycle-tolerant topological sort using Kahn's algorithm.
///
/// Nodes remaining after the main queue empties (i.e. in cycles) are appended
/// in ascending `NodeId` order. The 1-sample `SampleBuffer` delay makes the
/// chosen order safe for cyclic graphs.
fn kahn_toposort(
    node_ids: &[NodeId],
    edges: &[(NodeId, String, NodeId, String)],
) -> Result<Vec<NodeId>, BuildError> {
    let mut in_degree: HashMap<NodeId, usize> =
        node_ids.iter().map(|&id| (id, 0)).collect();

    for (_, _, to, _) in edges {
        *in_degree.get_mut(to).ok_or_else(|| {
            BuildError::InternalError(format!("edge target {to:?} not in node set"))
        })? += 1;
    }

    // Initialise the queue with zero-in-degree nodes, sorted for determinism.
    let mut queue: VecDeque<NodeId> = {
        let mut v: Vec<NodeId> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();
        v.sort_unstable();
        v.into_iter().collect()
    };

    let mut order: Vec<NodeId> = Vec::with_capacity(node_ids.len());

    while let Some(node) = queue.pop_front() {
        order.push(node);

        // Decrement in-degree for each unique successor.
        let mut successors: Vec<NodeId> = edges
            .iter()
            .filter(|(from, _, _, _)| *from == node)
            .map(|(_, _, to, _)| *to)
            .collect();
        successors.sort_unstable();
        successors.dedup();

        for succ in successors {
            let deg = in_degree.get_mut(&succ).ok_or_else(|| {
                BuildError::InternalError(format!("successor {succ:?} not in node set"))
            })?;
            *deg -= 1;
            if *deg == 0 {
                queue.push_back(succ);
            }
        }
    }

    // Append cycle participants in deterministic order.
    let mut remaining: Vec<NodeId> = node_ids
        .iter()
        .filter(|id| !order.contains(id))
        .copied()
        .collect();
    remaining.sort_unstable();
    order.extend(remaining);

    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_modules::{AudioOut, SineOscillator};

    fn sine_to_audio_out_graph() -> (ModuleGraph, NodeId, NodeId) {
        let mut graph = ModuleGraph::new();
        let sine_id = graph.add_module(Box::new(SineOscillator::new(440.0)));
        let out_id = graph.add_module(Box::new(AudioOut::new()));
        graph.connect(sine_id, "out", out_id, "left").unwrap();
        graph.connect(sine_id, "out", out_id, "right").unwrap();
        (graph, sine_id, out_id)
    }

    #[test]
    fn builds_minimal_plan_with_correct_order() {
        let (graph, _, _) = sine_to_audio_out_graph();
        let plan = build_patch(graph).expect("build should succeed");

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
        let plan = build_patch(graph).expect("build should succeed");

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
        let mut plan = build_patch(graph).expect("build should succeed");

        for _ in 0..1000 {
            plan.tick(44100.0);
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
        assert!(matches!(build_patch(graph), Err(BuildError::NoAudioOut)));
    }

    #[test]
    fn multiple_audio_out_returns_error() {
        let mut graph = ModuleGraph::new();
        let sine_id = graph.add_module(Box::new(SineOscillator::new(440.0)));
        let out1 = graph.add_module(Box::new(AudioOut::new()));
        let out2 = graph.add_module(Box::new(AudioOut::new()));
        graph.connect(sine_id, "out", out1, "left").unwrap();
        graph.connect(sine_id, "out", out1, "right").unwrap();
        graph.connect(sine_id, "out", out2, "left").unwrap();
        graph.connect(sine_id, "out", out2, "right").unwrap();
        assert!(matches!(
            build_patch(graph),
            Err(BuildError::MultipleAudioOut)
        ));
    }
}
