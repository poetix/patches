//! YAML serialisation and deserialisation for [`ModuleGraph`].
//!
//! # Format
//!
//! ```yaml
//! nodes:
//!   osc1:
//!     module: Oscillator
//!     channels: 1        # omitted when 0
//!     length: 0          # omitted when 0
//!     params:
//!       frequency: 440.0
//!       waveform: sine   # Enum — plain string
//!
//! cables:
//!   - from: osc1
//!     output: out        # "name" means index 0; "name/2" means index 2
//!     to: amp1
//!     input: in
//!     scale: 1.0         # omitted when 1.0
//! ```
//!
//! Parameter values are plain YAML scalars. On deserialisation the module's
//! [`crate::ParameterDescriptor`] is used to coerce each value to the correct
//! [`crate::ParameterValue`] variant, so no explicit type tags are needed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    GraphError, ModuleGraph, ModuleShape, NodeId, ParameterKind, ParameterMap, ParameterValue,
    PortRef, Registry,
};

// ── Error ────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum GraphYamlError {
    /// YAML parsing or serialisation failed.
    Yaml(serde_yml::Error),
    /// The registry has no builder for the module type named in the YAML.
    UnknownModule { name: String },
    /// A cable references a node id that doesn't appear in the `nodes` map.
    UnknownNode { id: String },
    /// A parameter name in the YAML is not declared by the module's descriptor.
    UnknownParameter { node: String, param: String },
    /// A YAML parameter value can't be coerced to the type declared by the descriptor.
    ParameterTypeMismatch { node: String, param: String, expected: &'static str },
    /// A port reference string is malformed (e.g. `"in/abc"` where the index isn't a number).
    InvalidPortRef { port: String },
    /// A graph construction operation (duplicate node, bad connection, …) failed.
    Graph(GraphError),
}

impl std::fmt::Display for GraphYamlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GraphYamlError::Yaml(e) => write!(f, "YAML error: {e}"),
            GraphYamlError::UnknownModule { name } => write!(f, "unknown module type: {name:?}"),
            GraphYamlError::UnknownNode { id } => write!(f, "unknown node id: {id:?}"),
            GraphYamlError::UnknownParameter { node, param } => {
                write!(f, "node {node:?}: unknown parameter {param:?}")
            }
            GraphYamlError::ParameterTypeMismatch { node, param, expected } => {
                write!(f, "node {node:?}: parameter {param:?} expected {expected}")
            }
            GraphYamlError::InvalidPortRef { port } => write!(f, "invalid port reference: {port:?}"),
            GraphYamlError::Graph(e) => write!(f, "graph error: {e}"),
        }
    }
}

impl std::error::Error for GraphYamlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GraphYamlError::Yaml(e) => Some(e),
            GraphYamlError::Graph(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_yml::Error> for GraphYamlError {
    fn from(e: serde_yml::Error) -> Self {
        GraphYamlError::Yaml(e)
    }
}

impl From<GraphError> for GraphYamlError {
    fn from(e: GraphError) -> Self {
        GraphYamlError::Graph(e)
    }
}

// ── Wire types ───────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct YamlGraph {
    nodes: BTreeMap<String, YamlNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cables: Vec<YamlCable>,
}

#[derive(Serialize, Deserialize)]
struct YamlNode {
    module: String,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    channels: usize,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    length: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    params: BTreeMap<String, serde_yml::Value>,
}

/// A cable in YAML. Port references use the convention `"name"` (index 0) or
/// `"name/n"` (index n), e.g. `"in/1"` means port `in` at index 1.
#[derive(Serialize, Deserialize)]
struct YamlCable {
    from: String,
    /// Output port: `"name"` or `"name/n"`.
    output: String,
    to: String,
    /// Input port: `"name"` or `"name/n"`.
    input: String,
    #[serde(default = "one_f64", skip_serializing_if = "is_one_f64")]
    scale: f64,
}

fn is_zero_usize(v: &usize) -> bool { *v == 0 }
fn is_one_f64(v: &f64) -> bool { *v == 1.0 }
fn one_f64() -> f64 { 1.0 }

/// Encode `(name, index)` as `"name"` when index is 0, or `"name/index"` otherwise.
fn encode_port(name: &str, index: u32) -> String {
    if index == 0 { name.to_string() } else { format!("{name}/{index}") }
}

/// Decode `"name"` → `("name", 0)` or `"name/n"` → `("name", n)`.
fn decode_port(s: &str) -> Result<(&str, u32), GraphYamlError> {
    match s.rfind('/') {
        Some(pos) => {
            let index = s[pos + 1..]
                .parse::<u32>()
                .map_err(|_| GraphYamlError::InvalidPortRef { port: s.to_string() })?;
            Ok((&s[..pos], index))
        }
        None => Ok((s, 0)),
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Serialise `graph` to a YAML string.
///
/// Nodes are emitted in lexicographic order of their [`NodeId`]. Cables are
/// sorted by `(from, output, to, input)`.
pub fn graph_to_yaml(graph: &ModuleGraph) -> Result<String, GraphYamlError> {
    let mut nodes = BTreeMap::new();
    let mut ids = graph.node_ids();
    ids.sort();

    for id in &ids {
        // id came from node_ids(), so it must be present.
        let node = graph.get_node(id).unwrap();
        let desc = &node.module_descriptor;

        let params: BTreeMap<String, serde_yml::Value> = node
            .parameter_map
            .iter()
            .map(|(k, v)| (k.clone(), param_value_to_yaml(v)))
            .collect();

        nodes.insert(
            id.as_str().to_string(),
            YamlNode {
                module: desc.module_name.to_string(),
                channels: desc.shape.channels,
                length: desc.shape.length,
                params,
            },
        );
    }

    let mut cables: Vec<YamlCable> = graph
        .edge_list()
        .into_iter()
        .map(|(from, output, output_index, to, input, input_index, scale)| YamlCable {
            from: from.as_str().to_string(),
            output: encode_port(&output, output_index),
            to: to.as_str().to_string(),
            input: encode_port(&input, input_index),
            scale,
        })
        .collect();

    cables.sort_by(|a, b| {
        a.from
            .cmp(&b.from)
            .then(a.output.cmp(&b.output))
            .then(a.to.cmp(&b.to))
            .then(a.input.cmp(&b.input))
    });

    Ok(serde_yml::to_string(&YamlGraph { nodes, cables })?)
}

/// Deserialise a YAML string into a [`ModuleGraph`].
///
/// Each node's [`crate::ModuleDescriptor`] is obtained from `registry` using
/// the module name and shape stored in the YAML. Returns
/// [`GraphYamlError::UnknownModule`] if the registry has no builder for a
/// named type.
///
/// Parameter values are coerced to the types declared by the module's
/// descriptor. Unknown parameter names and type mismatches are both errors.
pub fn yaml_to_graph(yaml: &str, registry: &Registry) -> Result<ModuleGraph, GraphYamlError> {
    let raw: YamlGraph = serde_yml::from_str(yaml)?;
    let mut graph = ModuleGraph::new();

    for (id, yaml_node) in &raw.nodes {
        let shape = ModuleShape { channels: yaml_node.channels, length: yaml_node.length };

        let descriptor = registry
            .describe(&yaml_node.module, &shape)
            .map_err(|_| GraphYamlError::UnknownModule { name: yaml_node.module.clone() })?;

        let mut params = ParameterMap::new();
        for (param_name, yaml_val) in &yaml_node.params {
            let param_desc = descriptor
                .parameters
                .iter()
                .find(|p| p.name == param_name.as_str())
                .ok_or_else(|| GraphYamlError::UnknownParameter {
                    node: id.clone(),
                    param: param_name.clone(),
                })?;

            let value = coerce_param(yaml_val, &param_desc.parameter_type, id, param_name)?;
            params.insert(param_name.clone(), value);
        }

        graph.add_module(id.as_str(), descriptor, &params)?;
    }

    for cable in &raw.cables {
        let from_id = NodeId::from(cable.from.as_str());
        let to_id = NodeId::from(cable.to.as_str());

        let (out_name, out_idx) = decode_port(&cable.output)?;
        let (in_name, in_idx) = decode_port(&cable.input)?;

        // Resolve &'static port names from the descriptors already in the graph.
        let output_port_name = graph
            .get_node(&from_id)
            .ok_or_else(|| GraphYamlError::UnknownNode { id: cable.from.clone() })?
            .module_descriptor
            .outputs
            .iter()
            .find(|p| p.name == out_name && p.index == out_idx)
            .map(|p| p.name)
            .ok_or_else(|| {
                GraphYamlError::Graph(GraphError::OutputPortNotFound {
                    node: from_id.clone(),
                    port: cable.output.clone(),
                })
            })?;

        let input_port_name = graph
            .get_node(&to_id)
            .ok_or_else(|| GraphYamlError::UnknownNode { id: cable.to.clone() })?
            .module_descriptor
            .inputs
            .iter()
            .find(|p| p.name == in_name && p.index == in_idx)
            .map(|p| p.name)
            .ok_or_else(|| {
                GraphYamlError::Graph(GraphError::InputPortNotFound {
                    node: to_id.clone(),
                    port: cable.input.clone(),
                })
            })?;

        graph.connect(
            &from_id,
            PortRef { name: output_port_name, index: out_idx },
            &to_id,
            PortRef { name: input_port_name, index: in_idx },
            cable.scale,
        )?;
    }

    Ok(graph)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn param_value_to_yaml(value: &ParameterValue) -> serde_yml::Value {
    match value {
        ParameterValue::Float(f) => serde_yml::Value::Number((*f).into()),
        ParameterValue::Int(i) => serde_yml::Value::Number((*i).into()),
        ParameterValue::Bool(b) => serde_yml::Value::Bool(*b),
        ParameterValue::Enum(s) => serde_yml::Value::String((*s).to_string()),
        ParameterValue::Array(v) => serde_yml::Value::Sequence(
            v.iter().map(|s| serde_yml::Value::String(s.clone())).collect(),
        ),
    }
}

fn coerce_param(
    yaml_val: &serde_yml::Value,
    kind: &ParameterKind,
    node_id: &str,
    param_name: &str,
) -> Result<ParameterValue, GraphYamlError> {
    match kind {
        ParameterKind::Float { .. } => yaml_val
            .as_f64()
            .map(ParameterValue::Float)
            .ok_or_else(|| mismatch(node_id, param_name, "float")),

        ParameterKind::Int { .. } => yaml_val
            .as_i64()
            .map(ParameterValue::Int)
            .ok_or_else(|| mismatch(node_id, param_name, "int")),

        ParameterKind::Bool { .. } => match yaml_val {
            serde_yml::Value::Bool(b) => Ok(ParameterValue::Bool(*b)),
            _ => Err(mismatch(node_id, param_name, "bool")),
        },

        ParameterKind::Enum { variants, .. } => {
            let s = yaml_val
                .as_str()
                .ok_or_else(|| mismatch(node_id, param_name, "string"))?;
            variants
                .iter()
                .find(|&&v| v == s)
                .map(|&v| ParameterValue::Enum(v))
                .ok_or_else(|| mismatch(node_id, param_name, "known enum variant"))
        }

        ParameterKind::Array { .. } => match yaml_val {
            serde_yml::Value::Sequence(seq) => {
                let strings: Result<Vec<String>, GraphYamlError> = seq
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        item.as_str()
                            .map(|s| s.to_string())
                            .ok_or_else(|| {
                                mismatch(node_id, &format!("{param_name}[{i}]"), "string")
                            })
                    })
                    .collect();
                strings.map(ParameterValue::Array)
            }
            _ => Err(mismatch(node_id, param_name, "sequence")),
        },
    }
}

fn mismatch(node: &str, param: &str, expected: &'static str) -> GraphYamlError {
    GraphYamlError::ParameterTypeMismatch {
        node: node.to_string(),
        param: param.to_string(),
        expected,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AudioEnvironment, CableKind, InstanceId, ModuleDescriptor, ParameterDescriptor, PortDescriptor,
    };
    use crate::modules::Module;

    // --- minimal test modules ------------------------------------------------

    struct SineOsc {
        instance_id: InstanceId,
        descriptor: ModuleDescriptor,
    }

    impl Module for SineOsc {
        fn describe(shape: &ModuleShape) -> ModuleDescriptor {
            ModuleDescriptor {
                module_name: "SineOsc",
                shape: shape.clone(),
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Mono }],
                parameters: vec![ParameterDescriptor {
                    name: "frequency",
                    index: 0,
                    parameter_type: ParameterKind::Float {
                        min: 0.0,
                        max: 20000.0,
                        default: 440.0,
                    },
                }],
                is_sink: false,
            }
        }

        fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
            Self { instance_id, descriptor }
        }

        fn update_validated_parameters(&mut self, _params: &ParameterMap) {}
        fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
        fn instance_id(&self) -> InstanceId { self.instance_id }
        fn process(&mut self, _inputs: &[f64], _outputs: &mut [f64]) {}
        fn as_any(&self) -> &dyn std::any::Any { self }
    }

    struct GainModule {
        instance_id: InstanceId,
        descriptor: ModuleDescriptor,
    }

    impl Module for GainModule {
        fn describe(shape: &ModuleShape) -> ModuleDescriptor {
            ModuleDescriptor {
                module_name: "Gain",
                shape: shape.clone(),
                inputs: vec![PortDescriptor { name: "in", index: 0, kind: CableKind::Mono }],
                outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Mono }],
                parameters: vec![ParameterDescriptor {
                    name: "gain",
                    index: 0,
                    parameter_type: ParameterKind::Float { min: 0.0, max: 2.0, default: 1.0 },
                }],
                is_sink: false,
            }
        }

        fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
            Self { instance_id, descriptor }
        }

        fn update_validated_parameters(&mut self, _params: &ParameterMap) {}
        fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
        fn instance_id(&self) -> InstanceId { self.instance_id }
        fn process(&mut self, _inputs: &[f64], _outputs: &mut [f64]) {}
        fn as_any(&self) -> &dyn std::any::Any { self }
    }

    /// A mixer with two indexed inputs (`in/0` and `in/1`) and one output.
    struct MixModule {
        instance_id: InstanceId,
        descriptor: ModuleDescriptor,
    }

    impl Module for MixModule {
        fn describe(shape: &ModuleShape) -> ModuleDescriptor {
            ModuleDescriptor {
                module_name: "Mix",
                shape: shape.clone(),
                inputs: vec![
                    PortDescriptor { name: "in", index: 0, kind: CableKind::Mono },
                    PortDescriptor { name: "in", index: 1, kind: CableKind::Mono },
                ],
                outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Mono }],
                parameters: vec![],
                is_sink: false,
            }
        }

        fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
            Self { instance_id, descriptor }
        }

        fn update_validated_parameters(&mut self, _params: &ParameterMap) {}
        fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
        fn instance_id(&self) -> InstanceId { self.instance_id }
        fn process(&mut self, _inputs: &[f64], _outputs: &mut [f64]) {}
        fn as_any(&self) -> &dyn std::any::Any { self }
    }

    fn make_registry() -> Registry {
        let mut r = Registry::new();
        r.register::<SineOsc>();
        r.register::<GainModule>();
        r.register::<MixModule>();
        r
    }

    // --- tests ---------------------------------------------------------------

    #[test]
    fn round_trip_preserves_nodes_edges_and_params() {
        let registry = make_registry();
        let osc_id = NodeId::from("osc1");
        let gain_id = NodeId::from("amp1");

        let osc_desc =
            registry.describe("SineOsc", &ModuleShape { channels: 1, length: 0 }).unwrap();
        let gain_desc =
            registry.describe("Gain", &ModuleShape { channels: 1, length: 0 }).unwrap();

        let mut osc_params = ParameterMap::new();
        osc_params.insert("frequency".to_string(), ParameterValue::Float(880.0));

        let mut graph = ModuleGraph::new();
        graph.add_module(osc_id.clone(), osc_desc, &osc_params).unwrap();
        graph.add_module(gain_id.clone(), gain_desc, &ParameterMap::new()).unwrap();
        graph
            .connect(
                &osc_id,
                PortRef { name: "out", index: 0 },
                &gain_id,
                PortRef { name: "in", index: 0 },
                0.5,
            )
            .unwrap();

        let yaml = graph_to_yaml(&graph).unwrap();
        let graph2 = yaml_to_graph(&yaml, &registry).unwrap();

        assert_eq!(graph2.node_ids().len(), 2);

        let osc_node = graph2.get_node(&osc_id).unwrap();
        assert_eq!(osc_node.module_descriptor.module_name, "SineOsc");
        assert_eq!(osc_node.module_descriptor.shape.channels, 1);
        assert_eq!(
            osc_node.parameter_map.get("frequency"),
            Some(&ParameterValue::Float(880.0))
        );

        let edges = graph2.edge_list();
        assert_eq!(edges.len(), 1);
        let (from, out_name, out_idx, to, in_name, in_idx, scale) = &edges[0];
        assert_eq!(from.as_str(), "osc1");
        assert_eq!(out_name, "out");
        assert_eq!(*out_idx, 0);
        assert_eq!(to.as_str(), "amp1");
        assert_eq!(in_name, "in");
        assert_eq!(*in_idx, 0);
        assert_eq!(*scale, 0.5);
    }

    #[test]
    fn unknown_module_returns_error() {
        let registry = make_registry();
        let yaml = "nodes:\n  x:\n    module: NoSuchModule\n    channels: 1\n";
        match yaml_to_graph(yaml, &registry) {
            Err(GraphYamlError::UnknownModule { name }) => assert_eq!(name, "NoSuchModule"),
            Err(e) => panic!("expected UnknownModule, got error: {e}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn parameter_type_mismatch_returns_error() {
        let registry = make_registry();
        // "frequency" expects a float but we give a bool
        let yaml =
            "nodes:\n  osc:\n    module: SineOsc\n    channels: 1\n    params:\n      frequency: true\n";
        assert!(matches!(
            yaml_to_graph(yaml, &registry),
            Err(GraphYamlError::ParameterTypeMismatch { .. })
        ));
    }

    #[test]
    fn indexed_port_round_trips_as_name_slash_n() {
        let registry = make_registry();
        let osc_id = NodeId::from("src");
        let mix_id = NodeId::from("mix");

        let osc_desc = registry.describe("SineOsc", &ModuleShape { channels: 0, length: 0 }).unwrap();
        let mix_desc = registry.describe("Mix", &ModuleShape { channels: 0, length: 0 }).unwrap();

        let mut graph = ModuleGraph::new();
        graph.add_module(osc_id.clone(), osc_desc, &ParameterMap::new()).unwrap();
        graph.add_module(mix_id.clone(), mix_desc, &ParameterMap::new()).unwrap();
        graph.connect(&osc_id, PortRef { name: "out", index: 0 }, &mix_id, PortRef { name: "in", index: 1 }, 1.0).unwrap();

        let yaml = graph_to_yaml(&graph).unwrap();
        // Index 1 must appear as "in/1", not as a separate input_index key.
        assert!(yaml.contains("in/1"), "expected 'in/1' in YAML, got:\n{yaml}");
        assert!(!yaml.contains("input_index"), "unexpected 'input_index' key in YAML");

        // Round-trip: the edge must survive with correct index.
        let graph2 = yaml_to_graph(&yaml, &registry).unwrap();
        let edges = graph2.edge_list();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].4, "in");
        assert_eq!(edges[0].5, 1);
    }

    #[test]
    fn scale_is_omitted_at_one_and_restored_on_load() {
        let registry = make_registry();
        let osc_id = NodeId::from("o");
        let gain_id = NodeId::from("g");

        let osc_desc =
            registry.describe("SineOsc", &ModuleShape { channels: 1, length: 0 }).unwrap();
        let gain_desc =
            registry.describe("Gain", &ModuleShape { channels: 1, length: 0 }).unwrap();

        let mut graph = ModuleGraph::new();
        graph.add_module(osc_id.clone(), osc_desc, &ParameterMap::new()).unwrap();
        graph.add_module(gain_id.clone(), gain_desc, &ParameterMap::new()).unwrap();
        graph
            .connect(
                &osc_id,
                PortRef { name: "out", index: 0 },
                &gain_id,
                PortRef { name: "in", index: 0 },
                1.0,
            )
            .unwrap();

        let yaml = graph_to_yaml(&graph).unwrap();
        assert!(!yaml.contains("scale"), "scale: 1.0 should be omitted from YAML");

        let graph2 = yaml_to_graph(&yaml, &registry).unwrap();
        assert_eq!(graph2.edge_list()[0].6, 1.0);
    }
}
