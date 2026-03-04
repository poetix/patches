use patches_core::{AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleShape, PortDescriptor};
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
}

impl Module for Vca {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Vca",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "in", index: 0 },
                PortDescriptor { name: "cv", index: 0 },
            ],
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
            parameters: vec![],
        }
    }

    fn prepare(_audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor) -> Self {
        Self {
            instance_id: InstanceId::next(),
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
        outputs[0] = inputs[0] * inputs[1];
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

    fn make_vca() -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<Vca>();
        r.create(
            "Vca",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels: 0 },
            &ParameterMap::new(),
        ).unwrap()
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
        let mut out = [0.0f64];
        m.process(&[0.5, 0.8], &mut out);
        assert!((out[0] - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn zero_cv_silences_signal() {
        let mut m = make_vca();
        let mut out = [0.0f64];
        m.process(&[1.0, 0.0], &mut out);
        assert_eq!(out[0], 0.0);
    }

    #[test]
    fn negative_cv_inverts_phase() {
        let mut m = make_vca();
        let mut out = [0.0f64];
        m.process(&[0.5, -1.0], &mut out);
        assert!((out[0] - (-0.5)).abs() < f64::EPSILON);
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_vca();
        let b = make_vca();
        assert_ne!(a.instance_id(), b.instance_id());
    }
}
