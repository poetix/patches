/// Describes a single port on a module by name and index.
///
/// The `index` field is the user-visible number in a multi-port group (e.g.
/// `in/2` has `name = "in"` and `index = 2`). For modules with a single port
/// of a given name, `index` is `0`. The position of a `PortDescriptor` in
/// `ModuleDescriptor::inputs` / `outputs` determines the slice offset passed to
/// `Module::process`; `index` is semantically distinct from that position.
#[derive(Debug, Clone)]
pub struct PortDescriptor {
    pub name: &'static str,
    pub index: u32,
}

/// A reference to a named, indexed port used in `ModuleGraph::connect()`.
///
/// Port names are always `&'static str` (defined by module implementations at
/// compile time), so producing a `PortRef` never allocates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRef {
    pub name: &'static str,
    pub index: u32,
}

use crate::parameter_map::ParameterValue;

#[derive(Debug, Clone)]
pub enum ParameterKind {
    Float { min: f64, max: f64, default: f64 },
    Int   { min: i64, max: i64, default: i64 },
    Bool  { default: bool },
    Enum  { variants: &'static [&'static str], default: &'static str },
    /// Variable-length array of strings (e.g. a step-sequencer pattern).
    ///
    /// The `default` field uses `&'static [&'static str]` so that the descriptor itself
    /// never allocates (consistent with ADR 0011). The `ParameterValue` it produces does
    /// allocate, but only at the non-realtime boundary.
    Array { default: &'static [&'static str] },
}

impl ParameterKind {
    /// Return the default value for this parameter kind as a [`ParameterValue`].
    pub fn default_value(&self) -> ParameterValue {
        match self {
            ParameterKind::Float { default, .. } => ParameterValue::Float(*default),
            ParameterKind::Int   { default, .. } => ParameterValue::Int(*default),
            ParameterKind::Bool  { default }     => ParameterValue::Bool(*default),
            ParameterKind::Enum  { default, .. } => ParameterValue::Enum(default),
            ParameterKind::Array { default }     => ParameterValue::Array(
                default.iter().map(|s| s.to_string()).collect()
            ),
        }
    }

    /// Return a short type name suitable for error messages.
    pub fn kind_name(&self) -> &'static str {
        match self {
            ParameterKind::Float { .. } => "float",
            ParameterKind::Int   { .. } => "int",
            ParameterKind::Bool  { .. } => "bool",
            ParameterKind::Enum  { .. } => "enum",
            ParameterKind::Array { .. } => "array",
        }
    }
}

pub struct ParameterSpec {
    pub name: &'static str,
    pub kind: ParameterKind,
}

#[derive(Debug, Clone)]
pub struct ParameterDescriptor {
    pub name: &'static str,
    pub index: usize,
    pub parameter_type: ParameterKind,
}

#[derive(Debug, Clone)]
pub struct ModuleShape {
    pub channels: usize,
}

/// Describes the full layout of a module.
///
/// Inputs, outputs, and parameters are stored in separate vecs.
/// 
/// The index of a port in `inputs` corresponds to the index in the `inputs` slice passed to
/// [`Module::process`], and similarly for `outputs`. The graph and patch builder
/// use this to resolve port names to slice indices at build time.
#[derive(Debug, Clone)]
pub struct ModuleDescriptor {
    pub module_name: &'static str,
    pub shape: ModuleShape,
    pub inputs: Vec<PortDescriptor>,
    pub outputs: Vec<PortDescriptor>,
    pub parameters: Vec<ParameterDescriptor>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_a_module_descriptor() {
        let gain_amount = ParameterKind::Float { min: 0.0, max: 1.2, default: 1.0 };
        let pan_amount = ParameterKind::Float { min: -1.0, max: 1.0, default: 0.0 };
        let toggle_off_on = ParameterKind::Bool { default: false };

        let m = ModuleDescriptor {
            module_name: "Mixer",
            shape: ModuleShape { channels: 2 },
            inputs: vec![
                PortDescriptor { name: "in", index: 0 },
                PortDescriptor { name: "in", index: 1 },
                PortDescriptor { name: "gain_mod", index: 0 },
                PortDescriptor { name: "gain_mod", index: 1 },
                PortDescriptor { name: "pan_mod", index: 0 },
                PortDescriptor { name: "pan_mod", index: 1 },
            ],
            outputs: vec![
                PortDescriptor { name: "out_l", index: 0 },
                PortDescriptor { name: "out_r", index: 0 },
            ],
            parameters: vec![
                ParameterDescriptor { name: "gain", index: 0, parameter_type: gain_amount.clone() },
                ParameterDescriptor { name: "gain", index: 1, parameter_type: gain_amount },
                ParameterDescriptor { name: "pan", index: 0, parameter_type: pan_amount.clone() },
                ParameterDescriptor { name: "pan", index: 1, parameter_type: pan_amount },
                ParameterDescriptor { name: "mute", index: 0, parameter_type: toggle_off_on.clone() },
                ParameterDescriptor { name: "mute", index: 1, parameter_type: toggle_off_on.clone() },
                ParameterDescriptor { name: "solo", index: 0, parameter_type: toggle_off_on.clone() },
                ParameterDescriptor { name: "solo", index: 0, parameter_type: toggle_off_on },
            ],
        };
        assert_eq!(m.module_name, "Mixer");
        assert_eq!(m.shape.channels, 2);
        assert_eq!(m.inputs.len(), 6);
        assert_eq!(m.outputs.len(), 2);
        assert_eq!(m.parameters.len(), 8);
    }
}