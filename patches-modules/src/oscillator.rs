use std::f64::consts::TAU;

use patches_core::{AudioEnvironment, ControlSignal, InstanceId, Module, ModuleDescriptor, PortDescriptor};

/// A sine wave oscillator with fixed frequency set at construction time.
///
/// Phase is accumulated continuously across calls so the waveform has no
/// discontinuities between samples. Phase wraps within `[0, 2π)` to prevent
/// floating-point drift over long runs.
///
/// `sample_rate` is received once via [`Module::initialise`] and stored
/// internally; it must be called before `process` produces meaningful output.
pub struct SineOscillator {
    instance_id: InstanceId,
    frequency: f64,
    phase: f64,
    /// Reciprocal of the sample rate; updated in `initialise`.
    /// Stored so `phase_increment` can be recalculated on frequency changes
    /// without a division in the hot path.
    sample_rate_reciprocal: f64,
    /// `TAU * frequency * sample_rate_reciprocal`; recomputed whenever either
    /// value changes. Used in `process` as a multiply-only phase step.
    phase_increment: f64,
    descriptor: ModuleDescriptor,
}

impl SineOscillator {
    pub fn new(frequency: f64) -> Self {
        let sample_rate_reciprocal = 1.0 / 44100.0;
        Self {
            instance_id: InstanceId::next(),
            frequency,
            phase: 0.0,
            sample_rate_reciprocal,
            phase_increment: TAU * frequency * sample_rate_reciprocal,
            descriptor: ModuleDescriptor {
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
        }
    }
}

impl Module for SineOscillator {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn initialise(&mut self, env: &AudioEnvironment) {
        self.sample_rate_reciprocal = 1.0 / env.sample_rate;
        self.phase_increment = TAU * self.frequency * self.sample_rate_reciprocal;
    }

    fn receive_signal(&mut self, signal: ControlSignal) {
        if let ControlSignal::Float { name: "freq", value } = signal {
            self.frequency = value;
            self.phase_increment = TAU * value * self.sample_rate_reciprocal;
        }
    }

    fn process(&mut self, _inputs: &[f64], outputs: &mut [f64]) {
        outputs[0] = self.phase.sin();
        self.phase = (self.phase + self.phase_increment) % TAU;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_has_no_inputs_and_one_output() {
        let osc = SineOscillator::new(440.0);
        let desc = osc.descriptor();
        assert_eq!(desc.inputs.len(), 0);
        assert_eq!(desc.outputs.len(), 1);
        assert_eq!(desc.outputs[0].name, "out");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = SineOscillator::new(440.0);
        let b = SineOscillator::new(440.0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn receive_signal_freq_updates_frequency() {
        let mut osc = SineOscillator::new(440.0);
        osc.receive_signal(ControlSignal::Float { name: "freq", value: 880.0 });
        // Confirm the frequency field changed by checking output diverges from 440 Hz.
        osc.initialise(&AudioEnvironment { sample_rate: 44100.0 });
        let mut out = [0.0_f64; 1];
        osc.process(&[], &mut out);
        // At sample 0, phase=0 so sin(0)=0 regardless of frequency.
        // At sample 1, the phase step differs between 440 and 880 Hz.
        osc.process(&[], &mut out);
        let step_880 = TAU * 880.0 / 44100.0;
        assert!((out[0] - step_880.sin()).abs() < 1e-10, "unexpected output: {}", out[0]);
    }

    #[test]
    fn receive_signal_unknown_name_is_ignored() {
        let mut osc = SineOscillator::new(440.0);
        osc.receive_signal(ControlSignal::Float { name: "gain", value: 0.5 });
        // frequency must remain 440 Hz
        osc.initialise(&AudioEnvironment { sample_rate: 44100.0 });
        let mut out = [0.0_f64; 1];
        osc.process(&[], &mut out); // phase 0
        osc.process(&[], &mut out); // phase after one step at 440 Hz
        let step_440 = TAU * 440.0 / 44100.0;
        assert!((out[0] - step_440.sin()).abs() < 1e-10, "unexpected output: {}", out[0]);
    }

    #[test]
    fn output_completes_full_cycle_in_period_samples() {
        // With frequency=1.0 and sample_rate=440.0 the period is exactly 440 samples.
        let sample_rate = 440.0_f64;
        let frequency = 1.0_f64;
        let period = (sample_rate / frequency) as usize; // 440

        let mut osc = SineOscillator::new(frequency);
        osc.initialise(&AudioEnvironment { sample_rate });
        let mut outputs = [0.0_f64; 1];

        // Collect one full cycle.
        let mut first_cycle = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&[], &mut outputs);
            first_cycle.push(outputs[0]);
        }

        // Collect a second full cycle — must match the first within floating-point error.
        let mut second_cycle = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&[], &mut outputs);
            second_cycle.push(outputs[0]);
        }

        for (a, b) in first_cycle.iter().zip(second_cycle.iter()) {
            assert!(
                (a - b).abs() < 1e-10,
                "cycle mismatch: {a} vs {b}"
            );
        }
    }
}
