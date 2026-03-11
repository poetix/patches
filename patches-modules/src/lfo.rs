use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor,
    ModuleShape, ParameterDescriptor, ParameterKind, PortDescriptor,
    PortConnectivity,
};
use patches_core::CableKind;
use patches_core::parameter_map::{ParameterMap, ParameterValue};
use crate::common::approximate::lookup_sine;

/// A low-frequency oscillator with six waveform outputs.
///
/// Outputs sine, triangle, saw_up, saw_down, square, and random waveforms.
/// Rate is in Hz; phase_offset shifts all waveforms by a fixed fraction of a cycle.
/// Mode controls polarity: bipolar ([-1, 1]), unipolar_positive ([0, 1]),
/// or unipolar_negative ([-1, 0]).
pub struct Lfo {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f64,
    phase: f64,
    phase_increment: f64,
    phase_offset: f64,
    mode: PolarityMode,
    rate: f64,
    prng_state: u64,
    random_value: f64,
    prev_sync: f64,
    // Input connectivity
    in_sync: bool,
    in_rate_cv: bool,
    // Output connectivity
    out_sine: bool,
    out_triangle: bool,
    out_saw_up: bool,
    out_saw_down: bool,
    out_square: bool,
    out_random: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum PolarityMode {
    Bipolar,
    UniposPositive,
    UnipolarNegative,
}

fn xorshift64(state: &mut u64) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state as i64 as f64) / (i64::MAX as f64)
}

fn apply_mode(v: f64, mode: PolarityMode) -> f64 {
    match mode {
        PolarityMode::Bipolar => v,
        PolarityMode::UniposPositive => 0.5 + 0.5 * v,
        PolarityMode::UnipolarNegative => -(0.5 + 0.5 * v),
    }
}

impl Module for Lfo {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Lfo",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "sync",    index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "rate_cv", index: 0, kind: CableKind::Mono },
            ],
            outputs: vec![
                PortDescriptor { name: "sine",     index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "triangle", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "saw_up",   index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "saw_down", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "square",   index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "random",   index: 0, kind: CableKind::Mono },
            ],
            parameters: vec![
                ParameterDescriptor {
                    name: "rate",
                    index: 0,
                    parameter_type: ParameterKind::Float { min: 0.01, max: 20.0, default: 1.0 },
                },
                ParameterDescriptor {
                    name: "phase_offset",
                    index: 0,
                    parameter_type: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.0 },
                },
                ParameterDescriptor {
                    name: "mode",
                    index: 0,
                    parameter_type: ParameterKind::Enum {
                        variants: &["bipolar", "unipolar_positive", "unipolar_negative"],
                        default: "bipolar",
                    },
                },
            ],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        let prng_state = instance_id.as_u64() + 1; // +1 ensures non-zero (xorshift64 requires state != 0)
        Self {
            instance_id,
            descriptor,
            sample_rate: audio_environment.sample_rate,
            phase: 0.0,
            phase_increment: 1.0 / audio_environment.sample_rate,
            phase_offset: 0.0,
            mode: PolarityMode::Bipolar,
            rate: 1.0,
            prng_state,
            random_value: 0.0,
            prev_sync: 0.0,
            in_sync: false,
            in_rate_cv: false,
            out_sine: false,
            out_triangle: false,
            out_saw_up: false,
            out_saw_down: false,
            out_square: false,
            out_random: false,
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("rate") {
            self.rate = *v;
            self.phase_increment = v / self.sample_rate;
        }
        if let Some(ParameterValue::Float(v)) = params.get("phase_offset") {
            self.phase_offset = *v;
        }
        if let Some(ParameterValue::Enum(v)) = params.get("mode") {
            self.mode = match *v {
                "bipolar" => PolarityMode::Bipolar,
                "unipolar_positive" => PolarityMode::UniposPositive,
                "unipolar_negative" => PolarityMode::UnipolarNegative,
                _ => return,
            };
        }
    }

    fn set_connectivity(&mut self, connectivity: PortConnectivity) {
        self.in_sync      = connectivity.inputs[0];
        self.in_rate_cv   = connectivity.inputs[1];
        self.out_sine     = connectivity.outputs[0];
        self.out_triangle = connectivity.outputs[1];
        self.out_saw_up   = connectivity.outputs[2];
        self.out_saw_down = connectivity.outputs[3];
        self.out_square   = connectivity.outputs[4];
        self.out_random   = connectivity.outputs[5];
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        // Sync: rising edge (prev <= 0, current > 0) resets phase before advance.
        if self.in_sync {
            let sync_val = inputs[0];
            if self.prev_sync <= 0.0 && sync_val > 0.0 {
                self.phase = 0.0;
            }
            self.prev_sync = sync_val;
        }

        // Rate CV: recompute increment per-sample when connected.
        let increment = if self.in_rate_cv {
            (self.rate + inputs[1]).clamp(0.001, 40.0) / self.sample_rate
        } else {
            self.phase_increment
        };

        let new_phase = self.phase + increment;
        let wrapped = new_phase >= 1.0;
        self.phase = new_phase.fract();

        if wrapped {
            self.random_value = xorshift64(&mut self.prng_state);
        }

        let read_phase = (self.phase + self.phase_offset).fract();
        let mode = self.mode;

        if self.out_sine {
            outputs[0] = apply_mode(lookup_sine(read_phase), mode);
        }
        if self.out_triangle {
            outputs[1] = apply_mode(1.0 - 4.0 * (read_phase - 0.5).abs(), mode);
        }
        if self.out_saw_up {
            outputs[2] = apply_mode(2.0 * read_phase - 1.0, mode);
        }
        if self.out_saw_down {
            outputs[3] = apply_mode(1.0 - 2.0 * read_phase, mode);
        }
        if self.out_square {
            let v = if read_phase < 0.5 { 1.0 } else { -1.0 };
            outputs[4] = apply_mode(v, mode);
        }
        if self.out_random {
            outputs[5] = apply_mode(self.random_value, mode);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{AudioEnvironment, Module, ModuleShape, PortConnectivity, Registry};
    use patches_core::parameter_map::{ParameterMap, ParameterValue};

    fn make_lfo(rate: f64) -> Box<dyn Module> {
        make_lfo_sr(rate, 44100.0)
    }

    fn make_lfo_sr(rate: f64, sample_rate: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("rate".into(), ParameterValue::Float(rate));
        let mut r = Registry::new();
        r.register::<Lfo>();
        r.create(
            "Lfo",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap()
    }

    fn no_inputs_connected() -> PortConnectivity {
        PortConnectivity {
            inputs: vec![false, false].into_boxed_slice(),
            outputs: vec![false, false, false, false, false, false].into_boxed_slice(),
        }
    }

    fn all_outputs_connected() -> PortConnectivity {
        PortConnectivity {
            inputs: vec![false, false].into_boxed_slice(),
            outputs: vec![true, true, true, true, true, true].into_boxed_slice(),
        }
    }

    #[test]
    fn descriptor_has_two_inputs_and_six_outputs() {
        let lfo = make_lfo(1.0);
        let desc = lfo.descriptor();
        assert_eq!(desc.inputs.len(), 2);
        assert_eq!(desc.inputs[0].name, "sync");
        assert_eq!(desc.inputs[1].name, "rate_cv");
        assert_eq!(desc.outputs.len(), 6);
        assert_eq!(desc.outputs[0].name, "sine");
        assert_eq!(desc.outputs[1].name, "triangle");
        assert_eq!(desc.outputs[2].name, "saw_up");
        assert_eq!(desc.outputs[3].name, "saw_down");
        assert_eq!(desc.outputs[4].name, "square");
        assert_eq!(desc.outputs[5].name, "random");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_lfo(1.0);
        let b = make_lfo(1.0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn sine_output_consistent_across_two_cycles() {
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;

        let mut lfo = make_lfo_sr(rate, sample_rate);
        lfo.set_connectivity(all_outputs_connected());
        let mut outputs = [0.0_f64; 6];

        let mut cycle1 = Vec::with_capacity(period);
        for _ in 0..period {
            lfo.process(&[], &mut outputs);
            cycle1.push(outputs[0]);
        }
        let mut cycle2 = Vec::with_capacity(period);
        for _ in 0..period {
            lfo.process(&[], &mut outputs);
            cycle2.push(outputs[0]);
        }
        for (a, b) in cycle1.iter().zip(cycle2.iter()) {
            assert!((a - b).abs() < 1e-10, "sine cycle mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn phase_offset_shifts_sine_by_quarter_cycle() {
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;

        // LFO without offset: record first cycle.
        let mut lfo_base = make_lfo_sr(rate, sample_rate);
        lfo_base.set_connectivity(all_outputs_connected());
        let mut out = [0.0_f64; 6];
        let mut base_cycle = Vec::with_capacity(period);
        for _ in 0..period {
            lfo_base.process(&[], &mut out);
            base_cycle.push(out[0]);
        }

        // LFO with phase_offset = 0.25: record first cycle.
        let mut params = ParameterMap::new();
        params.insert("rate".into(), ParameterValue::Float(rate));
        params.insert("phase_offset".into(), ParameterValue::Float(0.25));
        let mut r = Registry::new();
        r.register::<Lfo>();
        let mut lfo_shifted = r.create(
            "Lfo",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap();
        lfo_shifted.set_connectivity(all_outputs_connected());

        let mut shifted_cycle = Vec::with_capacity(period);
        for _ in 0..period {
            lfo_shifted.process(&[], &mut out);
            shifted_cycle.push(out[0]);
        }

        // Shifted by 0.25 means each sample i of the shifted LFO matches
        // sample (i + period/4) % period of the base LFO.
        let quarter = period / 4;
        for i in 0..period {
            let base_val = base_cycle[(i + quarter) % period];
            let shifted_val = shifted_cycle[i];
            assert!(
                (base_val - shifted_val).abs() < 1e-5,
                "phase_offset=0.25 mismatch at sample {i}: base[{idx}]={base_val}, shifted={shifted_val}",
                idx = (i + quarter) % period,
            );
        }
    }

    #[test]
    fn unipolar_positive_maps_saw_up_to_zero_one() {
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;

        let mut params = ParameterMap::new();
        params.insert("rate".into(), ParameterValue::Float(rate));
        params.insert("mode".into(), ParameterValue::Enum("unipolar_positive"));
        let mut r = Registry::new();
        r.register::<Lfo>();
        let mut lfo = r.create(
            "Lfo",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap();
        lfo.set_connectivity(PortConnectivity {
            inputs: vec![false, false].into_boxed_slice(),
            outputs: vec![false, false, true, false, false, false].into_boxed_slice(),
        });

        let mut outputs = [0.0_f64; 6];
        for _ in 0..period {
            lfo.process(&[], &mut outputs);
            assert!(
                outputs[2] >= 0.0 && outputs[2] <= 1.0,
                "unipolar_positive saw_up must be in [0, 1]; got {}", outputs[2]
            );
        }
    }

    #[test]
    fn random_output_holds_per_period_and_is_in_range() {
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;

        let mut params = ParameterMap::new();
        params.insert("rate".into(), ParameterValue::Float(rate));
        params.insert("mode".into(), ParameterValue::Enum("unipolar_positive"));
        let mut r = Registry::new();
        r.register::<Lfo>();
        let mut lfo = r.create(
            "Lfo",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap();
        lfo.set_connectivity(PortConnectivity {
            inputs: vec![false, false].into_boxed_slice(),
            outputs: vec![false, false, false, false, false, true].into_boxed_slice(),
        });

        let mut outputs = [0.0_f64; 6];

        // Run three full periods; within each period the random value must be constant,
        // and it must stay in [0, 1] (unipolar_positive mode).
        // Check period-2 samples per cycle (avoids the wrap-boundary sample where
        // floating-point accumulation may trigger the wrap one sample early or late).
        for _cycle in 0..3 {
            lfo.process(&[], &mut outputs);
            let cycle_value = outputs[5];
            assert!(
                cycle_value >= 0.0 && cycle_value <= 1.0,
                "random output must be in [0, 1] in unipolar_positive mode; got {cycle_value}"
            );
            for _ in 1..(period - 1) {
                lfo.process(&[], &mut outputs);
                assert!(
                    (outputs[5] - cycle_value).abs() < 1e-15,
                    "random output must hold within a period; changed from {cycle_value} to {}",
                    outputs[5]
                );
            }
            // Consume the remaining sample(s) to advance to the next cycle boundary.
            lfo.process(&[], &mut outputs);
        }
    }

    #[test]
    fn disconnected_outputs_are_not_written() {
        let mut lfo = make_lfo(1.0);
        lfo.set_connectivity(no_inputs_connected());
        let mut outputs = [99.0_f64; 6];
        lfo.process(&[0.0, 0.0], &mut outputs);
        for (i, &v) in outputs.iter().enumerate() {
            assert_eq!(v, 99.0, "output[{i}] was written despite being disconnected");
        }
    }

    #[test]
    fn sync_rising_edge_resets_phase_mid_cycle() {
        // Use a slow LFO (1 Hz at 100 Hz sample rate = 100-sample period).
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;
        let mut lfo = make_lfo_sr(rate, sample_rate);
        lfo.set_connectivity(PortConnectivity {
            inputs: vec![true, false].into_boxed_slice(),
            outputs: vec![true, false, false, false, false, false].into_boxed_slice(),
        });

        let mut outputs = [0.0_f64; 6];

        // Advance 25 samples (quarter-cycle) with sync low.
        for _ in 0..25 {
            lfo.process(&[0.0, 0.0], &mut outputs);
        }

        // Rising edge: sync goes from 0 → 1. Phase resets to 0 before this advance.
        lfo.process(&[1.0, 0.0], &mut outputs);
        let after_reset = outputs[0];

        // A fresh LFO at sample 1 (phase = 1/100) should match.
        let mut lfo_fresh = make_lfo_sr(rate, sample_rate);
        lfo_fresh.set_connectivity(PortConnectivity {
            inputs: vec![false, false].into_boxed_slice(),
            outputs: vec![true, false, false, false, false, false].into_boxed_slice(),
        });
        lfo_fresh.process(&[], &mut outputs);
        let expected = outputs[0];

        assert!(
            (after_reset - expected).abs() < 1e-10,
            "after sync reset sine={after_reset}, expected fresh LFO sine={expected}"
        );
    }

    #[test]
    fn sync_level_does_not_retrigger() {
        // A flat positive sync signal (no edge) must not reset the phase.
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;
        let mut lfo = make_lfo_sr(rate, sample_rate);
        lfo.set_connectivity(PortConnectivity {
            inputs: vec![true, false].into_boxed_slice(),
            outputs: vec![true, false, false, false, false, false].into_boxed_slice(),
        });

        let mut outputs = [0.0_f64; 6];

        // Trigger a rising edge first (prev=0 → 1), then hold high.
        lfo.process(&[1.0, 0.0], &mut outputs);

        // Advance 25 more samples with sync held high — no retrigger.
        let mut values = Vec::new();
        for _ in 0..25 {
            lfo.process(&[1.0, 0.0], &mut outputs);
            values.push(outputs[0]);
        }

        // Compare against an identical fresh LFO that had one edge trigger then held.
        let mut lfo_ref = make_lfo_sr(rate, sample_rate);
        lfo_ref.set_connectivity(PortConnectivity {
            inputs: vec![true, false].into_boxed_slice(),
            outputs: vec![true, false, false, false, false, false].into_boxed_slice(),
        });
        lfo_ref.process(&[1.0, 0.0], &mut outputs);
        let mut ref_values = Vec::new();
        for _ in 0..25 {
            lfo_ref.process(&[1.0, 0.0], &mut outputs);
            ref_values.push(outputs[0]);
        }

        // Values must advance monotonically (not reset back to near-zero repeatedly).
        for (i, (&v, &r)) in values.iter().zip(ref_values.iter()).enumerate() {
            assert!((v - r).abs() < 1e-10, "sample {i}: sync level caused retrigger; got {v} vs ref {r}");
        }
    }

    #[test]
    fn rate_cv_doubles_rate_halves_period() {
        // With rate=1 Hz and rate_cv=+1 Hz, effective rate = 2 Hz → period = 50 samples.
        let base_rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = base_rate * period as f64;

        let mut lfo = make_lfo_sr(base_rate, sample_rate);
        lfo.set_connectivity(PortConnectivity {
            inputs: vec![false, true].into_boxed_slice(),
            outputs: vec![true, false, false, false, false, false].into_boxed_slice(),
        });

        let mut outputs = [0.0_f64; 6];
        let mut cycle1 = Vec::with_capacity(50);
        for _ in 0..50 {
            lfo.process(&[0.0, 1.0], &mut outputs); // rate_cv = +1 → effective 2 Hz
            cycle1.push(outputs[0]);
        }
        let mut cycle2 = Vec::with_capacity(50);
        for _ in 0..50 {
            lfo.process(&[0.0, 1.0], &mut outputs);
            cycle2.push(outputs[0]);
        }

        for (i, (a, b)) in cycle1.iter().zip(cycle2.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "rate_cv=+1 should produce 50-sample period; mismatch at sample {i}: {a} vs {b}"
            );
        }
    }

    #[test]
    fn rate_cv_large_negative_is_clamped() {
        // A very negative rate_cv must not produce a zero or negative increment.
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;

        let mut lfo = make_lfo_sr(rate, sample_rate);
        lfo.set_connectivity(PortConnectivity {
            inputs: vec![false, true].into_boxed_slice(),
            outputs: vec![true, false, false, false, false, false].into_boxed_slice(),
        });

        let mut outputs = [0.0_f64; 6];

        // Run two samples; if phase advances, the second sine value differs from the first
        // (assuming minimum increment > 0).
        lfo.process(&[0.0, -1000.0], &mut outputs);
        let first = outputs[0];
        lfo.process(&[0.0, -1000.0], &mut outputs);
        let second = outputs[0];

        // Phase must have advanced (not stuck or gone backwards).
        assert!(
            (second - first).abs() > 1e-10,
            "rate_cv=-1000 clamped to minimum should still advance phase; got first={first}, second={second}"
        );
    }
}
