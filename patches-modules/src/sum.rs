use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleShape, PortDescriptor,
};
use patches_core::parameter_map::ParameterMap;

/// Sums a configurable number of input signals into a single output.
///
/// The number of inputs is determined by `ModuleShape::channels` at build time.
/// All inputs are summed with no normalisation:
/// `output = in/0 + in/1 + … + in/(size-1)`.
///
/// Constructed via the Module v2 protocol: `describe` → `prepare` →
/// `update_validated_parameters`.
pub struct Sum {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    size: usize,
}

impl Module for Sum {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        let inputs = (0..shape.channels)
            .map(|i| PortDescriptor { name: "in", index: i as u32 })
            .collect();
        ModuleDescriptor {
            module_name: "Sum",
            shape: shape.clone(),
            inputs,
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(_audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        let size = descriptor.shape.channels;
        Self {
            instance_id,
            size,
            descriptor,
        }
    }

    fn update_validated_parameters(&mut self, _params: &ParameterMap) {
    }

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
    use patches_core::{AudioEnvironment, Module, ModuleShape, Registry};
    use patches_core::parameter_map::ParameterMap;

    fn make_sum(channels: usize) -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<Sum>();
        r.create(
            "Sum",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels, length: 0 },
            &ParameterMap::new(),
            InstanceId::next(),
        ).unwrap()
    }

    #[test]
    fn descriptor_shape_size_3() {
        let m = make_sum(3);
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
        let mut m = make_sum(1);
        let mut out = [0.0f64];
        m.process(&[0.75], &mut out);
        assert_eq!(out[0], 0.75);
    }

    #[test]
    fn size_3_sums_inputs() {
        let mut m = make_sum(3);
        let mut out = [0.0f64];
        m.process(&[0.2, 0.3, 0.5], &mut out);
        assert!((out[0] - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_sum(2);
        let b = make_sum(2);
        assert_ne!(a.instance_id(), b.instance_id());
    }
}
