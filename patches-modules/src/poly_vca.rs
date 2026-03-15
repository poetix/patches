use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleShape, OutputPort, PolyInput, PolyOutput, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::ParameterMap;

/// Polyphonic voltage-controlled amplifier.
///
/// Multiplies each voice's signal by its corresponding CV channel.
/// No clamping is applied; negative CV inverts phase.
///
/// ## Input ports
/// | Index | Name | Kind |
/// |-------|------|------|
/// | 0     | `in` | Poly |
/// | 1     | `cv` | Poly |
///
/// ## Output ports
/// | Index | Name  | Kind |
/// |-------|-------|------|
/// | 0     | `out` | Poly |
pub struct PolyVca {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    in_signal: PolyInput,
    in_cv: PolyInput,
    out_audio: PolyOutput,
}

impl Module for PolyVca {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "PolyVca",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "in", index: 0, kind: CableKind::Poly },
                PortDescriptor { name: "cv", index: 0, kind: CableKind::Poly },
            ],
            outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Poly }],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(_audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            in_signal: PolyInput::default(),
            in_cv: PolyInput::default(),
            out_audio: PolyOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

    fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
    fn instance_id(&self) -> InstanceId { self.instance_id }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_signal = PolyInput::from_ports(inputs, 0);
        self.in_cv     = PolyInput::from_ports(inputs, 1);
        self.out_audio = PolyOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let signal = pool.read_poly(&self.in_signal);
        let cv     = pool.read_poly(&self.in_cv);
        let mut out = [0.0f32; 16];
        for i in 0..16 {
            out[i] = signal[i] * cv[i];
        }
        pool.write_poly(&self.out_audio, out);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{AudioEnvironment, CablePool, CableValue, Module, ModuleShape, Registry};

    fn make_vca() -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<PolyVca>();
        r.create(
            "PolyVca",
            &AudioEnvironment { sample_rate: 44100.0, poly_voices: 4 },
            &ModuleShape { channels: 0, length: 0 },
            &patches_core::parameter_map::ParameterMap::new(),
            InstanceId::next(),
        )
        .unwrap()
    }

    fn set_ports_for_test(m: &mut Box<dyn Module>) {
        use patches_core::{InputPort, OutputPort, PolyInput, PolyOutput};
        m.set_ports(
            &[
                InputPort::Poly(PolyInput { cable_idx: 0, scale: 1.0, connected: true }),
                InputPort::Poly(PolyInput { cable_idx: 1, scale: 1.0, connected: true }),
            ],
            &[OutputPort::Poly(PolyOutput { cable_idx: 2, connected: true })],
        );
    }

    #[test]
    fn multiplies_per_voice() {
        let mut m = make_vca();
        set_ports_for_test(&mut m);
        let mut sig = [0.0f32; 16];
        let mut cv  = [0.0f32; 16];
        sig[0] = 0.5;  cv[0] = 0.8;
        sig[1] = 1.0;  cv[1] = 0.0;
        sig[2] = -0.5; cv[2] = 1.0;

        let mut pool = vec![
            [CableValue::Poly([0.0; 16]); 2],
            [CableValue::Poly([0.0; 16]); 2],
            [CableValue::Poly([0.0; 16]); 2],
        ];
        pool[0][1] = CableValue::Poly(sig);
        pool[1][1] = CableValue::Poly(cv);
        m.process(&mut CablePool::new(&mut pool, 0));
        let out = match pool[2][0] {
            CableValue::Poly(v) => v,
            _ => panic!("expected Poly"),
        };
        assert!((out[0] - 0.4).abs()  < f32::EPSILON, "voice 0: 0.5×0.8=0.4, got {}", out[0]);
        assert_eq!(out[1], 0.0,                         "voice 1: 1.0×0.0=0.0");
        assert!((out[2] - (-0.5)).abs() < f32::EPSILON, "voice 2: -0.5×1.0=-0.5, got {}", out[2]);
    }
}
