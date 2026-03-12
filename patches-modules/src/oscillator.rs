use patches_core::{
    AudioEnvironment, CableValue, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, ModuleShape, OutputPort, ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::{ParameterMap, ParameterValue};
use crate::common::approximate::lookup_sine;
use crate::common::frequency::{C0_FREQ, UnitPhaseAccumulator, FMMode};

/// PolyBLEP correction for a normalised phase `t ∈ [0, 1)` and phase increment `dt`.
///
/// Returns a correction value that smooths the discontinuity near `t = 0` (rising)
/// and `t = 1` (falling) transitions. Only effective when `dt < 0.5`.
fn polyblep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let t = t / dt;
        2.0 * t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

/// A multi-waveform oscillator driven by a single phase accumulator.
///
/// Outputs sine, triangle, sawtooth, and square waveforms simultaneously.
/// All share the same phase; only connected outputs are computed each sample.
/// Frequency is V/OCT rooted at C0 (≈ 16.35 Hz); the `frequency` parameter
/// is a Hz offset added to the C0 reference before V/OCT modulation.
pub struct Oscillator {
    instance_id: InstanceId,
    phase_accumulator: UnitPhaseAccumulator,
    descriptor: ModuleDescriptor,
    // Input port fields
    in_voct: MonoInput,
    in_fm: MonoInput,
    in_pulse_width: MonoInput,
    in_phase_mod: MonoInput,
    // Output port fields
    out_sine: MonoOutput,
    out_triangle: MonoOutput,
    out_sawtooth: MonoOutput,
    out_square: MonoOutput,
}

impl Module for Oscillator {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Osc",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "voct",        index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "fm",          index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "pulse_width", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "phase_mod",   index: 0, kind: CableKind::Mono },
            ],
            outputs: vec![
                PortDescriptor { name: "sine",     index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "triangle", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "sawtooth", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "square",   index: 0, kind: CableKind::Mono },
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
                ParameterDescriptor {
                    name: "fm_type",
                    index: 0,
                    parameter_type: ParameterKind::Enum {
                        variants: &["linear", "logarithmic"],
                        default: "linear",
                    },
                },
            ],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            phase_accumulator: UnitPhaseAccumulator::new(audio_environment.sample_rate, C0_FREQ),
            descriptor,
            in_voct: MonoInput::default(),
            in_fm: MonoInput::default(),
            in_pulse_width: MonoInput::default(),
            in_phase_mod: MonoInput::default(),
            out_sine: MonoOutput::default(),
            out_triangle: MonoOutput::default(),
            out_sawtooth: MonoOutput::default(),
            out_square: MonoOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("frequency") {
            self.phase_accumulator.set_frequency_offset(*v);
        }
        if let Some(ParameterValue::Enum(v)) = params.get("fm_type") {
            let fm_mode = match *v {
                "linear" => FMMode::Linear,
                "logarithmic" => FMMode::Exponential,
                _ => return,
            };
            self.phase_accumulator.set_fm_mode(fm_mode);
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_voct = MonoInput::from_ports(inputs, 0);
        self.in_fm = MonoInput::from_ports(inputs, 1);
        self.in_pulse_width = MonoInput::from_ports(inputs, 2);
        self.in_phase_mod = MonoInput::from_ports(inputs, 3);
        self.out_sine = MonoOutput::from_ports(outputs, 0);
        self.out_triangle = MonoOutput::from_ports(outputs, 1);
        self.out_sawtooth = MonoOutput::from_ports(outputs, 2);
        self.out_square = MonoOutput::from_ports(outputs, 3);
    }

    fn process(&mut self, pool: &mut [[CableValue; 2]], wi: usize) {
        let ri = 1 - wi;
        let phase = self.phase_accumulator.phase;
        let read_phase = if self.in_phase_mod.is_connected() {
            (phase + self.in_phase_mod.read_from(pool, ri)).rem_euclid(1.0)
        } else {
            phase
        };

        if self.out_sine.is_connected() {
            self.out_sine.write_to(pool, wi, lookup_sine(read_phase));
        }
        if self.out_triangle.is_connected() {
            self.out_triangle.write_to(pool, wi, 1.0 - 4.0 * (read_phase - 0.5).abs());
        }
        if self.out_sawtooth.is_connected() {
            let dt = self.phase_accumulator.phase_increment;
            self.out_sawtooth.write_to(pool, wi, (2.0 * read_phase - 1.0) - polyblep(read_phase, dt));
        }
        if self.out_square.is_connected() {
            let dt = self.phase_accumulator.phase_increment;
            let duty = if self.in_pulse_width.is_connected() {
                (0.5 + 0.5 * self.in_pulse_width.read_from(pool, ri)).clamp(0.01, 0.99)
            } else {
                0.5
            };
            let raw = if read_phase < duty { 1.0 } else { -1.0 };
            let blep = polyblep(read_phase, dt)
                - polyblep((read_phase - duty).rem_euclid(1.0), dt);
            self.out_square.write_to(pool, wi, raw + blep);
        }

        if self.phase_accumulator.is_modulating {
            let voct = self.in_voct.read_from(pool, ri);
            let fm = self.in_fm.read_from(pool, ri);
            self.phase_accumulator.advance_modulating(voct, fm);
        } else {
            self.phase_accumulator.advance();
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::common::frequency::C0_FREQ;
    use patches_core::{AudioEnvironment, Module, ModuleShape, Registry};
    use patches_core::parameter_map::{ParameterMap, ParameterValue};

    fn make_osc(frequency: f64) -> Box<dyn Module> {
        make_osc_sr(frequency, 44100.0)
    }

    fn make_osc_sr(frequency: f64, sample_rate: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("frequency".into(), ParameterValue::Float(frequency));
        let mut r = Registry::new();
        r.register::<Oscillator>();
        r.create(
            "Osc",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

/// Set up ports with only voct connected as input, all outputs connected.
    fn set_ports_outputs_only(module: &mut Box<dyn Module>) {
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 2, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 3, scale: 1.0, connected: false }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 5, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 6, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 7, connected: true }),
        ];
        module.set_ports(&inputs, &outputs);
    }

    /// Set up ports with no outputs connected.
    fn set_ports_none_connected(module: &mut Box<dyn Module>) {
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 2, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 3, scale: 1.0, connected: false }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 5, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 6, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 7, connected: false }),
        ];
        module.set_ports(&inputs, &outputs);
    }

    #[test]
    fn sine_output_completes_full_cycle_in_period_samples() {
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        set_ports_outputs_only(&mut osc);
        let mut pool = make_pool(8);

        let mut first_cycle = Vec::with_capacity(period);
        for i in 0..period {
            osc.process(&mut pool, i % 2);
            let wi = i % 2;
            if let CableValue::Mono(v) = pool[4][wi] { first_cycle.push(v); }
        }

        let mut second_cycle = Vec::with_capacity(period);
        for i in 0..period {
            osc.process(&mut pool, (period + i) % 2);
            let wi = (period + i) % 2;
            if let CableValue::Mono(v) = pool[4][wi] { second_cycle.push(v); }
        }

        for (a, b) in first_cycle.iter().zip(second_cycle.iter()) {
            assert!((a - b).abs() < 1e-10, "sine cycle mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn triangle_output_completes_full_cycle() {
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        set_ports_outputs_only(&mut osc);
        let mut pool = make_pool(8);

        let mut first_cycle = Vec::with_capacity(period);
        for i in 0..period {
            osc.process(&mut pool, i % 2);
            let wi = i % 2;
            if let CableValue::Mono(v) = pool[5][wi] { first_cycle.push(v); }
        }

        let mut second_cycle = Vec::with_capacity(period);
        for i in 0..period {
            osc.process(&mut pool, (period + i) % 2);
            let wi = (period + i) % 2;
            if let CableValue::Mono(v) = pool[5][wi] { second_cycle.push(v); }
        }

        for (a, b) in first_cycle.iter().zip(second_cycle.iter()) {
            assert!((a - b).abs() < 1e-10, "triangle cycle mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn sawtooth_polyblep_smooths_transition() {
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        set_ports_outputs_only(&mut osc);
        let mut pool = make_pool(8);

        osc.process(&mut pool, 0);
        if let CableValue::Mono(v) = pool[6][0] {
            assert!(
                v > -1.0,
                "sawtooth at wrap transition must not output exact -1.0; got {}", v
            );
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn sawtooth_non_transition_samples_match_formula() {
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        set_ports_outputs_only(&mut osc);
        let mut pool = make_pool(8);

        osc.process(&mut pool, 0); // i=0 is the transition; skip
        for i in 1..period {
            osc.process(&mut pool, i % 2);
            let wi = i % 2;
            if let CableValue::Mono(v) = pool[6][wi] {
                let phase = i as f64 / period as f64;
                let expected = 2.0 * phase - 1.0;
                assert!(
                    (v - expected).abs() < 1e-10,
                    "sawtooth mismatch at sample {i}: got {}, expected {expected}", v
                );
            } else { panic!("expected Mono"); }
        }
    }

    #[test]
    fn square_polyblep_at_transition_not_exactly_plus_minus_one() {
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        set_ports_outputs_only(&mut osc);
        let mut pool = make_pool(8);

        osc.process(&mut pool, 0);
        if let CableValue::Mono(v) = pool[7][0] {
            assert!(
                v > -1.0 && v < 1.0,
                "square at rising edge must not be exactly ±1; got {}", v
            );
        } else { panic!("expected Mono"); }

        for i in 0..49 {
            osc.process(&mut pool, (1 + i) % 2);
        }
        osc.process(&mut pool, 50 % 2);
        if let CableValue::Mono(v) = pool[7][50 % 2] {
            assert!(
                v > -1.0 && v < 1.0,
                "square at falling edge must not be exactly ±1; got {}", v
            );
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn square_duty_cycle_responds_to_pulse_width_input() {
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        // Connect pulse_width input and square output
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 2, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 3, scale: 1.0, connected: false }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 5, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 6, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 7, connected: true }),
        ];
        osc.set_ports(&inputs, &outputs);

        let mut pool = make_pool(8);
        // pulse_width input = 1.0 → duty = 0.5 + 0.5*1.0 = 1.0, clamped to 0.99
        pool[2][1] = CableValue::Mono(1.0);

        let mut positive_count = 0usize;
        for i in 0..period {
            pool[2][1 - (i % 2)] = CableValue::Mono(1.0);
            osc.process(&mut pool, i % 2);
            if let CableValue::Mono(v) = pool[7][i % 2] {
                if v > 0.0 { positive_count += 1; }
            }
        }
        assert!(
            positive_count >= 95,
            "expected ~99 positive samples with pw=1.0, got {positive_count}"
        );
    }

    #[test]
    fn disconnected_outputs_are_not_written() {
        let mut osc = make_osc(440.0);
        set_ports_none_connected(&mut osc);
        // Pool slots 4..8 start at 99.0 sentinel
        let mut pool: Vec<[CableValue; 2]> = (0..8).map(|_| [CableValue::Mono(99.0); 2]).collect();
        osc.process(&mut pool, 0);
        for i in 4..8 {
            if let CableValue::Mono(v) = pool[i][0] {
                assert_eq!(v, 99.0, "output cable {i} was written despite being disconnected");
            }
        }
    }

    #[test]
    fn phase_mod_half_cycle_shifts_sine_output() {
        let mut osc = make_osc(440.0);
        // Connect phase_mod input and sine output
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 2, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 3, scale: 1.0, connected: true }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 5, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 6, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 7, connected: false }),
        ];
        osc.set_ports(&inputs, &outputs);

        let mut pool = make_pool(8);
        // phase_mod input = 0.5 in read slot (ri=1 when wi=0)
        pool[3][1] = CableValue::Mono(0.5);
        osc.process(&mut pool, 0);
        let expected = crate::common::approximate::lookup_sine(0.5);
        if let CableValue::Mono(v) = pool[4][0] {
            assert!(
                (v - expected).abs() < 1e-6,
                "phase_mod=0.5 must shift sine to lookup_sine(0.5); got {}, expected {expected}",
                v
            );
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn phase_mod_disconnected_restores_normal_sine() {
        let mut osc = make_osc(440.0);
        // phase_mod disconnected, sine connected
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 2, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 3, scale: 1.0, connected: false }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 5, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 6, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 7, connected: false }),
        ];
        osc.set_ports(&inputs, &outputs);

        let mut pool = make_pool(8);
        osc.process(&mut pool, 0);
        let expected = crate::common::approximate::lookup_sine(0.0);
        if let CableValue::Mono(v) = pool[4][0] {
            assert!(
                (v - expected).abs() < 1e-6,
                "with phase_mod disconnected sine must equal lookup_sine(0.0); got {}",
                v
            );
        } else { panic!("expected Mono"); }
    }
}
