use patches_core::{AudioEnvironment, ControlSignal, InstanceId, Module, ModuleDescriptor, PortDescriptor};

/// Middle C2 frequency in Hz (MIDI note 36).
const C2_FREQ: f64 = 65.406_194;

fn advance_phase(phase: &mut f64, freq: f64, sample_rate: f64) {
    *phase = (*phase + freq / sample_rate).fract();
}

/// A sawtooth oscillator with V/OCT pitch input.
///
/// Output = `2.0 * phase - 1.0`, ranging `[-1.0, 1.0)`.
/// Frequency is computed each sample as `C2_FREQ * 2^(base_voct + inputs[0])`.
pub struct SawtoothOscillator {
    instance_id: InstanceId,
    base_voct: f64,
    phase: f64,
    sample_rate: f64,
    descriptor: ModuleDescriptor,
}

impl SawtoothOscillator {
    pub fn new(base_voct: f64) -> Self {
        Self {
            instance_id: InstanceId::next(),
            base_voct,
            phase: 0.0,
            sample_rate: 44100.0,
            descriptor: ModuleDescriptor {
                inputs: vec![PortDescriptor { name: "voct", index: 0 }],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
        }
    }
}

impl Module for SawtoothOscillator {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn initialise(&mut self, env: &AudioEnvironment) {
        self.sample_rate = env.sample_rate;
    }

    fn receive_signal(&mut self, _signal: ControlSignal) {}

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        let freq = C2_FREQ * 2_f64.powf(self.base_voct + inputs[0]);
        outputs[0] = 2.0 * self.phase - 1.0;
        advance_phase(&mut self.phase, freq, self.sample_rate);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A square oscillator with V/OCT pitch input.
///
/// Output = `1.0` when `phase < 0.5`, `-1.0` otherwise.
/// Frequency is computed each sample as `C2_FREQ * 2^(base_voct + inputs[0])`.
pub struct SquareOscillator {
    instance_id: InstanceId,
    base_voct: f64,
    phase: f64,
    sample_rate: f64,
    descriptor: ModuleDescriptor,
}

impl SquareOscillator {
    pub fn new(base_voct: f64) -> Self {
        Self {
            instance_id: InstanceId::next(),
            base_voct,
            phase: 0.0,
            sample_rate: 44100.0,
            descriptor: ModuleDescriptor {
                inputs: vec![
                    PortDescriptor { name: "voct", index: 0 },
                    PortDescriptor { name: "pulse_width", index: 0 }, // for testing distinct instance IDs
                ],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
            },
        }
    }
}

impl Module for SquareOscillator {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn initialise(&mut self, env: &AudioEnvironment) {
        self.sample_rate = env.sample_rate;
    }

    fn receive_signal(&mut self, _signal: ControlSignal) {}

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        let freq = C2_FREQ * 2_f64.powf(self.base_voct + inputs[0]);
        let pulse_width = 0.5 + 0.5 * inputs[1];
        outputs[0] = if self.phase < pulse_width { 1.0 } else { -1.0 };
        advance_phase(&mut self.phase, freq, self.sample_rate);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_env(sample_rate: f64) -> AudioEnvironment {
        AudioEnvironment { sample_rate }
    }

    // --- SawtoothOscillator ---

    #[test]
    fn sawtooth_instance_ids_are_distinct() {
        let a = SawtoothOscillator::new(0.0);
        let b = SawtoothOscillator::new(0.0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn sawtooth_descriptor_ports() {
        let osc = SawtoothOscillator::new(0.0);
        let d = osc.descriptor();
        assert_eq!(d.inputs.len(), 1);
        assert_eq!(d.inputs[0].name, "voct");
        assert_eq!(d.outputs.len(), 1);
        assert_eq!(d.outputs[0].name, "out");
    }

    #[test]
    fn sawtooth_full_cycle_is_consistent() {
        // base_voct=0 → freq = C2_FREQ ≈ 65.4 Hz
        // Use a sample_rate that gives an integer period to avoid rounding error.
        // period = sample_rate / freq; choose sample_rate = C2_FREQ so period = 1 sample —
        // that's too coarse. Instead pick sample_rate = C2_FREQ * 100 so period = 100.
        let sample_rate = C2_FREQ * 100.0;
        let period = 100_usize;

        let mut osc = SawtoothOscillator::new(0.0);
        osc.initialise(&make_env(sample_rate));

        let mut out = [0.0_f64; 1];
        let voct_in = [0.0_f64; 1];

        let mut cycle1 = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&voct_in, &mut out);
            cycle1.push(out[0]);
        }

        let mut cycle2 = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&voct_in, &mut out);
            cycle2.push(out[0]);
        }

        for (a, b) in cycle1.iter().zip(cycle2.iter()) {
            assert!((a - b).abs() < 1e-10, "cycle mismatch: {a} vs {b}");
        }
    }

    // --- SquareOscillator ---

    #[test]
    fn square_instance_ids_are_distinct() {
        let a = SquareOscillator::new(0.0);
        let b = SquareOscillator::new(0.0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn square_descriptor_ports() {
        let osc = SquareOscillator::new(0.0);
        let d = osc.descriptor();
        assert_eq!(d.inputs.len(), 1);
        assert_eq!(d.inputs[0].name, "voct");
        assert_eq!(d.outputs.len(), 1);
        assert_eq!(d.outputs[0].name, "out");
    }

    #[test]
    fn square_full_cycle_is_consistent() {
        let sample_rate = C2_FREQ * 100.0;
        let period = 100_usize;

        let mut osc = SquareOscillator::new(0.0);
        osc.initialise(&make_env(sample_rate));

        let mut out = [0.0_f64; 1];
        let voct_in = [0.0_f64; 1];

        let mut cycle1 = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&voct_in, &mut out);
            cycle1.push(out[0]);
        }

        let mut cycle2 = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&voct_in, &mut out);
            cycle2.push(out[0]);
        }

        for (a, b) in cycle1.iter().zip(cycle2.iter()) {
            assert!((a - b).abs() < 1e-10, "cycle mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn square_output_values_are_only_plus_minus_one() {
        let sample_rate = C2_FREQ * 100.0;
        let mut osc = SquareOscillator::new(0.0);
        osc.initialise(&make_env(sample_rate));
        let mut out = [0.0_f64; 1];
        for _ in 0..200 {
            osc.process(&[0.0], &mut out);
            assert!(out[0] == 1.0 || out[0] == -1.0, "unexpected value: {}", out[0]);
        }
    }
}
