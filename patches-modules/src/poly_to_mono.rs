use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleShape, MonoOutput, OutputPort, PolyInput, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::ParameterMap;

/// Poly-to-mono summing adapter.
///
/// Sums all `poly_voices` channels from the poly input into a single mono output.
/// No normalisation is applied; callers should scale the output (e.g. via cable
/// scale or a downstream VCA) to avoid clipping with many active voices.
///
/// ## Input ports
/// | Index | Name | Kind |
/// |-------|------|------|
/// | 0     | `in` | Poly |
///
/// ## Output ports
/// | Index | Name  | Kind |
/// |-------|-------|------|
/// | 0     | `out` | Mono |
pub struct PolyToMono {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    voice_count: usize,
    in_poly: PolyInput,
    out_mono: MonoOutput,
}

impl Module for PolyToMono {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "PolyToMono",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "in", index: 0, kind: CableKind::Poly },
            ],
            outputs: vec![
                PortDescriptor { name: "out", index: 0, kind: CableKind::Mono },
            ],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            voice_count: audio_environment.poly_voices.min(16),
            in_poly: PolyInput::default(),
            out_mono: MonoOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

    fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
    fn instance_id(&self) -> InstanceId { self.instance_id }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_poly  = PolyInput::from_ports(inputs, 0);
        self.out_mono = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let channels = pool.read_poly(&self.in_poly);
        let sum: f64 = channels[..self.voice_count].iter().sum();
        pool.write_mono(&self.out_mono, sum);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{AudioEnvironment, CablePool, CableValue, Module, ModuleShape, Registry};

    fn make_collapse(poly_voices: usize) -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<PolyToMono>();
        r.create(
            "PolyToMono",
            &AudioEnvironment { sample_rate: 44100.0, poly_voices },
            &ModuleShape { channels: 0, length: 0 },
            &patches_core::parameter_map::ParameterMap::new(),
            InstanceId::next(),
        )
        .unwrap()
    }

    fn set_ports_for_test(m: &mut Box<dyn Module>) {
        use patches_core::{InputPort, OutputPort, PolyInput, MonoOutput};
        m.set_ports(
            &[InputPort::Poly(PolyInput { cable_idx: 0, scale: 1.0, connected: true })],
            &[OutputPort::Mono(MonoOutput { cable_idx: 1, connected: true })],
        );
    }

    #[test]
    fn sums_active_voices_only() {
        let mut m = make_collapse(4);
        set_ports_for_test(&mut m);
        let mut channels = [0.0f64; 16];
        channels[0] = 0.25;
        channels[1] = 0.25;
        channels[2] = 0.25;
        channels[3] = 0.25;
        channels[4] = 99.0; // beyond voice_count, should not be included

        let mut pool = vec![
            [CableValue::Poly([0.0; 16]); 2],
            [CableValue::Mono(0.0); 2],
        ];
        pool[0][1] = CableValue::Poly(channels);
        m.process(&mut CablePool::new(&mut pool, 0));
        match pool[1][0] {
            CableValue::Mono(v) => assert!((v - 1.0).abs() < f64::EPSILON, "expected 1.0, got {v}"),
            _ => panic!("expected Mono"),
        }
    }

    #[test]
    fn zero_voices_produce_zero() {
        let mut m = make_collapse(4);
        set_ports_for_test(&mut m);
        let mut pool = vec![
            [CableValue::Poly([0.0; 16]); 2],
            [CableValue::Mono(0.0); 2],
        ];
        m.process(&mut CablePool::new(&mut pool, 0));
        match pool[1][0] {
            CableValue::Mono(v) => assert_eq!(v, 0.0),
            _ => panic!("expected Mono"),
        }
    }
}
