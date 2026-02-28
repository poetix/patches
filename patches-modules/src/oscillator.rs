use std::f64::consts::TAU;

use patches_core::{Module, ModuleDescriptor, PortDescriptor};

/// A sine wave oscillator with fixed frequency set at construction time.
///
/// Phase is accumulated continuously across calls so the waveform has no
/// discontinuities between samples. Phase wraps within `[0, 2π)` to prevent
/// floating-point drift over long runs.
pub struct SineOscillator {
    frequency: f64,
    phase: f64,
    descriptor: ModuleDescriptor,
}

impl SineOscillator {
    pub fn new(frequency: f64) -> Self {
        Self {
            frequency,
            phase: 0.0,
            descriptor: ModuleDescriptor {
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out" }],
            },
        }
    }
}

impl Module for SineOscillator {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn process(&mut self, _inputs: &[f64], outputs: &mut [f64], sample_rate: f64) {
        outputs[0] = self.phase.sin();
        self.phase = (self.phase + TAU * self.frequency / sample_rate) % TAU;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
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
    fn output_completes_full_cycle_in_period_samples() {
        // With frequency=1.0 and sample_rate=440.0 the period is exactly 440 samples.
        let sample_rate = 440.0_f64;
        let frequency = 1.0_f64;
        let period = (sample_rate / frequency) as usize; // 440

        let mut osc = SineOscillator::new(frequency);
        let mut outputs = [0.0_f64; 1];

        // Collect one full cycle.
        let mut first_cycle = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&[], &mut outputs, sample_rate);
            first_cycle.push(outputs[0]);
        }

        // Collect a second full cycle — must match the first within floating-point error.
        let mut second_cycle = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&[], &mut outputs, sample_rate);
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
