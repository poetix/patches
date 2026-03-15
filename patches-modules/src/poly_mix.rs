use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleShape, OutputPort, PolyInput, PolyOutput, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::ParameterMap;

/// Polyphonic mixer: sums N poly inputs into one poly output, per-voice.
///
/// The number of inputs is set by `ModuleShape::channels` at build time.
/// For each voice channel `i`: `out[i] = in/0[i] + in/1[i] + … + in/(N-1)[i]`.
/// No normalisation is applied.
///
/// Useful for blending multiple waveforms from [`PolyOsc`] before applying a
/// per-voice envelope and VCA.
///
/// ## Input ports (N = `channels`)
/// | Index | Name | Kind |
/// |-------|------|------|
/// | 0…N-1 | `in` | Poly |
///
/// ## Output ports
/// | Index | Name  | Kind |
/// |-------|-------|------|
/// | 0     | `out` | Poly |
pub struct PolyMix {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    size: usize,
    in_ports: Vec<PolyInput>,
    out_port: PolyOutput,
}

impl Module for PolyMix {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        let inputs = (0..shape.channels)
            .map(|i| PortDescriptor { name: "in", index: i as u32, kind: CableKind::Poly })
            .collect();
        ModuleDescriptor {
            module_name: "PolyMix",
            shape: shape.clone(),
            inputs,
            outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Poly }],
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
            in_ports: vec![PolyInput::default(); size],
            out_port: PolyOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

    fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
    fn instance_id(&self) -> InstanceId { self.instance_id }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        for i in 0..self.size {
            self.in_ports[i] = PolyInput::from_ports(inputs, i);
        }
        self.out_port = PolyOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let mut out = [0.0f32; 16];
        for port in &self.in_ports[..self.size] {
            let channels = pool.read_poly(port);
            for i in 0..16 {
                out[i] += channels[i];
            }
        }
        pool.write_poly(&self.out_port, out);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{AudioEnvironment, CablePool, CableValue, Module, ModuleShape, Registry};

    fn make_mix(channels: usize) -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<PolyMix>();
        r.create(
            "PolyMix",
            &AudioEnvironment { sample_rate: 44100.0, poly_voices: 4 },
            &ModuleShape { channels, length: 0 },
            &patches_core::parameter_map::ParameterMap::new(),
            InstanceId::next(),
        )
        .unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Poly([0.0; 16]); 2]; n]
    }

    #[test]
    fn two_inputs_summed_per_voice() {
        use patches_core::{InputPort, OutputPort, PolyInput, PolyOutput};
        let mut m = make_mix(2);
        m.set_ports(
            &[
                InputPort::Poly(PolyInput { cable_idx: 0, scale: 1.0, connected: true }),
                InputPort::Poly(PolyInput { cable_idx: 1, scale: 1.0, connected: true }),
            ],
            &[OutputPort::Poly(PolyOutput { cable_idx: 2, connected: true })],
        );

        let mut a = [0.0f32; 16];
        let mut b = [0.0f32; 16];
        a[0] = 0.3; b[0] = 0.7;
        a[1] = 0.5; b[1] = 0.5;

        let mut pool = make_pool(3);
        pool[0][1] = CableValue::Poly(a);
        pool[1][1] = CableValue::Poly(b);
        m.process(&mut CablePool::new(&mut pool, 0));

        let out = match pool[2][0] {
            CableValue::Poly(v) => v,
            _ => panic!("expected Poly"),
        };
        assert!((out[0] - 1.0).abs() < f32::EPSILON, "voice 0: 0.3+0.7=1.0, got {}", out[0]);
        assert!((out[1] - 1.0).abs() < f32::EPSILON, "voice 1: 0.5+0.5=1.0, got {}", out[1]);
    }
}
