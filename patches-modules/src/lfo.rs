use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, ModuleShape, OutputPort, ParameterDescriptor, ParameterKind, PortDescriptor,
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
    // Input port fields
    in_sync: MonoInput,
    in_rate_cv: MonoInput,
    // Output port fields
    out_sine: MonoOutput,
    out_triangle: MonoOutput,
    out_saw_up: MonoOutput,
    out_saw_down: MonoOutput,
    out_square: MonoOutput,
    out_random: MonoOutput,
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
            in_sync: MonoInput::default(),
            in_rate_cv: MonoInput::default(),
            out_sine: MonoOutput::default(),
            out_triangle: MonoOutput::default(),
            out_saw_up: MonoOutput::default(),
            out_saw_down: MonoOutput::default(),
            out_square: MonoOutput::default(),
            out_random: MonoOutput::default(),
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

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_sync = MonoInput::from_ports(inputs, 0);
        self.in_rate_cv = MonoInput::from_ports(inputs, 1);
        self.out_sine = MonoOutput::from_ports(outputs, 0);
        self.out_triangle = MonoOutput::from_ports(outputs, 1);
        self.out_saw_up = MonoOutput::from_ports(outputs, 2);
        self.out_saw_down = MonoOutput::from_ports(outputs, 3);
        self.out_square = MonoOutput::from_ports(outputs, 4);
        self.out_random = MonoOutput::from_ports(outputs, 5);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        // Sync: rising edge (prev <= 0, current > 0) resets phase before advance.
        if self.in_sync.is_connected() {
            let sync_val = pool.read_mono(&self.in_sync);
            if self.prev_sync <= 0.0 && sync_val > 0.0 {
                self.phase = 0.0;
            }
            self.prev_sync = sync_val;
        }

        // Rate CV: recompute increment per-sample when connected.
        let increment = if self.in_rate_cv.is_connected() {
            (self.rate + pool.read_mono(&self.in_rate_cv)).clamp(0.001, 40.0) / self.sample_rate
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

        if self.out_sine.is_connected() {
            pool.write_mono(&self.out_sine, apply_mode(lookup_sine(read_phase), mode));
        }
        if self.out_triangle.is_connected() {
            pool.write_mono(&self.out_triangle, apply_mode(1.0 - 4.0 * (read_phase - 0.5).abs(), mode));
        }
        if self.out_saw_up.is_connected() {
            pool.write_mono(&self.out_saw_up, apply_mode(2.0 * read_phase - 1.0, mode));
        }
        if self.out_saw_down.is_connected() {
            pool.write_mono(&self.out_saw_down, apply_mode(1.0 - 2.0 * read_phase, mode));
        }
        if self.out_square.is_connected() {
            let v = if read_phase < 0.5 { 1.0 } else { -1.0 };
            pool.write_mono(&self.out_square, apply_mode(v, mode));
        }
        if self.out_random.is_connected() {
            pool.write_mono(&self.out_random, apply_mode(self.random_value, mode));
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{AudioEnvironment, CablePool, CableValue, Module, ModuleShape, Registry};
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
            &AudioEnvironment { sample_rate, poly_voices: 16 },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

    // Inputs: 0=sync, 1=rate_cv; Outputs: 2=sine, 3=triangle, 4=saw_up, 5=saw_down, 6=square, 7=random
    fn set_all_outputs_connected(module: &mut Box<dyn Module>) {
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: false }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 2, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 3, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 5, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 6, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 7, connected: true }),
        ];
        module.set_ports(&inputs, &outputs);
    }

    fn set_no_outputs_connected(module: &mut Box<dyn Module>) {
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: false }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 2, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 3, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 5, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 6, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 7, connected: false }),
        ];
        module.set_ports(&inputs, &outputs);
    }

    fn set_sync_and_rate_cv_connected(module: &mut Box<dyn Module>) {
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: true }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 2, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 3, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 5, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 6, connected: false }),
            OutputPort::Mono(MonoOutput { cable_idx: 7, connected: false }),
        ];
        module.set_ports(&inputs, &outputs);
    }

    #[test]
    fn sine_output_consistent_across_two_cycles() {
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;

        let mut lfo = make_lfo_sr(rate, sample_rate);
        set_all_outputs_connected(&mut lfo);
        let mut pool = make_pool(8);

        let mut cycle1 = Vec::with_capacity(period);
        for i in 0..period {
            let wi = i % 2;
            lfo.process(&mut CablePool::new(&mut pool, wi));
            if let CableValue::Mono(v) = pool[2][wi] { cycle1.push(v); }
        }
        let mut cycle2 = Vec::with_capacity(period);
        for i in 0..period {
            let wi = (period + i) % 2;
            lfo.process(&mut CablePool::new(&mut pool, wi));
            if let CableValue::Mono(v) = pool[2][wi] { cycle2.push(v); }
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

        let mut lfo_base = make_lfo_sr(rate, sample_rate);
        set_all_outputs_connected(&mut lfo_base);
        let mut pool_base = make_pool(8);
        let mut base_cycle = Vec::with_capacity(period);
        for i in 0..period {
            let wi = i % 2;
            lfo_base.process(&mut CablePool::new(&mut pool_base, wi));
            if let CableValue::Mono(v) = pool_base[2][wi] { base_cycle.push(v); }
        }

        let mut params = ParameterMap::new();
        params.insert("rate".into(), ParameterValue::Float(rate));
        params.insert("phase_offset".into(), ParameterValue::Float(0.25));
        let mut r = Registry::new();
        r.register::<Lfo>();
        let mut lfo_shifted = r.create(
            "Lfo",
            &AudioEnvironment { sample_rate, poly_voices: 16 },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap();
        set_all_outputs_connected(&mut lfo_shifted);

        let mut pool_shifted = make_pool(8);
        let mut shifted_cycle = Vec::with_capacity(period);
        for i in 0..period {
            let wi = i % 2;
            lfo_shifted.process(&mut CablePool::new(&mut pool_shifted, wi));
            if let CableValue::Mono(v) = pool_shifted[2][wi] { shifted_cycle.push(v); }
        }

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
            &AudioEnvironment { sample_rate, poly_voices: 16 },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap();
        set_all_outputs_connected(&mut lfo);

        let mut pool = make_pool(8);
        for i in 0..period {
            let wi = i % 2;
            lfo.process(&mut CablePool::new(&mut pool, wi));
            if let CableValue::Mono(v) = pool[4][wi] {
                assert!(
                    v >= 0.0 && v <= 1.0,
                    "unipolar_positive saw_up must be in [0, 1]; got {}", v
                );
            }
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
            &AudioEnvironment { sample_rate, poly_voices: 16 },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap();
        set_all_outputs_connected(&mut lfo);

        let mut pool = make_pool(8);
        let mut tick_count = 0usize;

        for _cycle in 0..3 {
            let wi = tick_count % 2;
            lfo.process(&mut CablePool::new(&mut pool, wi));
            let cycle_value = if let CableValue::Mono(v) = pool[7][wi] { v } else { panic!("Mono"); };
            tick_count += 1;
            assert!(
                cycle_value >= 0.0 && cycle_value <= 1.0,
                "random output must be in [0, 1] in unipolar_positive mode; got {cycle_value}"
            );
            for _ in 1..(period - 1) {
                let wi = tick_count % 2;
                lfo.process(&mut CablePool::new(&mut pool, wi));
                if let CableValue::Mono(v) = pool[7][wi] {
                    assert!(
                        (v - cycle_value).abs() < 1e-15,
                        "random output must hold within a period; changed from {cycle_value} to {}", v
                    );
                }
                tick_count += 1;
            }
            lfo.process(&mut CablePool::new(&mut pool, tick_count % 2));
            tick_count += 1;
        }
    }

    #[test]
    fn disconnected_outputs_are_not_written() {
        let mut lfo = make_lfo(1.0);
        set_no_outputs_connected(&mut lfo);
        let mut pool: Vec<[CableValue; 2]> = (0..8).map(|_| [CableValue::Mono(99.0); 2]).collect();
        lfo.process(&mut CablePool::new(&mut pool, 0));
        for i in 2..8 {
            if let CableValue::Mono(v) = pool[i][0] {
                assert_eq!(v, 99.0, "output[{i}] was written despite being disconnected");
            }
        }
    }

    #[test]
    fn sync_rising_edge_resets_phase_mid_cycle() {
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;
        let mut lfo = make_lfo_sr(rate, sample_rate);
        set_sync_and_rate_cv_connected(&mut lfo);
        let mut pool = make_pool(8);

        // Advance 25 samples (quarter-cycle) with sync low.
        for i in 0..25 {
            let wi = i % 2;
            pool[0][1 - wi] = CableValue::Mono(0.0);
            pool[1][1 - wi] = CableValue::Mono(0.0);
            lfo.process(&mut CablePool::new(&mut pool, wi));
        }

        // Rising edge: sync goes from 0 → 1.
        pool[0][1 - (25 % 2)] = CableValue::Mono(1.0);
        pool[1][1 - (25 % 2)] = CableValue::Mono(0.0);
        lfo.process(&mut CablePool::new(&mut pool, 25 % 2));
        let after_reset = if let CableValue::Mono(v) = pool[2][25 % 2] { v } else { panic!(); };

        // A fresh LFO at sample 1 should match.
        let mut lfo_fresh = make_lfo_sr(rate, sample_rate);
        set_all_outputs_connected(&mut lfo_fresh);
        let mut pool_fresh = make_pool(8);
        lfo_fresh.process(&mut CablePool::new(&mut pool_fresh, 0));
        let expected = if let CableValue::Mono(v) = pool_fresh[2][0] { v } else { panic!(); };

        assert!(
            (after_reset - expected).abs() < 1e-10,
            "after sync reset sine={after_reset}, expected fresh LFO sine={expected}"
        );
    }

    #[test]
    fn sync_level_does_not_retrigger() {
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;
        let mut lfo = make_lfo_sr(rate, sample_rate);
        set_sync_and_rate_cv_connected(&mut lfo);
        let mut pool = make_pool(8);

        // Trigger a rising edge first (prev=0 → 1), then hold high.
        pool[0][1] = CableValue::Mono(1.0);
        pool[1][1] = CableValue::Mono(0.0);
        lfo.process(&mut CablePool::new(&mut pool, 0));

        let mut values = Vec::new();
        for i in 0..25 {
            let wi = (1 + i) % 2;
            let ri = 1 - wi;
            pool[0][ri] = CableValue::Mono(1.0);
            pool[1][ri] = CableValue::Mono(0.0);
            lfo.process(&mut CablePool::new(&mut pool, wi));
            if let CableValue::Mono(v) = pool[2][wi] { values.push(v); }
        }

        // Compare against an identical fresh LFO.
        let mut lfo_ref = make_lfo_sr(rate, sample_rate);
        set_sync_and_rate_cv_connected(&mut lfo_ref);
        let mut pool_ref = make_pool(8);
        pool_ref[0][1] = CableValue::Mono(1.0);
        pool_ref[1][1] = CableValue::Mono(0.0);
        lfo_ref.process(&mut CablePool::new(&mut pool_ref, 0));
        let mut ref_values = Vec::new();
        for i in 0..25 {
            let wi = (1 + i) % 2;
            let ri = 1 - wi;
            pool_ref[0][ri] = CableValue::Mono(1.0);
            pool_ref[1][ri] = CableValue::Mono(0.0);
            lfo_ref.process(&mut CablePool::new(&mut pool_ref, wi));
            if let CableValue::Mono(v) = pool_ref[2][wi] { ref_values.push(v); }
        }

        for (i, (&v, &r)) in values.iter().zip(ref_values.iter()).enumerate() {
            assert!((v - r).abs() < 1e-10, "sample {i}: sync level caused retrigger; got {v} vs ref {r}");
        }
    }

    #[test]
    fn rate_cv_doubles_rate_halves_period() {
        let base_rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = base_rate * period as f64;

        let mut lfo = make_lfo_sr(base_rate, sample_rate);
        set_sync_and_rate_cv_connected(&mut lfo);
        let mut pool = make_pool(8);
        let mut cycle1 = Vec::with_capacity(50);
        for i in 0..50 {
            let wi = i % 2;
            pool[0][1 - wi] = CableValue::Mono(0.0);
            pool[1][1 - wi] = CableValue::Mono(1.0); // rate_cv = +1 → effective 2 Hz
            lfo.process(&mut CablePool::new(&mut pool, wi));
            if let CableValue::Mono(v) = pool[2][wi] { cycle1.push(v); }
        }
        let mut cycle2 = Vec::with_capacity(50);
        for i in 0..50 {
            let tick = 50 + i;
            let wi = tick % 2;
            pool[0][1 - wi] = CableValue::Mono(0.0);
            pool[1][1 - wi] = CableValue::Mono(1.0);
            lfo.process(&mut CablePool::new(&mut pool, wi));
            if let CableValue::Mono(v) = pool[2][wi] { cycle2.push(v); }
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
        let rate = 1.0_f64;
        let period = 100_usize;
        let sample_rate = rate * period as f64;

        let mut lfo = make_lfo_sr(rate, sample_rate);
        set_sync_and_rate_cv_connected(&mut lfo);
        let mut pool = make_pool(8);

        pool[0][1] = CableValue::Mono(0.0);
        pool[1][1] = CableValue::Mono(-1000.0);
        lfo.process(&mut CablePool::new(&mut pool, 0));
        let first = if let CableValue::Mono(v) = pool[2][0] { v } else { panic!(); };

        pool[0][0] = CableValue::Mono(0.0);
        pool[1][0] = CableValue::Mono(-1000.0);
        lfo.process(&mut CablePool::new(&mut pool, 1));
        let second = if let CableValue::Mono(v) = pool[2][1] { v } else { panic!(); };

        assert!(
            (second - first).abs() > 1e-10,
            "rate_cv=-1000 clamped to minimum should still advance phase; got first={first}, second={second}"
        );
    }
}
