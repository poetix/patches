use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, PortDescriptor, Sink};
use patches_core::module::ParameterSet;
use patches_core::module::Factory;

pub struct AudioOutFactory {}

impl AudioOutFactory {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for AudioOutFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl Factory for AudioOutFactory {
    fn get_module_name(&self) -> &'static str {
        "AudioOut"
    }

    fn descriptor(&self, size: usize) -> &ModuleDescriptor {
        ModuleDescriptor {
            module_name: self.get_module_name(),
            inputs: vec![
                PortDescriptor { name: "left", index: 0 },
                PortDescriptor { name: "right", index: 0 },
            ],
            outputs: vec![],
            parameters: vec![],
            defaults: vec![],
        }
    }

    fn create(
        &self,
        audio_env: &AudioEnvironment,
        descriptor: &ModuleDescriptor,
        parameters: &ParameterSet) -> Box<dyn Module> {
            Box::new(AudioOut::new(descriptor.clone()))
        }
}

/// A passive stereo sink node.
///
/// `AudioOut` receives left and right audio samples via its two input ports and
/// stores them internally. After each engine tick the sound engine reads them via
/// [`last_left`](AudioOut::last_left) and [`last_right`](AudioOut::last_right) and
/// forwards them to the hardware output buffer.
///
/// `AudioOut` does not call any audio API; it knows nothing about the backend.
pub struct AudioOut {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    last_left: f64,
    last_right: f64,
}

impl AudioOut {
    pub fn new(descriptor: ModuleDescriptor) -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor,
            last_left: 0.0,
            last_right: 0.0,
        }
    }

    /// The left-channel sample stored during the most recent [`process`](Module::process) call.
    pub fn last_left(&self) -> f64 {
        self.last_left
    }

    /// The right-channel sample stored during the most recent [`process`](Module::process) call.
    pub fn last_right(&self) -> f64 {
        self.last_right
    }
}

impl Default for AudioOut {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for AudioOut {

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

    fn make_module() -> AudioOut {
        let factory = AudioOutFactory::new();
        let descriptor = factory.descriptor(0);
        factory.create(
            &AudioEnvironment::new(44100.0),
            &descriptor,
            &ParameterSet::default()).as_any().downcast_ref::<AudioOut>().unwrap().clone()
    }

    #[test]
    fn descriptor_has_two_inputs_and_no_outputs() {
        let sink = make_module();
        let desc = sink.descriptor();
        assert_eq!(desc.inputs.len(), 2);
        assert_eq!(desc.inputs[0].name, "left");
        assert_eq!(desc.inputs[1].name, "right");
        assert_eq!(desc.outputs.len(), 0);
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_module();
        let b = make_module();
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn process_stores_left_and_right_samples() {
        let mut sink = make_module();
        assert_eq!(sink.last_right(), 0.0);

        module.process(&[0.5, -0.3], &mut []);
        assert_eq!(sink.last_left(), 0.5);
        assert_eq!(sink.last_right(), -0.3);

        module.process(&[1.0, 0.0], &mut []);
        assert_eq!(sink.last_left(), 1.0);
        assert_eq!(sink.last_right(), 0.0);
    }
}
