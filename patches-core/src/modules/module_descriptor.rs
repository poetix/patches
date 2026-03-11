/// Describes a single port on a module by name and index.
///
/// The `index` field is the user-visible number in a multi-port group (e.g.
/// `in/2` has `name = "in"` and `index = 2`). For modules with a single port
/// of a given name, `index` is `0`. The position of a `PortDescriptor` in
/// `ModuleDescriptor::inputs` / `outputs` determines the slice offset passed to
/// `Module::process`; `index` is semantically distinct from that position.
///
/// `kind` declares whether the port carries a mono or poly signal. Port arity
/// is fixed at module-definition time and used by [`ModuleGraph::connect`] to
/// reject kind-mismatched connections at graph-construction time.
#[derive(Debug, Clone)]
pub struct PortDescriptor {
    pub name: &'static str,
    pub index: u32,
    pub kind: crate::cables::CableKind,
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

use super::parameter_map::ParameterValue;

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
    ///
    /// `length` is the maximum number of elements the pre-allocated backing array can hold.
    /// It must match `ModuleShape::length` for the module that declares this parameter.
    /// `validate_parameters` rejects any `ParameterValue::Array` whose element count
    /// exceeds this limit.
    Array { default: &'static [&'static str], length: usize },
}

impl ParameterKind {
    /// Return the default value for this parameter kind as a [`ParameterValue`].
    pub fn default_value(&self) -> ParameterValue {
        match self {
            ParameterKind::Float { default, .. } => ParameterValue::Float(*default),
            ParameterKind::Int   { default, .. } => ParameterValue::Int(*default),
            ParameterKind::Bool  { default }     => ParameterValue::Bool(*default),
            ParameterKind::Enum  { default, .. } => ParameterValue::Enum(default),
            ParameterKind::Array { default, .. } => ParameterValue::Array(
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

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleShape {
    pub channels: usize,
    /// Pre-allocated step/slot count for sequencer-style modules.
    ///
    /// Set to `0` for modules that do not use array parameters. When non-zero,
    /// the module factory uses this value to pre-allocate the backing array so
    /// that subsequent `update_parameters` calls can write into the existing
    /// allocation. If the shape changes between builds the planner will
    /// tombstone the old instance and create a fresh one.
    pub length: usize,
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
    pub is_sink: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cables::CableKind;

    #[test]
    fn build_a_module_descriptor() {
        let gain_amount = ParameterKind::Float { min: 0.0, max: 1.2, default: 1.0 };
        let pan_amount = ParameterKind::Float { min: -1.0, max: 1.0, default: 0.0 };
        let toggle_off_on = ParameterKind::Bool { default: false };

        let m = ModuleDescriptor {
            module_name: "Mixer",
            shape: ModuleShape { channels: 2, length: 0 },
            inputs: vec![
                PortDescriptor { name: "in", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "in", index: 1, kind: CableKind::Mono },
                PortDescriptor { name: "gain_mod", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "gain_mod", index: 1, kind: CableKind::Mono },
                PortDescriptor { name: "pan_mod", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "pan_mod", index: 1, kind: CableKind::Mono },
            ],
            outputs: vec![
                PortDescriptor { name: "out_l", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "out_r", index: 0, kind: CableKind::Mono },
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
            is_sink: false,
        };
        assert_eq!(m.module_name, "Mixer");
        assert_eq!(m.shape.channels, 2);
        assert_eq!(m.shape.length, 0);
        assert_eq!(m.inputs.len(), 6);
        assert_eq!(m.outputs.len(), 2);
        assert_eq!(m.parameters.len(), 8);
    }
}