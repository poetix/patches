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
/// `as_any` enables downcasting from `&dyn Module` to a concrete type — for
/// example, the patch builder uses it to locate the `AudioOut` node by type.
pub trait Module: Send {
    fn descriptor(&self) -> &ModuleDescriptor;
    fn process(&mut self, inputs: &[f64], outputs: &mut [f64], sample_rate: f64);
    fn as_any(&self) -> &dyn std::any::Any;
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
