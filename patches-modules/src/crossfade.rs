use patches_core::{Module, ModuleDescriptor, PortDescriptor, PortDirection};

/// Mixes two input signals at a fixed 50/50 blend.
///
/// Output is `(a + b) / 2.0`, keeping the result in the same amplitude range
/// as the inputs.
pub struct Crossfade {
    descriptor: ModuleDescriptor,
}

impl Crossfade {
    pub fn new() -> Self {
        Self {
            descriptor: ModuleDescriptor {
                inputs: vec![
                    PortDescriptor {
                        name: "a",
                        direction: PortDirection::Input,
                    },
                    PortDescriptor {
                        name: "b",
                        direction: PortDirection::Input,
                    },
                ],
                outputs: vec![PortDescriptor {
                    name: "out",
                    direction: PortDirection::Output,
                }],
            },
        }
    }
}

impl Default for Crossfade {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Crossfade {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64], _sample_rate: f64) {
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
        let m = Crossfade::new();
        let desc = m.descriptor();
        assert_eq!(desc.inputs.len(), 2);
        assert_eq!(desc.inputs[0].name, "a");
        assert_eq!(desc.inputs[1].name, "b");
        assert_eq!(desc.outputs.len(), 1);
        assert_eq!(desc.outputs[0].name, "out");
    }

    #[test]
    fn output_is_average_of_inputs() {
        let mut m = Crossfade::new();
        let mut out = [0.0f64];

        m.process(&[1.0, 0.0], &mut out, 44100.0);
        assert_eq!(out[0], 0.5);

        m.process(&[-1.0, 1.0], &mut out, 44100.0);
        assert_eq!(out[0], 0.0);

        m.process(&[0.4, 0.6], &mut out, 44100.0);
        assert!((out[0] - 0.5).abs() < f64::EPSILON);
    }
}
