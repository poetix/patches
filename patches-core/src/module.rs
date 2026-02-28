/// Describes a single port on a module by name.
#[derive(Debug, Clone)]
pub struct PortDescriptor {
    pub name: &'static str,
}

/// Describes the full port layout of a module.
///
/// Inputs and outputs are stored in separate vecs. The index of a port in
/// `inputs` corresponds to the index in the `inputs` slice passed to
/// [`Module::process`], and similarly for `outputs`. The graph and patch builder
/// use this to resolve port names to slice indices at build time.
#[derive(Debug, Clone)]
pub struct ModuleDescriptor {
    pub inputs: Vec<PortDescriptor>,
    pub outputs: Vec<PortDescriptor>,
}

/// The core trait all audio modules implement.
///
/// `process` is called once per sample by the audio engine. `inputs` and
/// `outputs` are indexed according to the module's [`ModuleDescriptor`].
///
/// `as_any` enables downcasting from `&dyn Module` to a concrete type.
/// `as_sink` lets the patch builder detect a sink node without knowing its
/// concrete type — see [`Sink`].
pub trait Module: Send {
    fn descriptor(&self) -> &ModuleDescriptor;
    fn process(&mut self, inputs: &[f64], outputs: &mut [f64], sample_rate: f64);
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
    /// Returns `Some(self)` if this module is a [`Sink`], `None` otherwise.
    fn as_sink(&self) -> Option<&dyn Sink> {
        None
    }
}

/// Marker trait for modules that act as the final audio output in a patch.
///
/// The patch builder uses this to locate the sink node without knowledge of
/// any concrete type. Implement this alongside [`Module`] and override
/// [`Module::as_sink`] to return `Some(self)`.
pub trait Sink: Module {
    /// Left-channel sample stored during the most recent [`Module::process`] call.
    fn last_left(&self) -> f64;
    /// Right-channel sample stored during the most recent [`Module::process`] call.
    fn last_right(&self) -> f64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_descriptor_fields() {
        let p = PortDescriptor { name: "freq" };
        assert_eq!(p.name, "freq");
    }

    #[test]
    fn module_descriptor_port_counts() {
        let desc = ModuleDescriptor {
            inputs: vec![PortDescriptor { name: "in" }],
            outputs: vec![
                PortDescriptor { name: "out_l" },
                PortDescriptor { name: "out_r" },
            ],
        };
        assert_eq!(desc.inputs.len(), 1);
        assert_eq!(desc.outputs.len(), 2);
        assert_eq!(desc.inputs[0].name, "in");
        assert_eq!(desc.outputs[1].name, "out_r");
    }
}
