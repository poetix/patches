use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, ModuleShape, OutputPort, CableKind, PortDescriptor,
};
use patches_core::parameter_map::ParameterMap;

/// Voltage-controlled amplifier. Multiplies a signal by a control voltage.
///
/// Input ports: `in/0` (signal), `cv/0` (control voltage).
/// Output port: `out/0`.
///
/// No clamping is applied to the CV input; amplification above 1.0 and phase
/// inversion with negative CV are valid use cases.
pub struct Vca {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    in_signal: MonoInput,
    in_cv: MonoInput,
    out_audio: MonoOutput,
}

impl Module for Vca {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Vca",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "in", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "cv", index: 0, kind: CableKind::Mono },
            ],
            outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Mono }],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(_audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            in_signal: MonoInput::default(),
            in_cv: MonoInput::default(),
            out_audio: MonoOutput::default(),
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
        self.in_signal = MonoInput::from_ports(inputs, 0);
        self.in_cv = MonoInput::from_ports(inputs, 1);
        self.out_audio = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let signal = pool.read_mono(&self.in_signal);
        let cv = pool.read_mono(&self.in_cv);
        pool.write_mono(&self.out_audio, signal * cv);
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

    fn make_vca() -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<Vca>();
        r.create(
            "Vca",
            &AudioEnvironment { sample_rate: 44100.0, poly_voices: 16 },
            &ModuleShape { channels: 0, length: 0 },
            &ParameterMap::new(),
            InstanceId::next(),
        ).unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

    fn set_ports_for_test(module: &mut Box<dyn Module>) {
        // 0=in, 1=cv, 2=out
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: true }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 2, connected: true }),
        ];
        module.set_ports(&inputs, &outputs);
    }

    #[test]
    fn descriptor_shape() {
        let m = make_vca();
        let desc = m.descriptor();
        assert_eq!(desc.inputs.len(), 2);
        assert_eq!(desc.outputs.len(), 1);
        assert_eq!(desc.inputs[0].name, "in");
        assert_eq!(desc.inputs[0].index, 0);
        assert_eq!(desc.inputs[1].name, "cv");
        assert_eq!(desc.inputs[1].index, 0);
        assert_eq!(desc.outputs[0].name, "out");
        assert_eq!(desc.outputs[0].index, 0);
    }

    #[test]
    fn multiplies_signal_by_cv() {
        let mut m = make_vca();
        set_ports_for_test(&mut m);
        let mut pool = make_pool(3);
        pool[0][1] = CableValue::Mono(0.5);
        pool[1][1] = CableValue::Mono(0.8);
        m.process(&mut CablePool::new(&mut pool, 0));
        if let CableValue::Mono(v) = pool[2][0] {
            assert!((v - 0.4).abs() < f64::EPSILON);
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn zero_cv_silences_signal() {
        let mut m = make_vca();
        set_ports_for_test(&mut m);
        let mut pool = make_pool(3);
        pool[0][1] = CableValue::Mono(1.0);
        pool[1][1] = CableValue::Mono(0.0);
        m.process(&mut CablePool::new(&mut pool, 0));
        if let CableValue::Mono(v) = pool[2][0] {
            assert_eq!(v, 0.0);
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn negative_cv_inverts_phase() {
        let mut m = make_vca();
        set_ports_for_test(&mut m);
        let mut pool = make_pool(3);
        pool[0][1] = CableValue::Mono(0.5);
        pool[1][1] = CableValue::Mono(-1.0);
        m.process(&mut CablePool::new(&mut pool, 0));
        if let CableValue::Mono(v) = pool[2][0] {
            assert!((v - (-0.5)).abs() < f64::EPSILON);
        } else { panic!("expected Mono"); }
    }
}
