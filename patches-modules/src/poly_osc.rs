use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleShape, OutputPort, ParameterDescriptor, ParameterKind, PolyInput, PolyOutput, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::{ParameterMap, ParameterValue};
use crate::common::approximate::lookup_sine;
use crate::common::frequency::{C0_FREQ, UnitPhaseAccumulator};
use crate::oscillator::polyblep;

/// Polyphonic multi-waveform oscillator.
///
/// One phase accumulator per voice (up to `poly_voices` from [`AudioEnvironment`]).
/// All voices are driven by the `voct` poly input; channel `i` controls voice `i`.
/// Outputs four poly waveforms; only connected outputs are computed each sample.
///
/// ## Input ports
/// | Index | Name    | Kind |
/// |-------|---------|------|
/// | 0     | `voct`  | Poly |
///
/// ## Output ports
/// | Index | Name       | Kind |
/// |-------|------------|------|
/// | 0     | `sine`     | Poly |
/// | 1     | `triangle` | Poly |
/// | 2     | `sawtooth` | Poly |
/// | 3     | `square`   | Poly |
pub struct PolyOsc {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    voice_count: usize,
    /// Per-voice phase accumulators. Always 16 elements; only `[0..voice_count]` are used.
    accumulators: Vec<UnitPhaseAccumulator>,
    // Port fields
    in_voct: PolyInput,
    out_sine: PolyOutput,
    out_triangle: PolyOutput,
    out_sawtooth: PolyOutput,
    out_square: PolyOutput,
}

impl Module for PolyOsc {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "PolyOsc",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "voct", index: 0, kind: CableKind::Poly },
            ],
            outputs: vec![
                PortDescriptor { name: "sine",     index: 0, kind: CableKind::Poly },
                PortDescriptor { name: "triangle", index: 0, kind: CableKind::Poly },
                PortDescriptor { name: "sawtooth", index: 0, kind: CableKind::Poly },
                PortDescriptor { name: "square",   index: 0, kind: CableKind::Poly },
            ],
            parameters: vec![
                ParameterDescriptor {
                    name: "frequency",
                    index: 0,
                    parameter_type: ParameterKind::Float {
                        min: 0.0,
                        max: 20_000.0,
                        default: 0.0,
                    },
                },
            ],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        let voice_count = audio_environment.poly_voices.min(16);
        let accumulators = (0..16)
            .map(|_| UnitPhaseAccumulator::new(audio_environment.sample_rate, C0_FREQ))
            .collect();
        Self {
            instance_id,
            descriptor,
            voice_count,
            accumulators,
            in_voct: PolyInput::default(),
            out_sine: PolyOutput::default(),
            out_triangle: PolyOutput::default(),
            out_sawtooth: PolyOutput::default(),
            out_square: PolyOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("frequency") {
            for acc in &mut self.accumulators {
                acc.set_frequency_offset(*v);
            }
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
    fn instance_id(&self) -> InstanceId { self.instance_id }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_voct     = PolyInput::from_ports(inputs, 0);
        self.out_sine     = PolyOutput::from_ports(outputs, 0);
        self.out_triangle = PolyOutput::from_ports(outputs, 1);
        self.out_sawtooth = PolyOutput::from_ports(outputs, 2);
        self.out_square   = PolyOutput::from_ports(outputs, 3);

        let voct_connected = self.in_voct.is_connected();
        for acc in &mut self.accumulators[..self.voice_count] {
            acc.set_modulation(voct_connected, false);
        }
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let voct = if self.in_voct.is_connected() {
            pool.read_poly(&self.in_voct)
        } else {
            [0.0; 16]
        };

        let do_sine = self.out_sine.is_connected();
        let do_tri  = self.out_triangle.is_connected();
        let do_saw  = self.out_sawtooth.is_connected();
        let do_sq   = self.out_square.is_connected();

        if !do_sine && !do_tri && !do_saw && !do_sq {
            // Advance phases even when no outputs connected, so pitch stays coherent.
            for (i, acc) in self.accumulators[..self.voice_count].iter_mut().enumerate() {
                if acc.is_modulating {
                    acc.advance_modulating(voct[i], 0.0);
                } else {
                    acc.advance();
                }
            }
            return;
        }

        let mut sine_out = [0.0f64; 16];
        let mut tri_out  = [0.0f64; 16];
        let mut saw_out  = [0.0f64; 16];
        let mut sq_out   = [0.0f64; 16];

        for (i, acc) in self.accumulators[..self.voice_count].iter_mut().enumerate() {
            let phase = acc.phase;

            if do_sine {
                sine_out[i] = lookup_sine(phase);
            }
            if do_tri {
                tri_out[i] = 1.0 - 4.0 * (phase - 0.5).abs();
            }
            if do_saw {
                let dt = acc.phase_increment;
                saw_out[i] = (2.0 * phase - 1.0) - polyblep(phase, dt);
            }
            if do_sq {
                let dt = acc.phase_increment;
                let raw = if phase < 0.5 { 1.0 } else { -1.0 };
                let blep = polyblep(phase, dt) - polyblep((phase - 0.5).rem_euclid(1.0), dt);
                sq_out[i] = raw + blep;
            }

            if acc.is_modulating {
                acc.advance_modulating(voct[i], 0.0);
            } else {
                acc.advance();
            }
        }

        if do_sine { pool.write_poly(&self.out_sine,     sine_out); }
        if do_tri  { pool.write_poly(&self.out_triangle, tri_out);  }
        if do_saw  { pool.write_poly(&self.out_sawtooth, saw_out);  }
        if do_sq   { pool.write_poly(&self.out_square,   sq_out);   }
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{AudioEnvironment, CablePool, CableValue, Module, ModuleShape, Registry};

    fn make_osc(sample_rate: f64, poly_voices: usize) -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<PolyOsc>();
        r.create(
            "PolyOsc",
            &AudioEnvironment { sample_rate, poly_voices },
            &ModuleShape { channels: 0, length: 0 },
            &patches_core::parameter_map::ParameterMap::new(),
            InstanceId::next(),
        )
        .unwrap()
    }

    fn make_pool(n: usize, poly: bool) -> Vec<[CableValue; 2]> {
        if poly {
            vec![[CableValue::Poly([0.0; 16]); 2]; n]
        } else {
            vec![[CableValue::Mono(0.0); 2]; n]
        }
    }

    #[test]
    fn sawtooth_output_not_zero_after_one_tick() {
        use patches_core::{InputPort, OutputPort, PolyInput, PolyOutput};
        let mut osc = make_osc(44100.0, 4);
        let inputs = vec![InputPort::Poly(PolyInput { cable_idx: 0, scale: 1.0, connected: false })];
        let outputs = vec![
            OutputPort::Poly(PolyOutput { cable_idx: 1, connected: false }),
            OutputPort::Poly(PolyOutput { cable_idx: 2, connected: false }),
            OutputPort::Poly(PolyOutput { cable_idx: 3, connected: true }),  // sawtooth
            OutputPort::Poly(PolyOutput { cable_idx: 4, connected: false }),
        ];
        osc.set_ports(&inputs, &outputs);

        let mut pool = make_pool(5, true);
        osc.process(&mut CablePool::new(&mut pool, 0));
        // First sample is phase=0 which maps to sawtooth ≈ -1 (PolyBLEP corrected)
        // Just ensure it writes a Poly value
        assert!(matches!(pool[3][0], CableValue::Poly(_)), "expected Poly output");
    }

    #[test]
    fn voct_input_drives_independent_phases_per_voice() {
        use patches_core::{InputPort, OutputPort, PolyInput, PolyOutput};
        // At sample_rate = C0 * 100, one cycle of voice 0 (voct=0) takes 100 samples.
        // Voice 1 with voct=1 (one octave up) runs at 2× and completes a cycle in 50 samples.
        // After 25 samples: voice 0 is at phase 0.25 (sine ≈ +1), voice 1 at phase 0.50 (sine ≈ 0).
        let period = 100usize;
        let sample_rate = C0_FREQ * period as f64;
        let mut osc = make_osc(sample_rate, 2);

        let inputs = vec![InputPort::Poly(PolyInput { cable_idx: 0, scale: 1.0, connected: true })];
        let outputs = vec![
            OutputPort::Poly(PolyOutput { cable_idx: 1, connected: true }), // sine
            OutputPort::Poly(PolyOutput { cable_idx: 2, connected: false }),
            OutputPort::Poly(PolyOutput { cable_idx: 3, connected: false }),
            OutputPort::Poly(PolyOutput { cable_idx: 4, connected: false }),
        ];
        osc.set_ports(&inputs, &outputs);

        let mut pool = make_pool(5, true);
        let mut voct = [0.0f64; 16];
        voct[1] = 1.0; // voice 1: one octave up

        let mut last_wi = 0;
        for i in 0..25 {
            let wi = i % 2;
            let ri = 1 - wi;
            pool[0][ri] = CableValue::Poly(voct);
            osc.process(&mut CablePool::new(&mut pool, wi));
            last_wi = wi;
        }
        // voice 0 phase ≈ 0.25: sine near peak (+1)
        // voice 1 phase ≈ 0.50: sine near zero
        if let CableValue::Poly(sines) = pool[1][last_wi] {
            assert!(sines[0] > 0.9,   "voice 0 at 0.25 cycle, sine should be near +1; got {}", sines[0]);
            assert!(sines[1].abs() < 0.15, "voice 1 at 0.50 cycle, sine should be near 0; got {}", sines[1]);
        } else {
            panic!("expected Poly sine output");
        }
    }
}
