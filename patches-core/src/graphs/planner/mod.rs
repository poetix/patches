use std::collections::HashMap;
use std::fmt;

use crate::modules::{InstanceId, ModuleShape, ParameterMap, PortConnectivity};
use super::graph::{ModuleGraph, NodeId};

pub mod alloc;
pub mod graph_index;

pub use alloc::{allocate_buffers, BufferAllocState, BufferAllocation, ModuleAllocDiff, ModuleAllocState};
pub use graph_index::{GraphIndex, ResolvedGraph};

// ── PlanError ─────────────────────────────────────────────────────────────────

/// Errors that can occur during the decision phase of plan building.
#[derive(Debug)]
pub enum PlanError {
    /// The graph contains no sink node.
    NoSink,
    /// The graph contains more than one sink node.
    MultipleSinks,
    /// The number of output ports would exceed the buffer pool capacity.
    BufferPoolExhausted,
    /// The number of modules would exceed the module pool capacity.
    ModulePoolExhausted,
    /// An internal consistency invariant was violated (indicates a bug in the builder).
    Internal(String),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::NoSink => write!(f, "patch graph has no sink node"),
            PlanError::MultipleSinks => write!(f, "patch graph has more than one sink node"),
            PlanError::BufferPoolExhausted => {
                write!(f, "buffer pool exhausted: too many output ports")
            }
            PlanError::ModulePoolExhausted => {
                write!(f, "module pool exhausted: too many modules")
            }
            PlanError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for PlanError {}

// ── NodeState ─────────────────────────────────────────────────────────────────

/// Per-node identity and parameter state carried across successive builds.
pub struct NodeState {
    /// The module type name (from `ModuleDescriptor::module_name`).
    pub module_name: &'static str,
    /// Stable identity assigned by the planner when this node first appeared.
    pub instance_id: InstanceId,
    /// The parameter map applied to this node during the last build.
    pub parameter_map: ParameterMap,
    /// The shape used when this module instance was created.
    ///
    /// If the shape changes on the next build (same `NodeId`, same module type),
    /// the old instance is tombstoned and a fresh one is created with the new shape.
    pub shape: ModuleShape,
    /// The port connectivity computed during the last build.
    ///
    /// Stored so that the engine can diff against it to emit connectivity updates only
    /// when the wiring actually changes.
    pub connectivity: PortConnectivity,
}

// ── PlannerState ──────────────────────────────────────────────────────────────

/// Planning state threaded across successive plan builds.
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

// ── NodeDecision ──────────────────────────────────────────────────────────────

/// Per-node decision produced by [`classify_nodes`].
///
/// The decision phase is pure: it reads the graph and previous state but does
/// not mint [`InstanceId`]s or call `registry.create`. Both side effects happen
/// in the action phase that follows.
pub enum NodeDecision<'a> {
    /// Node is new, or its module type or shape changed.
    /// A fresh module must be instantiated in the action phase.
    Install {
        module_name: &'static str,
        shape: &'a ModuleShape,
        params: &'a ParameterMap,
    },
    /// Node is surviving. The existing module stays in the pool.
    /// Non-empty `param_diff` or `connectivity_changed == true` means diffs
    /// must be applied on plan adoption.
    Update {
        instance_id: InstanceId,
        param_diff: ParameterMap,
        connectivity_changed: bool,
    },
}

// ── PlanDecisions ─────────────────────────────────────────────────────────────

/// Everything produced by [`make_decisions`] and consumed by the action phase
/// of the builder in `patches-engine`.
pub struct PlanDecisions<'a> {
    pub index: GraphIndex<'a>,
    pub order: Vec<NodeId>,
    pub audio_out_index: usize,
    pub buf_alloc: BufferAllocation,
    pub decisions: Vec<(NodeId, NodeDecision<'a>)>,
}

// ── classify_nodes ────────────────────────────────────────────────────────────

/// Classify every node in `order` as [`NodeDecision::Install`] or [`NodeDecision::Update`]
/// by diffing against `prev_state`.
///
/// - A node absent from `prev_state.nodes` → `Install`.
/// - A node whose `module_name` or `shape` changed → `Install`.
/// - Otherwise → `Update`, with a key-by-key parameter diff and a boolean
///   indicating whether the computed [`PortConnectivity`] changed.
///
/// Pure: no [`InstanceId`]s are minted, no modules are instantiated.
pub fn classify_nodes<'a>(
    index: &GraphIndex<'a>,
    order: &[NodeId],
    prev_state: &PlannerState,
) -> Result<Vec<(NodeId, NodeDecision<'a>)>, PlanError> {
    let mut decisions = Vec::with_capacity(order.len());

    for id in order {
        let node = index.get_node(id).ok_or_else(|| {
            PlanError::Internal(format!("node {id:?} missing from graph"))
        })?;
        let desc = &node.module_descriptor;

        let decision = match prev_state.nodes.get(id) {
            Some(prev_ns)
                if prev_ns.module_name == desc.module_name && prev_ns.shape == desc.shape =>
            {
                // Surviving node: compute parameter diff and connectivity diff.
                let param_diff: ParameterMap = node
                    .parameter_map
                    .iter()
                    .filter(|(k, v)| prev_ns.parameter_map.get(*k) != Some(v))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let new_connectivity = index.compute_connectivity(desc, id);
                let connectivity_changed = new_connectivity != prev_ns.connectivity;
                NodeDecision::Update { instance_id: prev_ns.instance_id, param_diff, connectivity_changed }
            }
            _ => {
                // New, type-changed, or shape-changed node → fresh installation.
                NodeDecision::Install {
                    module_name: desc.module_name,
                    shape: &desc.shape,
                    params: &node.parameter_map,
                }
            }
        };

        decisions.push((id.clone(), decision));
    }

    Ok(decisions)
}

// ── make_decisions ────────────────────────────────────────────────────────────

/// Index the graph, sort nodes into execution order, allocate cable buffers,
/// and classify every node as [`NodeDecision::Install`] or [`NodeDecision::Update`].
///
/// This is the pure decision phase: no [`InstanceId`]s are minted and no modules
/// are instantiated. Those side-effects happen in the action phase performed by
/// the builder in `patches-engine`.
pub fn make_decisions<'a>(
    graph: &'a ModuleGraph,
    prev_state: &PlannerState,
    pool_capacity: usize,
) -> Result<PlanDecisions<'a>, PlanError> {
    let index = GraphIndex::build(graph);
    let node_ids = graph.node_ids();
    let sink = find_sink(graph, &node_ids)?;
    let (order, audio_out_index) = compute_order(&node_ids, &sink)?;
    let buf_alloc = allocate_buffers(&index, &order, &prev_state.buffer_alloc, pool_capacity)?;
    let decisions = classify_nodes(&index, &order, prev_state)?;
    Ok(PlanDecisions { index, order, audio_out_index, buf_alloc, decisions })
}

fn find_sink(graph: &ModuleGraph, node_ids: &[NodeId]) -> Result<NodeId, PlanError> {
    let sinks: Vec<NodeId> = node_ids
        .iter()
        .filter(|id| {
            graph.get_node(id).map(|n| n.module_descriptor.is_sink).unwrap_or(false)
        })
        .cloned()
        .collect();
    match sinks.len() {
        0 => Err(PlanError::NoSink),
        1 => Ok(sinks.into_iter().next().unwrap()),
        _ => Err(PlanError::MultipleSinks),
    }
}

fn compute_order(node_ids: &[NodeId], sink: &NodeId) -> Result<(Vec<NodeId>, usize), PlanError> {
    let mut order = node_ids.to_vec();
    order.sort_unstable();
    let audio_out_index = order
        .iter()
        .position(|id| id == sink)
        .ok_or_else(|| PlanError::Internal("sink node missing from order".to_string()))?;
    Ok((order, audio_out_index))
}

// ── classify_nodes tests (T-0099) ────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{InstanceId, ModuleDescriptor, ParameterValue, PortDescriptor, PortRef};
    use crate::ModuleGraph;

    fn p(name: &'static str) -> PortRef {
        PortRef { name, index: 0 }
    }

    fn osc_desc() -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Oscillator",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![],
            outputs: vec![PortDescriptor { name: "sine", index: 0 }],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn sink_desc() -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "AudioOut",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![
                PortDescriptor { name: "left", index: 0 },
                PortDescriptor { name: "right", index: 0 },
            ],
            outputs: vec![],
            parameters: vec![],
            is_sink: true,
        }
    }

    fn multi_in_desc(module_name: &'static str, in_count: usize, shape: ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name,
            shape,
            inputs: (0..in_count as u32).map(|i| PortDescriptor { name: "in", index: i }).collect(),
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prev_with_node(
        node_id: &NodeId,
        module_name: &'static str,
        shape: ModuleShape,
        params: ParameterMap,
        connectivity: PortConnectivity,
    ) -> PlannerState {
        let instance_id = InstanceId::next();
        let mut state = PlannerState::empty();
        state.nodes.insert(
            node_id.clone(),
            NodeState {
                module_name,
                instance_id,
                parameter_map: params,
                shape,
                connectivity,
            },
        );
        state
    }

    #[test]
    fn classify_new_node_is_install() {
        let desc = osc_desc();
        let mut params = ParameterMap::new();
        params.insert("frequency".to_string(), ParameterValue::Float(440.0));
        let mut graph = ModuleGraph::new();
        graph.add_module("osc", desc, &params).unwrap();

        let order = vec![NodeId::from("osc")];
        let index = GraphIndex::build(&graph);
        let decisions = classify_nodes(&index, &order, &PlannerState::empty()).unwrap();

        assert_eq!(decisions.len(), 1);
        match &decisions[0].1 {
            NodeDecision::Install { module_name, .. } => {
                assert_eq!(*module_name, "Oscillator");
            }
            NodeDecision::Update { .. } => panic!("expected Install"),
        }
    }

    #[test]
    fn classify_type_changed_node_is_install() {
        let mut graph = ModuleGraph::new();
        graph.add_module("x", sink_desc(), &ParameterMap::new()).unwrap();

        let od = osc_desc();
        let prev = prev_with_node(
            &NodeId::from("x"),
            od.module_name,
            od.shape,
            ParameterMap::new(),
            PortConnectivity::new(od.inputs.len(), od.outputs.len()),
        );

        let order = vec![NodeId::from("x")];
        let index = GraphIndex::build(&graph);
        let decisions = classify_nodes(&index, &order, &prev).unwrap();
        assert!(matches!(decisions[0].1, NodeDecision::Install { .. }));
    }

    #[test]
    fn classify_shape_changed_node_is_install() {
        let new_shape = ModuleShape { channels: 2, length: 0 };
        let old_shape = ModuleShape { channels: 1, length: 0 };
        let new_desc = multi_in_desc("Sum", 2, new_shape);
        let old_desc = multi_in_desc("Sum", 1, old_shape.clone());

        let mut graph = ModuleGraph::new();
        graph.add_module("s", new_desc, &ParameterMap::new()).unwrap();

        let prev = prev_with_node(
            &NodeId::from("s"),
            "Sum",
            old_shape,
            ParameterMap::new(),
            PortConnectivity::new(old_desc.inputs.len(), old_desc.outputs.len()),
        );

        let order = vec![NodeId::from("s")];
        let index = GraphIndex::build(&graph);
        let decisions = classify_nodes(&index, &order, &prev).unwrap();
        assert!(matches!(decisions[0].1, NodeDecision::Install { .. }));
    }

    #[test]
    fn classify_surviving_no_changes_is_update_with_empty_diff() {
        let desc = sink_desc();
        let mut graph = ModuleGraph::new();
        graph.add_module("out", desc.clone(), &ParameterMap::new()).unwrap();

        let prev = prev_with_node(
            &NodeId::from("out"),
            desc.module_name,
            desc.shape,
            ParameterMap::new(),
            PortConnectivity::new(desc.inputs.len(), desc.outputs.len()),
        );

        let order = vec![NodeId::from("out")];
        let index = GraphIndex::build(&graph);
        let decisions = classify_nodes(&index, &order, &prev).unwrap();

        match &decisions[0].1 {
            NodeDecision::Update { param_diff, connectivity_changed, .. } => {
                assert!(param_diff.is_empty());
                assert!(!connectivity_changed);
            }
            NodeDecision::Install { .. } => panic!("expected Update"),
        }
    }

    #[test]
    fn classify_surviving_param_changed_produces_diff() {
        let desc = osc_desc();
        let mut old_params = ParameterMap::new();
        old_params.insert("frequency".to_string(), ParameterValue::Float(440.0));
        let mut new_params = ParameterMap::new();
        new_params.insert("frequency".to_string(), ParameterValue::Float(880.0));

        let mut graph = ModuleGraph::new();
        graph.add_module("osc", desc.clone(), &new_params).unwrap();

        let prev = prev_with_node(
            &NodeId::from("osc"),
            desc.module_name,
            desc.shape,
            old_params,
            PortConnectivity::new(desc.inputs.len(), desc.outputs.len()),
        );

        let order = vec![NodeId::from("osc")];
        let index = GraphIndex::build(&graph);
        let decisions = classify_nodes(&index, &order, &prev).unwrap();

        match &decisions[0].1 {
            NodeDecision::Update { param_diff, .. } => {
                assert!(!param_diff.is_empty());
                assert_eq!(param_diff.get("frequency"), Some(&ParameterValue::Float(880.0)));
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn classify_surviving_edge_added_connectivity_changed() {
        let od = osc_desc();
        let sd = sink_desc();

        let mut graph = ModuleGraph::new();
        let mut params = ParameterMap::new();
        params.insert("frequency".to_string(), ParameterValue::Float(440.0));
        graph.add_module("osc", od.clone(), &params).unwrap();
        graph.add_module("out", sd, &ParameterMap::new()).unwrap();
        graph.connect(&NodeId::from("osc"), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();

        // prev: osc had no connected outputs
        let prev = prev_with_node(
            &NodeId::from("osc"),
            od.module_name,
            od.shape,
            params,
            PortConnectivity::new(od.inputs.len(), od.outputs.len()),
        );

        let order = vec![NodeId::from("osc"), NodeId::from("out")];
        let index = GraphIndex::build(&graph);
        let decisions = classify_nodes(&index, &order, &prev).unwrap();

        let osc = decisions.iter().find(|(id, _)| id == &NodeId::from("osc")).unwrap();
        match &osc.1 {
            NodeDecision::Update { connectivity_changed, .. } => {
                assert!(*connectivity_changed, "osc output newly connected");
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn classify_surviving_edge_removed_connectivity_changed() {
        let od = osc_desc();
        let sd = sink_desc();

        // New graph has no connection
        let mut graph = ModuleGraph::new();
        let mut params = ParameterMap::new();
        params.insert("frequency".to_string(), ParameterValue::Float(440.0));
        graph.add_module("osc", od.clone(), &params).unwrap();
        graph.add_module("out", sd, &ParameterMap::new()).unwrap();

        // prev: osc output[0] was connected
        let mut prev_conn = PortConnectivity::new(od.inputs.len(), od.outputs.len());
        prev_conn.outputs[0] = true;
        let prev = prev_with_node(
            &NodeId::from("osc"),
            od.module_name,
            od.shape,
            params,
            prev_conn,
        );

        let order = vec![NodeId::from("osc"), NodeId::from("out")];
        let index = GraphIndex::build(&graph);
        let decisions = classify_nodes(&index, &order, &prev).unwrap();

        let osc = decisions.iter().find(|(id, _)| id == &NodeId::from("osc")).unwrap();
        match &osc.1 {
            NodeDecision::Update { connectivity_changed, .. } => {
                assert!(*connectivity_changed, "osc output no longer connected");
            }
            _ => panic!("expected Update"),
        }
    }

    #[test]
    fn classify_multiple_nodes_each_classified_independently() {
        let od = osc_desc();
        let sd = sink_desc();

        let mut graph = ModuleGraph::new();
        let mut params = ParameterMap::new();
        params.insert("frequency".to_string(), ParameterValue::Float(440.0));
        graph.add_module("osc", od.clone(), &params).unwrap();
        graph.add_module("out", sd, &ParameterMap::new()).unwrap();

        // prev_state: osc is surviving; "out" is new
        let prev = prev_with_node(
            &NodeId::from("osc"),
            od.module_name,
            od.shape,
            params,
            PortConnectivity::new(od.inputs.len(), od.outputs.len()),
        );

        let order = vec![NodeId::from("osc"), NodeId::from("out")];
        let index = GraphIndex::build(&graph);
        let decisions = classify_nodes(&index, &order, &prev).unwrap();

        assert_eq!(decisions.len(), 2);
        let osc = decisions.iter().find(|(id, _)| id == &NodeId::from("osc")).unwrap();
        let out = decisions.iter().find(|(id, _)| id == &NodeId::from("out")).unwrap();
        assert!(matches!(osc.1, NodeDecision::Update { .. }), "osc should survive");
        assert!(matches!(out.1, NodeDecision::Install { .. }), "out is new");
    }
}
