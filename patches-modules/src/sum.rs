use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, ModuleShape, OutputPort, PortDescriptor,
};
use patches_core::CableKind;
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
    // Port fields
    in_ports: Vec<MonoInput>,
    out_port: MonoOutput,
}

impl Module for Sum {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        let inputs = (0..shape.channels)
            .map(|i| PortDescriptor { name: "in", index: i as u32, kind: CableKind::Mono })
            .collect();
        ModuleDescriptor {
            module_name: "Sum",
            shape: shape.clone(),
            inputs,
            outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Mono }],
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
            in_ports: vec![MonoInput::default(); size],
            out_port: MonoOutput::default(),
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

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        for i in 0..self.size {
            self.in_ports[i] = MonoInput::from_ports(inputs, i);
        }
        self.out_port = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let total: f32 = self.in_ports[..self.size]
            .iter()
            .map(|p| pool.read_mono(p))
            .sum();
        pool.write_mono(&self.out_port, total);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use patches_core::{AudioEnvironment, CablePool, CableValue, Module, ModuleShape, Registry};
    use patches_core::parameter_map::ParameterMap;

    fn make_sum(channels: usize) -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<Sum>();
        r.create(
            "Sum",
            &AudioEnvironment { sample_rate: 44100.0, poly_voices: 16 },
            &ModuleShape { channels, length: 0 },
            &ParameterMap::new(),
            InstanceId::next(),
        ).unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

    fn set_ports_for_test(module: &mut Box<dyn Module>, n_inputs: usize) {
        let inputs: Vec<InputPort> = (0..n_inputs)
            .map(|i| InputPort::Mono(MonoInput { cable_idx: i, scale: 1.0, connected: true }))
            .collect();
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: n_inputs, connected: true }),
        ];
        module.set_ports(&inputs, &outputs);
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
        set_ports_for_test(&mut m, 1);
        let mut pool = make_pool(2);
        pool[0][1] = CableValue::Mono(0.75);
        m.process(&mut CablePool::new(&mut pool, 0));
        if let CableValue::Mono(v) = pool[1][0] {
            assert_eq!(v, 0.75);
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn size_3_sums_inputs() {
        let mut m = make_sum(3);
        set_ports_for_test(&mut m, 3);
        let mut pool = make_pool(4);
        pool[0][1] = CableValue::Mono(0.2);
        pool[1][1] = CableValue::Mono(0.3);
        pool[2][1] = CableValue::Mono(0.5);
        m.process(&mut CablePool::new(&mut pool, 0));
        if let CableValue::Mono(v) = pool[3][0] {
            assert!((v - 1.0).abs() < f32::EPSILON);
        } else { panic!("expected Mono"); }
    }
}
