use patches_core::{InstanceId, Module, ModuleDescriptor, PortDescriptor};

/// Voltage-controlled amplifier. Multiplies a signal by a control voltage.
///
/// Input ports: `in/0` (signal), `cv/1` (control voltage).
/// Output port: `out/0`.
///
/// No clamping is applied to the CV input; amplification above 1.0 and phase
/// inversion with negative CV are valid use cases.
pub struct Vca {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
}

impl Vca {
    pub fn new() -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                inputs: vec![
                    PortDescriptor { name: "in", index: 0 },
                    PortDescriptor { name: "cv", index: 0 },
                ],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
        }
    }
}

impl Default for Vca {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for Vca {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        outputs[0] = inputs[0] * inputs[1];
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_shape() {
        let m = Vca::new();
        let desc = m.descriptor();
        assert_eq!(desc.inputs.len(), 2);
        assert_eq!(desc.outputs.len(), 1);
        assert_eq!(desc.inputs[0].name, "in");
        assert_eq!(desc.inputs[0].index, 0);
        assert_eq!(desc.inputs[1].name, "cv");
        assert_eq!(desc.inputs[1].index, 1);
        assert_eq!(desc.outputs[0].name, "out");
        assert_eq!(desc.outputs[0].index, 0);
    }

    #[test]
    fn multiplies_signal_by_cv() {
        let mut m = Vca::new();
        let mut out = [0.0f64];
        m.process(&[0.5, 0.8], &mut out);
        assert!((out[0] - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn zero_cv_silences_signal() {
        let mut m = Vca::new();
        let mut out = [0.0f64];
        m.process(&[1.0, 0.0], &mut out);
        assert_eq!(out[0], 0.0);
    }

    #[test]
    fn negative_cv_inverts_phase() {
        let mut m = Vca::new();
        let mut out = [0.0f64];
        m.process(&[0.5, -1.0], &mut out);
        assert!((out[0] - (-0.5)).abs() < f64::EPSILON);
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = Vca::new();
        let b = Vca::new();
        assert_ne!(a.instance_id(), b.instance_id());
    }
}
