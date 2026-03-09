use patches_core::{AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleShape, PortDescriptor, Sink};
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
}

impl Module for AudioOut {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "AudioOut",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "left", index: 0 },
                PortDescriptor { name: "right", index: 0 },
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

    fn process(&mut self, inputs: &[f64], _outputs: &mut [f64]) {
        self.last_left = inputs[0];
        self.last_right = inputs[1];
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
    use patches_core::{AudioEnvironment, Module, ModuleShape, Registry};
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

    #[test]
    fn descriptor_has_two_inputs_and_no_outputs() {
        let module = make_audio_out();
        let desc = module.descriptor();
        assert_eq!(desc.inputs.len(), 2);
        assert_eq!(desc.inputs[0].name, "left");
        assert_eq!(desc.inputs[1].name, "right");
        assert_eq!(desc.outputs.len(), 0);
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_audio_out();
        let b = make_audio_out();
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn process_stores_left_and_right_samples() {
        let mut module = make_audio_out();
        let sink = module.as_sink().unwrap();
        assert_eq!(sink.last_right(), 0.0);

        module.process(&[0.5, -0.3], &mut []);
        let sink = module.as_sink().unwrap();
        assert_eq!(sink.last_left(), 0.5);
        assert_eq!(sink.last_right(), -0.3);

        module.process(&[1.0, 0.0], &mut []);
        let sink = module.as_sink().unwrap();
        assert_eq!(sink.last_left(), 1.0);
        assert_eq!(sink.last_right(), 0.0);
    }
}
