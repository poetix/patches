use patches_core::{InstanceId, Module, ModuleDescriptor, PortDescriptor};

/// Sums a configurable number of input signals into a single output.
///
/// The number of inputs is fixed at construction time. All inputs are summed
/// with no normalisation: `output = in/0 + in/1 + … + in/(size-1)`.
pub struct Sum {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    size: usize,
}

impl Sum {
    /// Construct a `Sum` with `size` input ports (`in/0` … `in/(size-1)`)
    /// and a single output port (`out/0`).
    ///
    /// `size = 0` is valid: the descriptor has no inputs and the output is
    /// always 0.0.
    pub fn new(size: usize) -> Self {
        let inputs = (0..size)
            .map(|i| PortDescriptor { name: "in", index: i as u32 })
            .collect();
        Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                inputs,
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
            size,
        }
    }
}

impl Module for Sum {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        outputs[0] = inputs[..self.size].iter().sum();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_shape_size_3() {
        let m = Sum::new(3);
        let desc = m.descriptor();
        assert_eq!(desc.inputs.len(), 3);
        assert_eq!(desc.outputs.len(), 1);
        for (i, port) in desc.inputs.iter().enumerate() {
            assert_eq!(port.name, "in");
            assert_eq!(port.index, i as u32);
        }
        assert_eq!(desc.outputs[0].name, "out");
        assert_eq!(desc.outputs[0].index, 0);
    }

    #[test]
    fn size_1_passes_input_unchanged() {
        let mut m = Sum::new(1);
        let mut out = [0.0f64];
        m.process(&[0.75], &mut out);
        assert_eq!(out[0], 0.75);
    }

    #[test]
    fn size_3_sums_inputs() {
        let mut m = Sum::new(3);
        let mut out = [0.0f64];
        m.process(&[0.2, 0.3, 0.5], &mut out);
        assert!((out[0] - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = Sum::new(2);
        let b = Sum::new(2);
        assert_ne!(a.instance_id(), b.instance_id());
    }
}
