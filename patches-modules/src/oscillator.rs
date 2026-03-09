use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor,
    ModuleShape, ParameterDescriptor, ParameterKind, PortDescriptor,
    PortConnectivity
};
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
    // Output connectivity
    out_sine: bool,
    out_triangle: bool,
    out_sawtooth: bool,
    out_square: bool,
    // Input connectivity
    in_pulse_width: bool,
    in_phase_mod: bool,
}

impl Module for Oscillator {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Oscillator",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "voct",        index: 0 },
                PortDescriptor { name: "fm",          index: 0 },
                PortDescriptor { name: "pulse_width", index: 0 },
                PortDescriptor { name: "phase_mod",   index: 0 },
            ],
            outputs: vec![
                PortDescriptor { name: "sine",     index: 0 },
                PortDescriptor { name: "triangle", index: 0 },
                PortDescriptor { name: "sawtooth", index: 0 },
                PortDescriptor { name: "square",   index: 0 },
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
            out_sine: false,
            out_triangle: false,
            out_sawtooth: false,
            out_square: false,
            in_pulse_width: false,
            in_phase_mod: false,
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

    fn set_connectivity(&mut self, connectivity: PortConnectivity) {
        self.phase_accumulator.set_modulation(
            connectivity.inputs[0], // voct
            connectivity.inputs[1], // fm
        );
        self.in_pulse_width = connectivity.inputs[2];
        self.in_phase_mod  = connectivity.inputs[3];
        self.out_sine     = connectivity.outputs[0];
        self.out_triangle = connectivity.outputs[1];
        self.out_sawtooth = connectivity.outputs[2];
        self.out_square   = connectivity.outputs[3];
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        let phase = self.phase_accumulator.phase;
        let read_phase = if self.in_phase_mod {
            (phase + inputs[3]).rem_euclid(1.0)
        } else {
            phase
        };

        if self.out_sine {
            outputs[0] = lookup_sine(read_phase);
        }
        if self.out_triangle {
            outputs[1] = 1.0 - 4.0 * (read_phase - 0.5).abs();
        }
        if self.out_sawtooth {
            let dt = self.phase_accumulator.phase_increment;
            outputs[2] = (2.0 * read_phase - 1.0) - polyblep(read_phase, dt);
        }
        if self.out_square {
            let dt = self.phase_accumulator.phase_increment;
            let duty = if self.in_pulse_width {
                (0.5 + 0.5 * inputs[2]).clamp(0.01, 0.99)
            } else {
                0.5
            };
            let raw = if read_phase < duty { 1.0 } else { -1.0 };
            let blep = polyblep(read_phase, dt)
                - polyblep((read_phase - duty).rem_euclid(1.0), dt);
            outputs[3] = raw + blep;
        }

        if self.phase_accumulator.is_modulating {
            self.phase_accumulator.advance_modulating(inputs[0], inputs[1]);
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
    use patches_core::{AudioEnvironment, Module, ModuleShape, PortConnectivity, Registry};
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
            "Oscillator",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap()
    }

    fn all_outputs_connected() -> PortConnectivity {
        PortConnectivity {
            inputs: vec![false, false, false, false].into_boxed_slice(),
            outputs: vec![true, true, true, true].into_boxed_slice(),
        }
    }

    #[test]
    fn descriptor_has_four_inputs_and_four_outputs() {
        let osc = make_osc(440.0);
        let desc = osc.descriptor();
        assert_eq!(desc.inputs.len(), 4);
        assert_eq!(desc.inputs[0].name, "voct");
        assert_eq!(desc.inputs[1].name, "fm");
        assert_eq!(desc.inputs[2].name, "pulse_width");
        assert_eq!(desc.inputs[3].name, "phase_mod");
        assert_eq!(desc.outputs.len(), 4);
        assert_eq!(desc.outputs[0].name, "sine");
        assert_eq!(desc.outputs[1].name, "triangle");
        assert_eq!(desc.outputs[2].name, "sawtooth");
        assert_eq!(desc.outputs[3].name, "square");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_osc(440.0);
        let b = make_osc(440.0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn sine_output_completes_full_cycle_in_period_samples() {
        // base = C0_FREQ + frequency. Choose sample_rate so period is exact.
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        osc.set_connectivity(all_outputs_connected());
        let mut outputs = [0.0_f64; 4];

        let mut first_cycle = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&[0.0, 0.0, 0.0], &mut outputs);
            first_cycle.push(outputs[0]);
        }

        let mut second_cycle = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&[0.0, 0.0, 0.0], &mut outputs);
            second_cycle.push(outputs[0]);
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
        osc.set_connectivity(all_outputs_connected());
        let mut outputs = [0.0_f64; 4];

        let mut first_cycle = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&[0.0, 0.0, 0.0], &mut outputs);
            first_cycle.push(outputs[1]);
        }

        let mut second_cycle = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&[0.0, 0.0, 0.0], &mut outputs);
            second_cycle.push(outputs[1]);
        }

        for (a, b) in first_cycle.iter().zip(second_cycle.iter()) {
            assert!((a - b).abs() < 1e-10, "triangle cycle mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn sawtooth_polyblep_smooths_transition() {
        // At phase=0 the raw sawtooth would output -1.0, but PolyBLEP should give a value
        // strictly above -1.0.
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        osc.set_connectivity(all_outputs_connected());
        let mut outputs = [0.0_f64; 4];

        // First sample: phase = 0 (transition wrap point).
        osc.process(&[0.0, 0.0, 0.0], &mut outputs);
        assert!(
            outputs[2] > -1.0,
            "sawtooth at wrap transition must not output exact -1.0; got {}", outputs[2]
        );
    }

    #[test]
    fn sawtooth_non_transition_samples_match_formula() {
        // Non-transition samples (well away from the wrap) must match 2*phase - 1 exactly.
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        osc.set_connectivity(all_outputs_connected());
        let mut outputs = [0.0_f64; 4];

        osc.process(&[0.0, 0.0, 0.0], &mut outputs); // i=0 is the transition; skip
        for i in 1..period {
            osc.process(&[0.0, 0.0, 0.0], &mut outputs);
            let phase = i as f64 / period as f64;
            let expected = 2.0 * phase - 1.0;
            assert!(
                (outputs[2] - expected).abs() < 1e-10,
                "sawtooth mismatch at sample {i}: got {}, expected {expected}", outputs[2]
            );
        }
    }

    #[test]
    fn square_polyblep_at_transition_not_exactly_plus_minus_one() {
        // At phase=0 (rising edge) and phase≈duty (falling edge), PolyBLEP correction
        // must produce a value strictly between -1 and +1.
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        osc.set_connectivity(PortConnectivity {
            inputs: vec![false, false, false, false].into_boxed_slice(),
            outputs: vec![false, false, false, true].into_boxed_slice(),
        });
        let mut outputs = [0.0_f64; 4];

        // First sample: phase = 0 (rising edge). Raw would be +1; PolyBLEP gives ~0.
        osc.process(&[0.0, 0.0, 0.0], &mut outputs);
        assert!(
            outputs[3] > -1.0 && outputs[3] < 1.0,
            "square at rising edge must not be exactly ±1; got {}", outputs[3]
        );

        // Advance to the falling edge (phase = 0.50 = duty).
        // After the first call above, phase = 0.01. Run 49 more to bring phase to 0.50,
        // then sample once at that phase.
        for _ in 0..49 {
            osc.process(&[0.0, 0.0, 0.0], &mut outputs);
        }
        // Phase is now 0.50; sample it (falling edge transition).
        osc.process(&[0.0, 0.0, 0.0], &mut outputs);
        assert!(
            outputs[3] > -1.0 && outputs[3] < 1.0,
            "square at falling edge must not be exactly ±1; got {}", outputs[3]
        );
    }

    #[test]
    fn square_duty_cycle_responds_to_pulse_width_input() {
        // pulse_width input = 1.0 → duty = 0.5 + 0.5*1.0 = 1.0, clamped to 0.99
        // → ~99% of samples should be +1.0
        let frequency = 1.0_f64;
        let period = 100_usize;
        let sample_rate = (C0_FREQ + frequency) * period as f64;

        let mut osc = make_osc_sr(frequency, sample_rate);
        osc.set_connectivity(PortConnectivity {
            inputs: vec![false, false, true, false].into_boxed_slice(),
            outputs: vec![false, false, false, true].into_boxed_slice(),
        });
        let mut outputs = [0.0_f64; 4];

        let mut positive_count = 0usize;
        for _ in 0..period {
            osc.process(&[0.0, 0.0, 1.0], &mut outputs);
            if outputs[3] > 0.0 { positive_count += 1; }
        }
        assert!(
            positive_count >= 95,
            "expected ~99 positive samples with pw=1.0, got {positive_count}"
        );
    }

    #[test]
    fn disconnected_outputs_are_not_written() {
        let mut osc = make_osc(440.0);
        osc.set_connectivity(PortConnectivity {
            inputs: vec![false, false, false, false].into_boxed_slice(),
            outputs: vec![false, false, false, false].into_boxed_slice(),
        });
        let mut outputs = [99.0_f64; 4]; // sentinel values
        osc.process(&[0.0, 0.0, 0.0], &mut outputs);
        for (i, &v) in outputs.iter().enumerate() {
            assert_eq!(v, 99.0, "output[{i}] was written despite being disconnected");
        }
    }

    #[test]
    fn phase_mod_half_cycle_shifts_sine_output() {
        // With phase_mod = 0.5 and accumulator phase = 0.0, read_phase = 0.5.
        // lookup_sine(0.5) ≈ 0.0 (sine at half cycle).
        let mut osc = make_osc(440.0);
        osc.set_connectivity(PortConnectivity {
            inputs: vec![false, false, false, true].into_boxed_slice(),
            outputs: vec![true, false, false, false].into_boxed_slice(),
        });
        let mut outputs = [0.0_f64; 4];
        // Phase starts at 0. With phase_mod = 0.5, read_phase = (0 + 0.5).rem_euclid(1) = 0.5.
        osc.process(&[0.0, 0.0, 0.0, 0.5], &mut outputs);
        let expected = crate::common::approximate::lookup_sine(0.5);
        assert!(
            (outputs[0] - expected).abs() < 1e-6,
            "phase_mod=0.5 must shift sine to lookup_sine(0.5); got {}, expected {expected}",
            outputs[0]
        );
    }

    #[test]
    fn phase_mod_disconnected_restores_normal_sine() {
        // Without phase_mod, sine at phase=0 should equal lookup_sine(0).
        let mut osc = make_osc(440.0);
        osc.set_connectivity(PortConnectivity {
            inputs: vec![false, false, false, false].into_boxed_slice(),
            outputs: vec![true, false, false, false].into_boxed_slice(),
        });
        let mut outputs = [0.0_f64; 4];
        osc.process(&[0.0, 0.0, 0.0, 0.5], &mut outputs); // inputs[3] ignored
        let expected = crate::common::approximate::lookup_sine(0.0);
        assert!(
            (outputs[0] - expected).abs() < 1e-6,
            "with phase_mod disconnected sine must equal lookup_sine(0.0); got {}",
            outputs[0]
        );
    }
}
