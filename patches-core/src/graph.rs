use std::collections::HashMap;
use std::fmt;

use crate::{ModuleDescriptor, PortRef};
use crate::parameter_map::ParameterMap;

/// Stable identifier for a module node in the graph.
///
/// Wraps a `String` so that callers (e.g. a DSL layer) can assign meaningful,
/// stable names that survive across re-plans.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(String);

impl NodeId {
    /// Return the string identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for NodeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Errors returned by [`ModuleGraph`] operations.
#[derive(Debug)]
pub enum GraphError {
    /// A node with this id already exists in the graph.
    DuplicateNodeId(NodeId),
    NodeNotFound(NodeId),
    /// `port` is formatted as `"name/index"` (e.g. `"out/0"`).
    OutputPortNotFound { node: NodeId, port: String },
    /// `port` is formatted as `"name/index"` (e.g. `"in/2"`).
    InputPortNotFound { node: NodeId, port: String },
    InputAlreadyConnected { node: NodeId, port: String },
    /// `scale` must be finite and in `[-1.0, 1.0]`.
    ScaleOutOfRange(f64),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::DuplicateNodeId(id) => write!(f, "duplicate node id {:?}", id),
            GraphError::NodeNotFound(id) => write!(f, "node {:?} not found", id),
            GraphError::OutputPortNotFound { node, port } => {
                write!(f, "node {:?} has no output port {:?}", node, port)
            }
            GraphError::InputPortNotFound { node, port } => {
                write!(f, "node {:?} has no input port {:?}", node, port)
            }
            GraphError::InputAlreadyConnected { node, port } => {
                write!(
                    f,
                    "input port {:?} on node {:?} already has a connection",
                    port, node
                )
            }
            GraphError::ScaleOutOfRange(s) => {
                write!(f, "scale {s} is out of range; must be finite and in [-1.0, 1.0]")
            }
        }
    }
}

impl std::error::Error for GraphError {}

/// A directed connection from one module's output to another's input.
#[derive(Debug, Clone, PartialEq)]
struct Edge {
    from: NodeId,
    output_name: String,
    output_index: u32,
    to: NodeId,
    input_name: String,
    input_index: u32,
    /// Scaling factor applied to the signal at read-time. Must be in `[-1.0, 1.0]`.
    scale: f64,
}

/// A node in the module graph, holding a descriptor and its current parameter values.
pub struct Node {
    pub module_descriptor: ModuleDescriptor,
    pub parameter_map: ParameterMap,
}

/// An in-memory, editable directed graph of audio modules connected by patch cables.
///
/// Nodes store `ModuleDescriptor` and `ParameterMap` values with stable [`NodeId`]s.
/// Edges represent patch cables: a connection from a named, indexed output port on
/// one node to a named, indexed input port on another.
///
/// This is a **topology-only** structure. No audio processing happens here; execution
/// ordering and buffer allocation are handled by the patch builder.
pub struct ModuleGraph {
    nodes: HashMap<NodeId, Node>,
    /// Indexed by `(destination NodeId, input port name, input port index)` for O(1)
    /// duplicate-input detection in [`connect`](Self::connect). Each input port can
    /// have at most one driver.
    edges: HashMap<(NodeId, String, u32), Edge>,
}

impl ModuleGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
        }
    }

    /// Add a module to the graph with the given [`NodeId`].
    ///
    /// Returns an error if a module with the same id already exists.
    pub fn add_module(
        &mut self,
        id: impl Into<NodeId>,
        module_descriptor: ModuleDescriptor,
        parameter_map: &ParameterMap,
    ) -> Result<(), GraphError> {
        let id = id.into();
        if self.nodes.contains_key(&id) {
            return Err(GraphError::DuplicateNodeId(id));
        }
        self.nodes.insert(
            id,
            Node {
                module_descriptor,
                parameter_map: parameter_map.clone(),
            },
        );
        Ok(())
    }

    /// Connect an output port on one node to an input port on another.
    ///
    /// `output` and `input` are [`PortRef`] values identifying the source and
    /// destination ports by name and index. Use `index: 0` for modules with a
    /// single port of a given name.
    ///
    /// `scale` is a multiplier in `[-1.0, 1.0]` applied to the signal at
    /// read-time during `tick()`. Use `1.0` for an unscaled connection.
    ///
    /// Returns an error if either node or port does not exist, if the target
    /// input already has an incoming connection, or if `scale` is not finite
    /// or falls outside `[-1.0, 1.0]`.
    pub fn connect(
        &mut self,
        from: &NodeId,
        output: PortRef,
        to: &NodeId,
        input: PortRef,
        scale: f64,
    ) -> Result<(), GraphError> {
        if !scale.is_finite() || !(-1.0..=1.0).contains(&scale) {
            return Err(GraphError::ScaleOutOfRange(scale));
        }

        // Validate source node and output port.
        {
            let from_node = self
                .nodes
                .get(from)
                .ok_or_else(|| GraphError::NodeNotFound(from.clone()))?;
            if !from_node
                .module_descriptor
                .outputs
                .iter()
                .any(|p| p.name == output.name && p.index == output.index)
            {
                return Err(GraphError::OutputPortNotFound {
                    node: from.clone(),
                    port: format!("{}/{}", output.name, output.index),
                });
            }
        }

        // Validate destination node and input port.
        {
            let to_node = self
                .nodes
                .get(to)
                .ok_or_else(|| GraphError::NodeNotFound(to.clone()))?;
            if !to_node
                .module_descriptor
                .inputs
                .iter()
                .any(|p| p.name == input.name && p.index == input.index)
            {
                return Err(GraphError::InputPortNotFound {
                    node: to.clone(),
                    port: format!("{}/{}", input.name, input.index),
                });
            }
        }

        // Enforce one driver per input — O(1) via the edge index.
        let key = (to.clone(), input.name.to_string(), input.index);
        if self.edges.contains_key(&key) {
            return Err(GraphError::InputAlreadyConnected {
                node: to.clone(),
                port: format!("{}/{}", input.name, input.index),
            });
        }

        self.edges.insert(
            key,
            Edge {
                from: from.clone(),
                output_name: output.name.to_string(),
                output_index: output.index,
                to: to.clone(),
                input_name: input.name.to_string(),
                input_index: input.index,
                scale,
            },
        );

        Ok(())
    }

    /// Remove a module and all edges that involve it.
    ///
    /// No-ops if the [`NodeId`] is not present.
    pub fn remove_module(&mut self, id: &NodeId) {
        self.nodes.remove(id);
        self.edges.retain(|_, e| e.from != *id && e.to != *id);
    }

    /// Remove a specific connection. No-op if the edge does not exist.
    pub fn disconnect(&mut self, from: &NodeId, output: PortRef, to: &NodeId, input: PortRef) {
        self.edges.retain(|_, e| {
            !(e.from == *from
                && e.output_name == output.name
                && e.output_index == output.index
                && e.to == *to
                && e.input_name == input.name
                && e.input_index == input.index)
        });
    }

    /// Return all node IDs currently in the graph.
    pub fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.keys().cloned().collect()
    }

    /// Return a snapshot of all edges as
    /// `(from, output_name, output_index, to, input_name, input_index, scale)` tuples.
    pub fn edge_list(&self) -> Vec<(NodeId, String, u32, NodeId, String, u32, f64)> {
        self.edges
            .values()
            .map(|e| {
                (
                    e.from.clone(),
                    e.output_name.clone(),
                    e.output_index,
                    e.to.clone(),
                    e.input_name.clone(),
                    e.input_index,
                    e.scale,
                )
            })
            .collect()
    }

    /// Borrow a node by id for inspection (e.g. descriptor or parameter map).
    pub fn get_node(&self, id: &NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// Consume the graph and return the underlying node map.
    ///
    /// Call [`node_ids`](Self::node_ids), [`edge_list`](Self::edge_list), and
    /// [`get_node`](Self::get_node) first to snapshot any information you need
    /// before consuming.
    pub fn into_nodes(self) -> HashMap<NodeId, Node> {
        self.nodes
    }
}

impl Default for ModuleGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModuleDescriptor, PortDescriptor, PortRef};
    use crate::module_descriptor::ModuleShape;
    use crate::parameter_map::ParameterMap;

    fn stub_desc(inputs: &[&'static str], outputs: &[&'static str]) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "stub",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: inputs
                .iter()
                .map(|&n| PortDescriptor { name: n, index: 0 })
                .collect(),
            outputs: outputs
                .iter()
                .map(|&n| PortDescriptor { name: n, index: 0 })
                .collect(),
            parameters: vec![],
            is_sink: false,
        }
    }

    fn pref(name: &'static str) -> PortRef {
        PortRef { name, index: 0 }
    }

    fn no_params() -> ParameterMap {
        ParameterMap::new()
    }

    #[test]
    fn add_module_succeeds() {
        let mut g = ModuleGraph::new();
        g.add_module("a", stub_desc(&[], &[]), &no_params()).unwrap();
        g.add_module("b", stub_desc(&[], &[]), &no_params()).unwrap();
        assert_eq!(g.node_ids().len(), 2);
    }

    #[test]
    fn add_module_duplicate_id_errors() {
        let mut g = ModuleGraph::new();
        g.add_module("a", stub_desc(&[], &[]), &no_params()).unwrap();
        assert!(matches!(
            g.add_module("a", stub_desc(&[], &[]), &no_params()),
            Err(GraphError::DuplicateNodeId(_))
        ));
    }

    #[test]
    fn connect_valid_ports_succeeds() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        g.add_module(src.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(dst.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        assert!(g.connect(&src, pref("out"), &dst, pref("in"), 1.0).is_ok());
    }

    #[test]
    fn connect_unknown_source_node_errors() {
        let mut g = ModuleGraph::new();
        let dst = NodeId::from("dst");
        let ghost = NodeId::from("ghost");
        g.add_module(dst.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        g.add_module(ghost.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.remove_module(&ghost);
        assert!(matches!(
            g.connect(&ghost, pref("out"), &dst, pref("in"), 1.0),
            Err(GraphError::NodeNotFound(_))
        ));
    }

    #[test]
    fn connect_unknown_dest_node_errors() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let ghost = NodeId::from("ghost");
        g.add_module(src.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(ghost.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        g.remove_module(&ghost);
        assert!(matches!(
            g.connect(&src, pref("out"), &ghost, pref("in"), 1.0),
            Err(GraphError::NodeNotFound(_))
        ));
    }

    #[test]
    fn connect_bad_output_port_errors() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        g.add_module(src.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(dst.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        assert!(matches!(
            g.connect(&src, pref("nope"), &dst, pref("in"), 1.0),
            Err(GraphError::OutputPortNotFound { .. })
        ));
    }

    #[test]
    fn connect_bad_input_port_errors() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        g.add_module(src.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(dst.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        assert!(matches!(
            g.connect(&src, pref("out"), &dst, pref("nope"), 1.0),
            Err(GraphError::InputPortNotFound { .. })
        ));
    }

    #[test]
    fn connect_input_already_connected_errors() {
        let mut g = ModuleGraph::new();
        let src1 = NodeId::from("src1");
        let src2 = NodeId::from("src2");
        let dst = NodeId::from("dst");
        g.add_module(src1.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(src2.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(dst.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        g.connect(&src1, pref("out"), &dst, pref("in"), 1.0).unwrap();
        assert!(matches!(
            g.connect(&src2, pref("out"), &dst, pref("in"), 1.0),
            Err(GraphError::InputAlreadyConnected { .. })
        ));
    }

    #[test]
    fn fanout_one_output_to_multiple_inputs_succeeds() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst1 = NodeId::from("dst1");
        let dst2 = NodeId::from("dst2");
        g.add_module(src.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(dst1.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        g.add_module(dst2.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        assert!(g.connect(&src, pref("out"), &dst1, pref("in"), 1.0).is_ok());
        assert!(g.connect(&src, pref("out"), &dst2, pref("in"), 1.0).is_ok());
    }

    #[test]
    fn cycles_are_permitted() {
        let mut g = ModuleGraph::new();
        let a = NodeId::from("a");
        let b = NodeId::from("b");
        g.add_module(a.clone(), stub_desc(&["in"], &["out"]), &no_params()).unwrap();
        g.add_module(b.clone(), stub_desc(&["in"], &["out"]), &no_params()).unwrap();
        assert!(g.connect(&a, pref("out"), &b, pref("in"), 1.0).is_ok());
        assert!(g.connect(&b, pref("out"), &a, pref("in"), 1.0).is_ok());
    }

    #[test]
    fn remove_module_clears_node_and_its_edges() {
        let mut g = ModuleGraph::new();
        let a = NodeId::from("a");
        let b = NodeId::from("b");
        let c = NodeId::from("c");
        g.add_module(a.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(b.clone(), stub_desc(&["in"], &["out"]), &no_params()).unwrap();
        g.add_module(c.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        g.connect(&a, pref("out"), &b, pref("in"), 1.0).unwrap();
        g.connect(&b, pref("out"), &c, pref("in"), 1.0).unwrap();

        g.remove_module(&b);

        // b is gone; a→b and b→c edges are removed; a→c would still be addable.
        assert!(g.connect(&a, pref("out"), &c, pref("in"), 1.0).is_ok());
    }

    #[test]
    fn disconnect_removes_edge_and_is_idempotent() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        g.add_module(src.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(dst.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        g.connect(&src, pref("out"), &dst, pref("in"), 1.0).unwrap();

        g.disconnect(&src, pref("out"), &dst, pref("in"));
        // Now we can connect again (input is free).
        assert!(g.connect(&src, pref("out"), &dst, pref("in"), 1.0).is_ok());

        // Second disconnect is a no-op (no panic).
        g.disconnect(&src, pref("out"), &dst, pref("in"));
        g.disconnect(&src, pref("out"), &dst, pref("in"));
    }

    #[test]
    fn connect_scale_out_of_range_errors() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        g.add_module(src.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(dst.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();

        assert!(matches!(
            g.connect(&src, pref("out"), &dst, pref("in"), 1.5),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        assert!(matches!(
            g.connect(&src, pref("out"), &dst, pref("in"), -2.0),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        assert!(matches!(
            g.connect(&src, pref("out"), &dst, pref("in"), f64::NAN),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        assert!(matches!(
            g.connect(&src, pref("out"), &dst, pref("in"), f64::INFINITY),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        // Boundary values are valid.
        assert!(g.connect(&src, pref("out"), &dst, pref("in"), -1.0).is_ok());
    }

    #[test]
    fn connect_scale_boundary_values_are_valid() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst1 = NodeId::from("dst1");
        let dst2 = NodeId::from("dst2");
        g.add_module(src.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(dst1.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        g.add_module(dst2.clone(), stub_desc(&["in"], &[]), &no_params()).unwrap();
        assert!(g.connect(&src, pref("out"), &dst1, pref("in"), 1.0).is_ok());
        assert!(g.connect(&src, pref("out"), &dst2, pref("in"), -1.0).is_ok());
    }

    #[test]
    fn port_ref_index_distinguishes_same_named_ports() {
        // A descriptor with two ports both named "in" but different indices.
        let desc = ModuleDescriptor {
            module_name: "stub",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![
                PortDescriptor { name: "in", index: 0 },
                PortDescriptor { name: "in", index: 1 },
            ],
            outputs: vec![],
            parameters: vec![],
            is_sink: false,
        };
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        let mut g = ModuleGraph::new();
        g.add_module(src.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        g.add_module(dst.clone(), desc, &no_params()).unwrap();

        // Connect to in/0 and in/1 — both must succeed.
        assert!(g
            .connect(&src, pref("out"), &dst, PortRef { name: "in", index: 0 }, 1.0)
            .is_ok());
        // Fanout src to in/1 requires a second src output (or we use a separate src).
        let src2 = NodeId::from("src2");
        g.add_module(src2.clone(), stub_desc(&[], &["out"]), &no_params()).unwrap();
        assert!(g
            .connect(&src2, pref("out"), &dst, PortRef { name: "in", index: 1 }, 1.0)
            .is_ok());
    }
}
