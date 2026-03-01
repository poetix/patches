use std::collections::HashMap;
use std::fmt;

use crate::module::Module;

/// Stable identifier for a module node in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(usize);

/// Errors returned by [`ModuleGraph::connect`].
#[derive(Debug)]
pub enum GraphError {
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
    next_id: usize,
}

impl ModuleGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: HashMap::new(),
            next_id: 0,
        }
    }

    /// Add a module to the graph and return its stable [`NodeId`].
    pub fn add_module(&mut self, module: Box<dyn Module>) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        self.nodes.insert(id, module);
        id
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
        from: NodeId,
        output: &str,
        to: NodeId,
        input: &str,
        scale: f64,
    ) -> Result<(), GraphError> {
        if !scale.is_finite() || !(-1.0..=1.0).contains(&scale) {
            return Err(GraphError::ScaleOutOfRange(scale));
        }
        // Validate source node and output port.
        let from_desc = self
            .nodes
            .get(&from)
            .ok_or(GraphError::NodeNotFound(from))?
            .descriptor()
            .clone();

        if !from_desc.outputs.iter().any(|p| p.name == output) {
            return Err(GraphError::OutputPortNotFound {
                node: from,
                port: output.to_string(),
            });
        }

        // Validate destination node and input port.
        let to_desc = self
            .nodes
            .get(&to)
            .ok_or(GraphError::NodeNotFound(to))?
            .descriptor()
            .clone();

        if !to_desc.inputs.iter().any(|p| p.name == input) {
            return Err(GraphError::InputPortNotFound {
                node: to,
                port: input.to_string(),
            });
        }

        // Enforce one driver per input — O(1) via the edge index.
        let key = (to, input.to_string());
        if self.edges.contains_key(&key) {
            return Err(GraphError::InputAlreadyConnected {
                node: to,
                port: input.to_string(),
            });
        }

        self.edges.insert(
            key,
            Edge {
                from,
                output: output.to_string(),
                to,
                input: input.to_string(),
                scale,
            },
        );

        Ok(())
    }

    /// Remove a module and all edges that involve it.
    ///
    /// No-ops if the [`NodeId`] is not present.
    pub fn remove_module(&mut self, id: NodeId) {
        self.nodes.remove(&id);
        self.edges.retain(|_, e| e.from != id && e.to != id);
    }

    /// Remove a specific connection. No-op if the edge does not exist.
    pub fn disconnect(&mut self, from: NodeId, output: &str, to: NodeId, input: &str) {
        self.edges.retain(|_, e| {
            !(e.from == from && e.output == output && e.to == to && e.input == input)
        });
    }

    /// Return all node IDs currently in the graph.
    pub fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.keys().copied().collect()
    }

    /// Return a snapshot of all edges as `(from, output_name, to, input_name, scale)` tuples.
    pub fn edge_list(&self) -> Vec<(NodeId, String, NodeId, String, f64)> {
        self.edges
            .values()
            .map(|e| (e.from, e.output.clone(), e.to, e.input.clone(), e.scale))
            .collect()
    }

    /// Borrow a module by id for inspection (e.g. descriptor or type-checking).
    pub fn get_module(&self, id: NodeId) -> Option<&dyn Module> {
        self.nodes.get(&id).map(|m| m.as_ref())
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
    fn add_module_returns_distinct_ids() {
        let mut g = ModuleGraph::new();
        let a = g.add_module(stub(&[], &[]));
        let b = g.add_module(stub(&[], &[]));
        assert_ne!(a, b);
    }

    #[test]
    fn connect_valid_ports_succeeds() {
        let mut g = ModuleGraph::new();
        let src = g.add_module(stub(&[], &["out"]));
        let dst = g.add_module(stub(&["in"], &[]));
        assert!(g.connect(src, "out", dst, "in", 1.0).is_ok());
    }

    #[test]
    fn connect_unknown_source_node_errors() {
        let mut g = ModuleGraph::new();
        let dst = g.add_module(stub(&["in"], &[]));
        let ghost = {
            let id = g.add_module(stub(&[], &["out"]));
            g.remove_module(id);
            id
        };
        assert!(matches!(
            g.connect(ghost, "out", dst, "in", 1.0),
            Err(GraphError::NodeNotFound(_))
        ));
    }

    #[test]
    fn connect_unknown_dest_node_errors() {
        let mut g = ModuleGraph::new();
        let src = g.add_module(stub(&[], &["out"]));
        let ghost = {
            let id = g.add_module(stub(&["in"], &[]));
            g.remove_module(id);
            id
        };
        assert!(matches!(
            g.connect(src, "out", ghost, "in", 1.0),
            Err(GraphError::NodeNotFound(_))
        ));
    }

    #[test]
    fn connect_bad_output_port_errors() {
        let mut g = ModuleGraph::new();
        let src = g.add_module(stub(&[], &["out"]));
        let dst = g.add_module(stub(&["in"], &[]));
        assert!(matches!(
            g.connect(src, "nope", dst, "in", 1.0),
            Err(GraphError::OutputPortNotFound { .. })
        ));
    }

    #[test]
    fn connect_bad_input_port_errors() {
        let mut g = ModuleGraph::new();
        let src = g.add_module(stub(&[], &["out"]));
        let dst = g.add_module(stub(&["in"], &[]));
        assert!(matches!(
            g.connect(src, "out", dst, "nope", 1.0),
            Err(GraphError::InputPortNotFound { .. })
        ));
    }

    #[test]
    fn connect_input_already_connected_errors() {
        let mut g = ModuleGraph::new();
        let src1 = g.add_module(stub(&[], &["out"]));
        let src2 = g.add_module(stub(&[], &["out"]));
        let dst = g.add_module(stub(&["in"], &[]));
        g.connect(src1, "out", dst, "in", 1.0).unwrap();
        assert!(matches!(
            g.connect(src2, "out", dst, "in", 1.0),
            Err(GraphError::InputAlreadyConnected { .. })
        ));
    }

    #[test]
    fn fanout_one_output_to_multiple_inputs_succeeds() {
        let mut g = ModuleGraph::new();
        let src = g.add_module(stub(&[], &["out"]));
        let dst1 = g.add_module(stub(&["in"], &[]));
        let dst2 = g.add_module(stub(&["in"], &[]));
        assert!(g.connect(src, "out", dst1, "in", 1.0).is_ok());
        assert!(g.connect(src, "out", dst2, "in", 1.0).is_ok());
    }

    #[test]
    fn cycles_are_permitted() {
        let mut g = ModuleGraph::new();
        let a = g.add_module(stub(&["in"], &["out"]));
        let b = g.add_module(stub(&["in"], &["out"]));
        assert!(g.connect(a, "out", b, "in", 1.0).is_ok());
        assert!(g.connect(b, "out", a, "in", 1.0).is_ok());
    }

    #[test]
    fn remove_module_clears_node_and_its_edges() {
        let mut g = ModuleGraph::new();
        let a = g.add_module(stub(&[], &["out"]));
        let b = g.add_module(stub(&["in"], &["out"]));
        let c = g.add_module(stub(&["in"], &[]));
        g.connect(a, "out", b, "in", 1.0).unwrap();
        g.connect(b, "out", c, "in", 1.0).unwrap();

        g.remove_module(b);

        // b is gone; a→b and b→c edges are removed; a→c would still be addable.
        assert!(g.connect(a, "out", c, "in", 1.0).is_ok());
    }

    #[test]
    fn disconnect_removes_edge_and_is_idempotent() {
        let mut g = ModuleGraph::new();
        let src = g.add_module(stub(&[], &["out"]));
        let dst = g.add_module(stub(&["in"], &[]));
        g.connect(src, "out", dst, "in", 1.0).unwrap();

        g.disconnect(src, "out", dst, "in");
        // Now we can connect again (input is free).
        assert!(g.connect(src, "out", dst, "in", 1.0).is_ok());

        // Second disconnect is a no-op (no panic).
        g.disconnect(src, "out", dst, "in");
        g.disconnect(src, "out", dst, "in");
    }

    #[test]
    fn connect_scale_out_of_range_errors() {
        let mut g = ModuleGraph::new();
        let src = g.add_module(stub(&[], &["out"]));
        let dst = g.add_module(stub(&["in"], &[]));

        assert!(matches!(
            g.connect(src, "out", dst, "in", 1.5),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        assert!(matches!(
            g.connect(src, "out", dst, "in", -2.0),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        assert!(matches!(
            g.connect(src, "out", dst, "in", f64::NAN),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        assert!(matches!(
            g.connect(src, "out", dst, "in", f64::INFINITY),
            Err(GraphError::ScaleOutOfRange(_))
        ));
        // Boundary values are valid.
        assert!(g.connect(src, "out", dst, "in", -1.0).is_ok());
    }

    #[test]
    fn connect_scale_boundary_values_are_valid() {
        let mut g = ModuleGraph::new();
        let src = g.add_module(stub(&[], &["out"]));
        let dst1 = g.add_module(stub(&["in"], &[]));
        let dst2 = g.add_module(stub(&["in"], &[]));
        assert!(g.connect(src, "out", dst1, "in", 1.0).is_ok());
        assert!(g.connect(src, "out", dst2, "in", -1.0).is_ok());
    }
}
