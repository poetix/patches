use patches_core::{InstanceId, Module, ModuleDescriptor, PortDescriptor};

/// Mixes two input signals at a fixed 50/50 blend.
///
/// Output is `(a + b) / 2.0`, keeping the result in the same amplitude range
/// as the inputs.
pub struct Mix {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
}

impl Mix {
    pub fn new() -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                inputs: vec![
                    PortDescriptor { name: "a", index: 0 },
                    PortDescriptor { name: "b", index: 0 },
                ],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
        }
    }
}

impl Default for Mix {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Mix {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        outputs[0] = (inputs[0] + inputs[1]) / 2.0;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_has_two_inputs_and_one_output() {
        let m = Mix::new();
        let desc = m.descriptor();
        assert_eq!(desc.inputs.len(), 2);
        assert_eq!(desc.inputs[0].name, "a");
        assert_eq!(desc.inputs[1].name, "b");
        assert_eq!(desc.outputs.len(), 1);
        assert_eq!(desc.outputs[0].name, "out");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = Mix::new();
        let b = Mix::new();
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn output_is_average_of_inputs() {
        let mut m = Mix::new();
        let mut out = [0.0f64];

        m.process(&[1.0, 0.0], &mut out);
        assert_eq!(out[0], 0.5);

        m.process(&[-1.0, 1.0], &mut out);
        assert_eq!(out[0], 0.0);

        m.process(&[0.4, 0.6], &mut out);
        assert!((out[0] - 0.5).abs() < f64::EPSILON);
    }
}
