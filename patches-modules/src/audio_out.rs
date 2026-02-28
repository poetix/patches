use patches_core::{Module, ModuleDescriptor, PortDescriptor, Sink};

/// A passive stereo sink node.
///
/// `AudioOut` receives left and right audio samples via its two input ports and
/// stores them internally. After each engine tick the sound engine reads them via
/// [`last_left`](AudioOut::last_left) and [`last_right`](AudioOut::last_right) and
/// forwards them to the hardware output buffer.
///
/// `AudioOut` does not call any audio API; it knows nothing about the backend.
pub struct AudioOut {
    last_left: f64,
    last_right: f64,
    descriptor: ModuleDescriptor,
}

impl AudioOut {
    pub fn new() -> Self {
        Self {
            last_left: 0.0,
            last_right: 0.0,
            descriptor: ModuleDescriptor {
                inputs: vec![
                    PortDescriptor { name: "left" },
                    PortDescriptor { name: "right" },
                ],
                outputs: vec![],
            },
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

    fn process(&mut self, inputs: &[f64], _outputs: &mut [f64], _sample_rate: f64) {
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

    #[test]
    fn descriptor_has_two_inputs_and_no_outputs() {
        let sink = AudioOut::new();
        let desc = sink.descriptor();
        assert_eq!(desc.inputs.len(), 2);
        assert_eq!(desc.inputs[0].name, "left");
        assert_eq!(desc.inputs[1].name, "right");
        assert_eq!(desc.outputs.len(), 0);
    }

    #[test]
    fn process_stores_left_and_right_samples() {
        let mut sink = AudioOut::new();
        assert_eq!(sink.last_left(), 0.0);
        assert_eq!(sink.last_right(), 0.0);

        sink.process(&[0.5, -0.3], &mut [], 44100.0);
        assert_eq!(sink.last_left(), 0.5);
        assert_eq!(sink.last_right(), -0.3);

        sink.process(&[1.0, 0.0], &mut [], 44100.0);
        assert_eq!(sink.last_left(), 1.0);
        assert_eq!(sink.last_right(), 0.0);
    }
}
