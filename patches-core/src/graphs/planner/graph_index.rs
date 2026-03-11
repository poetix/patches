use std::collections::{HashMap, HashSet};

use crate::modules::{ModuleDescriptor, PortConnectivity};
use super::super::graph::{ModuleGraph, Node, NodeId};
use super::PlanError;

// ── Type aliases ──────────────────────────────────────────────────────────────

type EdgeList = Vec<(NodeId, String, u32, NodeId, String, u32, f64)>;
type InputBufferMap = HashMap<(NodeId, String, u32), (usize, f64)>;

// ── GraphIndex ────────────────────────────────────────────────────────────────

/// Pre-built connectivity index over a [`ModuleGraph`].
///
/// Constructed once from the graph's edge list, enabling O(1) per-port
/// connectivity queries. Used by the decision phase and action phase of plan building.
pub struct GraphIndex<'a> {
    graph: &'a ModuleGraph,
    pub(super) edges: EdgeList,
    connected_inputs: HashSet<(NodeId, String, u32)>,
    connected_outputs: HashSet<(NodeId, String, u32)>,
}

impl<'a> GraphIndex<'a> {
    pub fn build(graph: &'a ModuleGraph) -> Self {
        let edges = graph.edge_list();
        let mut connected_inputs = HashSet::with_capacity(edges.len());
        let mut connected_outputs = HashSet::with_capacity(edges.len());
        for (from, out_name, out_idx, to, in_name, in_idx, _) in &edges {
            connected_inputs.insert((to.clone(), in_name.clone(), *in_idx));
            connected_outputs.insert((from.clone(), out_name.clone(), *out_idx));
        }
        Self { graph, edges, connected_inputs, connected_outputs }
    }

    pub fn get_node(&self, id: &NodeId) -> Option<&'a Node> {
        self.graph.get_node(id)
    }

    /// Compute [`PortConnectivity`] for a single node using this index.
    ///
    /// An input port is marked connected if the index contains `(node_id, name, idx)`.
    /// An output port is marked connected if the index contains `(node_id, name, idx)`.
    /// Each port lookup is O(1); total cost is O(P_in + P_out) per node.
    pub fn compute_connectivity(
        &self,
        desc: &ModuleDescriptor,
        node_id: &NodeId,
    ) -> PortConnectivity {
        let mut connectivity = PortConnectivity::new(desc.inputs.len(), desc.outputs.len());
        for (i, port) in desc.inputs.iter().enumerate() {
            if self.connected_inputs.contains(&(node_id.clone(), port.name.to_owned(), port.index)) {
                connectivity.inputs[i] = true;
            }
        }
        for (j, port) in desc.outputs.iter().enumerate() {
            if self.connected_outputs.contains(&(node_id.clone(), port.name.to_owned(), port.index)) {
                connectivity.outputs[j] = true;
            }
        }
        connectivity
    }
}

// ── ResolvedGraph ─────────────────────────────────────────────────────────────

/// A [`GraphIndex`] extended with a resolved input-buffer map.
///
/// Constructed after buffer allocation is complete; enables O(1) input-buffer
/// lookups per module port in the action phase.
pub struct ResolvedGraph<'a> {
    #[allow(dead_code)]
    index: &'a GraphIndex<'a>,
    input_buffer_map: InputBufferMap,
}

impl<'a> ResolvedGraph<'a> {
    pub fn build(
        index: &'a GraphIndex<'a>,
        output_buf: &HashMap<(NodeId, usize), usize>,
    ) -> Result<Self, PlanError> {
        let input_buffer_map = build_input_buffer_map(&index.edges, output_buf, index.graph)?;
        Ok(Self { index, input_buffer_map })
    }

    /// Resolve each input port of `desc` on `node_id` to a `(buffer_index, scale)` pair.
    ///
    /// Looks up each port in `input_buffer_map` in O(1). Unconnected ports default to
    /// `(0, 1.0)` — the permanent-zero slot with implicit scale 1.0.
    pub fn resolve_input_buffers(
        &self,
        desc: &ModuleDescriptor,
        node_id: &NodeId,
    ) -> Vec<(usize, f64)> {
        desc.inputs
            .iter()
            .map(|port| {
                self.input_buffer_map
                    .get(&(node_id.clone(), port.name.to_owned(), port.index))
                    .copied()
                    .unwrap_or((0, 1.0))
            })
            .collect()
    }
}

// ── build_input_buffer_map ────────────────────────────────────────────────────

/// Build a map from `(to_node, in_port_name, in_port_idx)` to `(buffer_slot, scale)`.
///
/// Performs one O(E) pass over the edge list. For each edge the source node is looked up
/// in `graph`, the output port is located by name and index within that node's descriptor,
/// and the pre-allocated buffer slot is retrieved from `output_buf`.
///
/// Returns [`PlanError::Internal`] if a referenced source node, output port, or
/// buffer allocation is missing.
fn build_input_buffer_map(
    edges: &EdgeList,
    output_buf: &HashMap<(NodeId, usize), usize>,
    graph: &ModuleGraph,
) -> Result<InputBufferMap, PlanError> {
    let mut map = HashMap::with_capacity(edges.len());
    for (from, out_name, out_idx, to, in_name, in_idx, scale) in edges {
        let from_node = graph
            .get_node(from)
            .ok_or_else(|| PlanError::Internal(format!("node {from:?} missing from graph")))?;
        let out_port_idx = from_node
            .module_descriptor
            .outputs
            .iter()
            .position(|p| p.name == out_name.as_str() && p.index == *out_idx)
            .ok_or_else(|| {
                PlanError::Internal(format!(
                    "output port {out_name:?}/{out_idx} not found on node {from:?}"
                ))
            })?;
        let buf = output_buf
            .get(&(from.clone(), out_port_idx))
            .copied()
            .ok_or_else(|| {
                PlanError::Internal(format!(
                    "buffer for ({from:?}, {out_port_idx}) not found"
                ))
            })?;
        map.insert((to.clone(), in_name.clone(), *in_idx), (buf, *scale));
    }
    Ok(map)
}

// ── Test helpers (cfg(test)) ──────────────────────────────────────────────────

/// Build a [`ResolvedGraph`] from a pre-built [`GraphIndex`] and a raw input-buffer map.
///
/// Bypasses [`build_input_buffer_map`] so tests can inject a custom map directly.
#[cfg(test)]
pub(super) fn resolved_graph_for_test<'a>(
    index: &'a GraphIndex<'a>,
    input_buffer_map: InputBufferMap,
) -> ResolvedGraph<'a> {
    ResolvedGraph { index, input_buffer_map }
}

/// Build a [`GraphIndex`] from raw edge data without a populated [`ModuleGraph`].
///
/// The `graph` field is set to `graph` (may be empty); only the connectivity sets and
/// edge list are populated from `edges_raw`. Used in tests for `compute_connectivity`
/// where real module nodes are not required.
#[cfg(test)]
pub(super) fn graph_index_for_test<'a>(
    graph: &'a ModuleGraph,
    edges_raw: &[(NodeId, String, u32, NodeId, String, u32, f64)],
) -> GraphIndex<'a> {
    let mut connected_inputs = HashSet::new();
    let mut connected_outputs = HashSet::new();
    for (from, out_name, out_idx, to, in_name, in_idx, _) in edges_raw {
        connected_inputs.insert((to.clone(), in_name.clone(), *in_idx));
        connected_outputs.insert((from.clone(), out_name.clone(), *out_idx));
    }
    GraphIndex {
        graph,
        edges: edges_raw.to_vec(),
        connected_inputs,
        connected_outputs,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use super::super::PlanError;
    use crate::cables::CableKind;
    use crate::modules::{ModuleDescriptor, ModuleShape, PortDescriptor};
    use crate::parameter_map::ParameterMap;
    use crate::ModuleGraph;

    fn two_node_graph() -> (ModuleGraph, NodeId, NodeId) {
        let src_desc = ModuleDescriptor {
            module_name: "Src",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![],
            outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Mono }],
            parameters: vec![],
            is_sink: false,
        };
        let dst_desc = ModuleDescriptor {
            module_name: "Dst",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![PortDescriptor { name: "in", index: 0, kind: CableKind::Mono }],
            outputs: vec![],
            parameters: vec![],
            is_sink: true,
        };
        let mut graph = ModuleGraph::new();
        graph.add_module("src", src_desc, &ParameterMap::new()).unwrap();
        graph.add_module("dst", dst_desc, &ParameterMap::new()).unwrap();
        let src_id = NodeId::from("src");
        let dst_id = NodeId::from("dst");
        (graph, src_id, dst_id)
    }

    fn two_port_desc() -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Test",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![
                PortDescriptor { name: "in", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "in", index: 1, kind: CableKind::Mono },
            ],
            outputs: vec![
                PortDescriptor { name: "out", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "out", index: 1, kind: CableKind::Mono },
            ],
            parameters: vec![],
            is_sink: false,
        }
    }

    // ── resolve_input_buffers tests ───────────────────────────────────────────

    #[test]
    fn resolve_unconnected_port_returns_zero_buffer_scale_one() {
        let (graph, _, dst_id) = two_node_graph();
        let dst_desc = graph.get_node(&dst_id).unwrap().module_descriptor.clone();
        let empty_graph = ModuleGraph::new();
        let index = graph_index_for_test(&empty_graph, &[]);
        let resolved = resolved_graph_for_test(&index, HashMap::new());

        let result = resolved.resolve_input_buffers(&dst_desc, &dst_id);
        assert_eq!(result, vec![(0, 1.0)], "unconnected port must map to (0, 1.0)");
    }

    #[test]
    fn resolve_connected_port_returns_correct_buffer_and_scale() {
        let (graph, _src_id, dst_id) = two_node_graph();
        let dst_desc = graph.get_node(&dst_id).unwrap().module_descriptor.clone();

        let mut map: HashMap<(NodeId, String, u32), (usize, f64)> = HashMap::new();
        map.insert((dst_id.clone(), "in".to_string(), 0), (7, 0.5));
        let empty_graph = ModuleGraph::new();
        let index = graph_index_for_test(&empty_graph, &[]);
        let resolved = resolved_graph_for_test(&index, map);

        let result = resolved.resolve_input_buffers(&dst_desc, &dst_id);
        assert_eq!(result, vec![(7, 0.5)], "connected port must resolve to buffer 7 scale 0.5");
    }

    #[test]
    fn resolve_multiple_ports_independently() {
        let dst_desc_data = ModuleDescriptor {
            module_name: "Dst2",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![
                PortDescriptor { name: "x", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "y", index: 0, kind: CableKind::Mono },
            ],
            outputs: vec![],
            parameters: vec![],
            is_sink: true,
        };
        let mut graph = ModuleGraph::new();
        graph.add_module("dst2", dst_desc_data, &ParameterMap::new()).unwrap();
        let dst_id = NodeId::from("dst2");
        let dst_desc = graph.get_node(&dst_id).unwrap().module_descriptor.clone();

        let mut map: HashMap<(NodeId, String, u32), (usize, f64)> = HashMap::new();
        map.insert((dst_id.clone(), "x".to_string(), 0), (3, 1.0));
        map.insert((dst_id.clone(), "y".to_string(), 0), (4, 2.0));
        let empty_graph = ModuleGraph::new();
        let index = graph_index_for_test(&empty_graph, &[]);
        let resolved = resolved_graph_for_test(&index, map);

        let result = resolved.resolve_input_buffers(&dst_desc, &dst_id);
        assert_eq!(result, vec![(3, 1.0), (4, 2.0)]);
    }

    // ── build_input_buffer_map tests ──────────────────────────────────────────

    #[test]
    fn build_input_buffer_map_missing_source_node_returns_internal_error() {
        let (graph, _src_id, dst_id) = two_node_graph();

        let ghost_id = NodeId::from("ghost");
        let edges = vec![(
            ghost_id.clone(), "out".to_string(), 0u32,
            dst_id.clone(), "in".to_string(), 0u32,
            1.0f64,
        )];
        let output_buf = HashMap::new();

        let result = build_input_buffer_map(&edges, &output_buf, &graph);
        assert!(
            matches!(result, Err(PlanError::Internal(_))),
            "missing source node must return InternalError"
        );
    }

    #[test]
    fn build_input_buffer_map_missing_buffer_returns_internal_error() {
        let (graph, src_id, dst_id) = two_node_graph();

        let edges = vec![(
            src_id.clone(), "out".to_string(), 0u32,
            dst_id.clone(), "in".to_string(), 0u32,
            1.0f64,
        )];
        let output_buf = HashMap::new();

        let result = build_input_buffer_map(&edges, &output_buf, &graph);
        assert!(
            matches!(result, Err(PlanError::Internal(_))),
            "missing buffer allocation must return InternalError"
        );
    }

    // ── compute_connectivity tests ────────────────────────────────────────────

    #[test]
    fn connectivity_no_edges_all_false() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &[]);
        let c = index.compute_connectivity(&desc, &node);
        assert!(!c.inputs[0] && !c.inputs[1] && !c.outputs[0] && !c.outputs[1]);
    }

    #[test]
    fn connectivity_single_input_connected() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let other = NodeId::from("src");
        let edges = vec![(other, "out".to_string(), 0, node.clone(), "in".to_string(), 0, 1.0)];
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &edges);
        let c = index.compute_connectivity(&desc, &node);
        assert!(c.inputs[0]);
        assert!(!c.inputs[1] && !c.outputs[0] && !c.outputs[1]);
    }

    #[test]
    fn connectivity_single_output_connected() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let other = NodeId::from("dst");
        let edges = vec![(node.clone(), "out".to_string(), 1, other, "in".to_string(), 0, 1.0)];
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &edges);
        let c = index.compute_connectivity(&desc, &node);
        assert!(c.outputs[1]);
        assert!(!c.inputs[0] && !c.inputs[1] && !c.outputs[0]);
    }

    #[test]
    fn connectivity_multiple_ports_correct_subset() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        let edges = vec![
            (src.clone(), "out".to_string(), 0, node.clone(), "in".to_string(), 1, 1.0),
            (node.clone(), "out".to_string(), 0, dst.clone(), "in".to_string(), 0, 1.0),
        ];
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &edges);
        let c = index.compute_connectivity(&desc, &node);
        assert!(!c.inputs[0] && c.inputs[1]);
        assert!(c.outputs[0] && !c.outputs[1]);
    }

    #[test]
    fn connectivity_edges_for_other_nodes_ignored() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let a = NodeId::from("a");
        let b = NodeId::from("b");
        let edges = vec![(a.clone(), "out".to_string(), 0, b.clone(), "in".to_string(), 0, 1.0)];
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &edges);
        let c = index.compute_connectivity(&desc, &node);
        assert!(!c.inputs[0] && !c.inputs[1] && !c.outputs[0] && !c.outputs[1]);
    }

    #[test]
    fn connectivity_no_false_positive_same_name_different_index() {
        let desc = two_port_desc();
        let node = NodeId::from("n");
        let src = NodeId::from("src");
        let edges = vec![(src, "out".to_string(), 0, node.clone(), "in".to_string(), 1, 1.0)];
        let graph = ModuleGraph::new();
        let index = graph_index_for_test(&graph, &edges);
        let c = index.compute_connectivity(&desc, &node);
        assert!(!c.inputs[0], "in/0 must not be marked");
        assert!(c.inputs[1], "in/1 must be marked");
    }
}
