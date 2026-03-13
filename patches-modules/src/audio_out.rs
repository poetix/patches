use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, ModuleShape, OutputPort, CableKind, PortDescriptor, Sink,
};
use patches_core::parameter_map::ParameterMap;

/// A passive stereo sink node.
///
/// `AudioOut` receives left and right audio samples via its two input ports and
/// stores them internally. After each engine tick the sound engine reads them via
/// the [`Sink`] trait and forwards them to the hardware output buffer.
///
/// `AudioOut` does not call any audio API; it knows nothing about the backend.
pub struct AudioOut {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    last_left: f64,
    last_right: f64,
    in_left: MonoInput,
    in_right: MonoInput,
}

impl Module for AudioOut {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "AudioOut",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "left", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "right", index: 0, kind: CableKind::Mono },
            ],
            outputs: vec![],
            parameters: vec![],
            is_sink: true,
        }
    }

    fn prepare(_audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            last_left: 0.0,
            last_right: 0.0,
            in_left: MonoInput::default(),
            in_right: MonoInput::default(),
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

    fn set_ports(&mut self, inputs: &[InputPort], _outputs: &[OutputPort]) {
        self.in_left = MonoInput::from_ports(inputs, 0);
        self.in_right = MonoInput::from_ports(inputs, 1);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        self.last_left = pool.read_mono(&self.in_left);
        self.last_right = pool.read_mono(&self.in_right);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_sink(&self) -> Option<&dyn Sink> {
        Some(self)
    }
}

impl Sink for AudioOut {
    fn last_left(&self) -> f64 {
        self.last_left
    }

    fn last_right(&self) -> f64 {
        self.last_right
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use patches_core::{AudioEnvironment, CablePool, Module, ModuleShape, Registry};
    use patches_core::parameter_map::ParameterMap;

    fn make_audio_out() -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<AudioOut>();
        r.create(
            "AudioOut",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels: 0, length: 0 },
            &ParameterMap::new(),
            InstanceId::next(),
        ).unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

    fn set_ports_for_test(module: &mut Box<dyn Module>) {
        // 0=left, 1=right; no outputs
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: true }),
        ];
        module.set_ports(&inputs, &[]);
    }

    #[test]
    fn process_stores_left_and_right_samples() {
        let mut module = make_audio_out();
        set_ports_for_test(&mut module);
        let sink = module.as_sink().unwrap();
        assert_eq!(sink.last_right(), 0.0);

        let mut pool = make_pool(2);
        pool[0][1] = CableValue::Mono(0.5);
        pool[1][1] = CableValue::Mono(-0.3);
        module.process(&mut CablePool::new(&mut pool, 0));
        let sink = module.as_sink().unwrap();
        assert_eq!(sink.last_left(), 0.5);
        assert_eq!(sink.last_right(), -0.3);

        pool[0][0] = CableValue::Mono(1.0);
        pool[1][0] = CableValue::Mono(0.0);
        module.process(&mut CablePool::new(&mut pool, 1));
        let sink = module.as_sink().unwrap();
        assert_eq!(sink.last_left(), 1.0);
        assert_eq!(sink.last_right(), 0.0);
    }
}
