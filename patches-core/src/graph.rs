use std::collections::HashMap;
use std::fmt;

use crate::module::Module;

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
    OutputPortNotFound { node: NodeId, port: String },
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
    output: String,
    to: NodeId,
    input: String,
    /// Scaling factor applied to the signal at read-time. Must be in `[-1.0, 1.0]`.
    scale: f64,
}

/// An in-memory, editable directed graph of audio modules connected by patch cables.
///
/// Nodes are module instances stored as `Box<dyn Module>` with stable [`NodeId`]s.
/// Edges represent patch cables: a connection from a named output port on one node
/// to a named input port on another.
///
/// This is a **topology-only** structure. No audio processing happens here; execution
/// ordering and buffer allocation are handled by the patch builder.
pub struct ModuleGraph {
    nodes: HashMap<NodeId, Box<dyn Module>>,
    /// Indexed by `(destination NodeId, input port name)` for O(1) duplicate-input
    /// detection in [`connect`](Self::connect). Each input port can have at most one driver.
    edges: HashMap<(NodeId, String), Edge>,
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
        module: Box<dyn Module>,
    ) -> Result<(), GraphError> {
        let id = id.into();
        if self.nodes.contains_key(&id) {
            return Err(GraphError::DuplicateNodeId(id));
        }
        self.nodes.insert(id, module);
        Ok(())
    }

    /// Connect an output port on one node to an input port on another.
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
        output: &str,
        to: &NodeId,
        input: &str,
        scale: f64,
    ) -> Result<(), GraphError> {
        if !scale.is_finite() || !(-1.0..=1.0).contains(&scale) {
            return Err(GraphError::ScaleOutOfRange(scale));
        }
        // Validate source node and output port.
        let from_desc = self
            .nodes
            .get(from)
            .ok_or_else(|| GraphError::NodeNotFound(from.clone()))?
            .descriptor()
            .clone();

        if !from_desc.outputs.iter().any(|p| p.name == output) {
            return Err(GraphError::OutputPortNotFound {
                node: from.clone(),
                port: output.to_string(),
            });
        }

        // Validate destination node and input port.
        let to_desc = self
            .nodes
            .get(to)
            .ok_or_else(|| GraphError::NodeNotFound(to.clone()))?
            .descriptor()
            .clone();

        if !to_desc.inputs.iter().any(|p| p.name == input) {
            return Err(GraphError::InputPortNotFound {
                node: to.clone(),
                port: input.to_string(),
            });
        }

        // Enforce one driver per input — O(1) via the edge index.
        let key = (to.clone(), input.to_string());
        if self.edges.contains_key(&key) {
            return Err(GraphError::InputAlreadyConnected {
                node: to.clone(),
                port: input.to_string(),
            });
        }

        self.edges.insert(
            key,
            Edge {
                from: from.clone(),
                output: output.to_string(),
                to: to.clone(),
                input: input.to_string(),
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
    pub fn disconnect(&mut self, from: &NodeId, output: &str, to: &NodeId, input: &str) {
        self.edges.retain(|_, e| {
            !(e.from == *from && e.output == output && e.to == *to && e.input == input)
        });
    }

    /// Return all node IDs currently in the graph.
    pub fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.keys().cloned().collect()
    }

    /// Return a snapshot of all edges as `(from, output_name, to, input_name, scale)` tuples.
    pub fn edge_list(&self) -> Vec<(NodeId, String, NodeId, String, f64)> {
        self.edges
            .values()
            .map(|e| (e.from.clone(), e.output.clone(), e.to.clone(), e.input.clone(), e.scale))
            .collect()
    }

    /// Borrow a module by id for inspection (e.g. descriptor or type-checking).
    pub fn get_module(&self, id: &NodeId) -> Option<&dyn Module> {
        self.nodes.get(id).map(|m| m.as_ref())
    }

    /// Consume the graph and return the underlying module map.
    ///
    /// Call [`node_ids`](Self::node_ids), [`edge_list`](Self::edge_list), and
    /// [`get_module`](Self::get_module) first to snapshot any information you need
    /// before consuming.
    pub fn into_modules(self) -> HashMap<NodeId, Box<dyn Module>> {
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
    use crate::module::{InstanceId, ModuleDescriptor, PortDescriptor};

    // A minimal stub module with configurable ports for testing.
    struct StubModule {
        instance_id: InstanceId,
        descriptor: ModuleDescriptor,
    }

    impl StubModule {
        fn new(inputs: &[&'static str], outputs: &[&'static str]) -> Self {
            Self {
                instance_id: InstanceId::next(),
                descriptor: ModuleDescriptor {
                    inputs: inputs
                        .iter()
                        .map(|&n| PortDescriptor { name: n })
                        .collect(),
                    outputs: outputs
                        .iter()
                        .map(|&n| PortDescriptor { name: n })
                        .collect(),
                },
            }
        }
    }

    impl Module for StubModule {
        fn descriptor(&self) -> &ModuleDescriptor {
            &self.descriptor
        }

        fn instance_id(&self) -> InstanceId {
            self.instance_id
        }

        fn process(&mut self, _inputs: &[f64], _outputs: &mut [f64]) {}

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn stub(inputs: &[&'static str], outputs: &[&'static str]) -> Box<dyn Module> {
        Box::new(StubModule::new(inputs, outputs))
    }

    #[test]
    fn add_module_succeeds() {
        let mut g = ModuleGraph::new();
        g.add_module("a", stub(&[], &[])).unwrap();
        g.add_module("b", stub(&[], &[])).unwrap();
        assert_eq!(g.node_ids().len(), 2);
    }

    #[test]
    fn add_module_duplicate_id_errors() {
        let mut g = ModuleGraph::new();
        g.add_module("a", stub(&[], &[])).unwrap();
        assert!(matches!(
            g.add_module("a", stub(&[], &[])),
            Err(GraphError::DuplicateNodeId(_))
        ));
    }

    #[test]
    fn connect_valid_ports_succeeds() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        g.add_module(src.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(dst.clone(), stub(&["in"], &[])).unwrap();
        assert!(g.connect(&src, "out", &dst, "in", 1.0).is_ok());
    }

    #[test]
    fn connect_unknown_source_node_errors() {
        let mut g = ModuleGraph::new();
        let dst = NodeId::from("dst");
        let ghost = NodeId::from("ghost");
        g.add_module(dst.clone(), stub(&["in"], &[])).unwrap();
        g.add_module(ghost.clone(), stub(&[], &["out"])).unwrap();
        g.remove_module(&ghost);
        assert!(matches!(
            g.connect(&ghost, "out", &dst, "in", 1.0),
            Err(GraphError::NodeNotFound(_))
        ));
    }

    #[test]
    fn connect_unknown_dest_node_errors() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let ghost = NodeId::from("ghost");
        g.add_module(src.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(ghost.clone(), stub(&["in"], &[])).unwrap();
        g.remove_module(&ghost);
        assert!(matches!(
            g.connect(&src, "out", &ghost, "in", 1.0),
            Err(GraphError::NodeNotFound(_))
        ));
    }

    #[test]
    fn connect_bad_output_port_errors() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        g.add_module(src.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(dst.clone(), stub(&["in"], &[])).unwrap();
        assert!(matches!(
            g.connect(&src, "nope", &dst, "in", 1.0),
            Err(GraphError::OutputPortNotFound { .. })
        ));
    }

    #[test]
    fn connect_bad_input_port_errors() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        g.add_module(src.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(dst.clone(), stub(&["in"], &[])).unwrap();
        assert!(matches!(
            g.connect(&src, "out", &dst, "nope", 1.0),
            Err(GraphError::InputPortNotFound { .. })
        ));
    }

    #[test]
    fn connect_input_already_connected_errors() {
        let mut g = ModuleGraph::new();
        let src1 = NodeId::from("src1");
        let src2 = NodeId::from("src2");
        let dst = NodeId::from("dst");
        g.add_module(src1.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(src2.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(dst.clone(), stub(&["in"], &[])).unwrap();
        g.connect(&src1, "out", &dst, "in", 1.0).unwrap();
        assert!(matches!(
            g.connect(&src2, "out", &dst, "in", 1.0),
            Err(GraphError::InputAlreadyConnected { .. })
        ));
    }

    #[test]
    fn fanout_one_output_to_multiple_inputs_succeeds() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst1 = NodeId::from("dst1");
        let dst2 = NodeId::from("dst2");
        g.add_module(src.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(dst1.clone(), stub(&["in"], &[])).unwrap();
        g.add_module(dst2.clone(), stub(&["in"], &[])).unwrap();
        assert!(g.connect(&src, "out", &dst1, "in", 1.0).is_ok());
        assert!(g.connect(&src, "out", &dst2, "in", 1.0).is_ok());
    }

    #[test]
    fn cycles_are_permitted() {
        let mut g = ModuleGraph::new();
        let a = NodeId::from("a");
        let b = NodeId::from("b");
        g.add_module(a.clone(), stub(&["in"], &["out"])).unwrap();
        g.add_module(b.clone(), stub(&["in"], &["out"])).unwrap();
        assert!(g.connect(&a, "out", &b, "in", 1.0).is_ok());
        assert!(g.connect(&b, "out", &a, "in", 1.0).is_ok());
    }

    #[test]
    fn remove_module_clears_node_and_its_edges() {
        let mut g = ModuleGraph::new();
        let a = NodeId::from("a");
        let b = NodeId::from("b");
        let c = NodeId::from("c");
        g.add_module(a.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(b.clone(), stub(&["in"], &["out"])).unwrap();
        g.add_module(c.clone(), stub(&["in"], &[])).unwrap();
        g.connect(&a, "out", &b, "in", 1.0).unwrap();
        g.connect(&b, "out", &c, "in", 1.0).unwrap();

        g.remove_module(&b);

        // b is gone; a→b and b→c edges are removed; a→c would still be addable.
        assert!(g.connect(&a, "out", &c, "in", 1.0).is_ok());
    }

    #[test]
    fn disconnect_removes_edge_and_is_idempotent() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        g.add_module(src.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(dst.clone(), stub(&["in"], &[])).unwrap();
        g.connect(&src, "out", &dst, "in", 1.0).unwrap();

        g.disconnect(&src, "out", &dst, "in");
        // Now we can connect again (input is free).
        assert!(g.connect(&src, "out", &dst, "in", 1.0).is_ok());

        // Second disconnect is a no-op (no panic).
        g.disconnect(&src, "out", &dst, "in");
        g.disconnect(&src, "out", &dst, "in");
    }

    #[test]
    fn connect_scale_out_of_range_errors() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst = NodeId::from("dst");
        g.add_module(src.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(dst.clone(), stub(&["in"], &[])).unwrap();

        assert!(matches!(
            g.connect(&src, "out", &dst, "in", 1.5),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        assert!(matches!(
            g.connect(&src, "out", &dst, "in", -2.0),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        assert!(matches!(
            g.connect(&src, "out", &dst, "in", f64::NAN),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        assert!(matches!(
            g.connect(&src, "out", &dst, "in", f64::INFINITY),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        // Boundary values are valid.
        assert!(g.connect(&src, "out", &dst, "in", -1.0).is_ok());
    }

    #[test]
    fn connect_scale_boundary_values_are_valid() {
        let mut g = ModuleGraph::new();
        let src = NodeId::from("src");
        let dst1 = NodeId::from("dst1");
        let dst2 = NodeId::from("dst2");
        g.add_module(src.clone(), stub(&[], &["out"])).unwrap();
        g.add_module(dst1.clone(), stub(&["in"], &[])).unwrap();
        g.add_module(dst2.clone(), stub(&["in"], &[])).unwrap();
        assert!(g.connect(&src, "out", &dst1, "in", 1.0).is_ok());
        assert!(g.connect(&src, "out", &dst2, "in", -1.0).is_ok());
    }
}
