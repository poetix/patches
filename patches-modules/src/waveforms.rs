use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor,
    ModuleShape, ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::parameter_map::{ParameterMap, ParameterValue};

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

impl Module for SawtoothOscillator {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "SawtoothOscillator",
            shape: shape.clone(),
            inputs: vec![PortDescriptor { name: "voct", index: 0 }],
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
            parameters: vec![ParameterDescriptor {
                name: "base_voct",
                index: 0,
                parameter_type: ParameterKind::Float { min: -4.0, max: 8.0, default: 0.0 },
            }],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor) -> Self {
        Self {
            instance_id: InstanceId::next(),
            base_voct: 0.0,
            phase: 0.0,
            sample_rate: audio_environment.sample_rate,
            descriptor,
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("base_voct") {
            self.base_voct = *v;
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        let freq = C2_FREQ * 2_f64.powf(self.base_voct + inputs[0]);
        outputs[0] = 2.0 * self.phase - 1.0;
        advance_phase(&mut self.phase, freq, self.sample_rate);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A square oscillator with V/OCT pitch input and variable pulse width.
///
/// Output = `1.0` when `phase < pulse_width`, `-1.0` otherwise.
/// Frequency is computed each sample as `C2_FREQ * 2^(base_voct + inputs[0])`.
/// Pulse width is `0.5 + 0.5 * inputs[1]`, so `inputs[1] = 0` gives a 50% duty cycle.
pub struct SquareOscillator {
    instance_id: InstanceId,
    base_voct: f64,
    phase: f64,
    sample_rate: f64,
    descriptor: ModuleDescriptor,
}

impl Module for SquareOscillator {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "SquareOscillator",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "voct", index: 0 },
                PortDescriptor { name: "pulse_width", index: 0 },
            ],
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
            parameters: vec![ParameterDescriptor {
                name: "base_voct",
                index: 0,
                parameter_type: ParameterKind::Float { min: -4.0, max: 8.0, default: 0.0 },
            }],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor) -> Self {
        Self {
            instance_id: InstanceId::next(),
            base_voct: 0.0,
            phase: 0.0,
            sample_rate: audio_environment.sample_rate,
            descriptor,
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("base_voct") {
            self.base_voct = *v;
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

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
    use patches_core::{AudioEnvironment, Module, ModuleShape, Registry};
    use patches_core::parameter_map::{ParameterMap, ParameterValue};

    fn make_sawtooth(base_voct: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("base_voct".into(), ParameterValue::Float(base_voct));
        let mut r = Registry::new();
        r.register::<SawtoothOscillator>();
        r.create(
            "SawtoothOscillator",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels: 0, length: 0 },
            &params,
        ).unwrap()
    }

    fn make_sawtooth_sr(base_voct: f64, sample_rate: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("base_voct".into(), ParameterValue::Float(base_voct));
        let mut r = Registry::new();
        r.register::<SawtoothOscillator>();
        r.create(
            "SawtoothOscillator",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0, length: 0 },
            &params,
        ).unwrap()
    }

    fn make_square(base_voct: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("base_voct".into(), ParameterValue::Float(base_voct));
        let mut r = Registry::new();
        r.register::<SquareOscillator>();
        r.create(
            "SquareOscillator",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels: 0, length: 0 },
            &params,
        ).unwrap()
    }

    fn make_square_sr(base_voct: f64, sample_rate: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("base_voct".into(), ParameterValue::Float(base_voct));
        let mut r = Registry::new();
        r.register::<SquareOscillator>();
        r.create(
            "SquareOscillator",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0, length: 0 },
            &params,
        ).unwrap()
    }

    // --- SawtoothOscillator ---

    #[test]
    fn sawtooth_instance_ids_are_distinct() {
        let a = make_sawtooth(0.0);
        let b = make_sawtooth(0.0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn sawtooth_descriptor_ports() {
        let osc = make_sawtooth(0.0);
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

        let mut osc = make_sawtooth_sr(0.0, sample_rate);

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
        let a = make_square(0.0);
        let b = make_square(0.0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn square_descriptor_ports() {
        let osc = make_square(0.0);
        let d = osc.descriptor();
        assert_eq!(d.inputs.len(), 2);
        assert_eq!(d.inputs[0].name, "voct");
        assert_eq!(d.inputs[1].name, "pulse_width");
        assert_eq!(d.outputs.len(), 1);
        assert_eq!(d.outputs[0].name, "out");
    }

    #[test]
    fn square_full_cycle_is_consistent() {
        let sample_rate = C2_FREQ * 100.0;
        let period = 100_usize;

        let mut osc = make_square_sr(0.0, sample_rate);

        let mut out = [0.0_f64; 1];
        // SquareOscillator has 2 inputs: voct and pulse_width.
        let inputs = [0.0_f64, 0.0_f64];

        let mut cycle1 = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&inputs, &mut out);
            cycle1.push(out[0]);
        }

        let mut cycle2 = Vec::with_capacity(period);
        for _ in 0..period {
            osc.process(&inputs, &mut out);
            cycle2.push(out[0]);
        }

        for (a, b) in cycle1.iter().zip(cycle2.iter()) {
            assert!((a - b).abs() < 1e-10, "cycle mismatch: {a} vs {b}");
        }
    }

    #[test]
    fn square_output_values_are_only_plus_minus_one() {
        let sample_rate = C2_FREQ * 100.0;
        let mut osc = make_square_sr(0.0, sample_rate);
        let mut out = [0.0_f64; 1];
        for _ in 0..200 {
            osc.process(&[0.0, 0.0], &mut out);
            assert!(out[0] == 1.0 || out[0] == -1.0, "unexpected value: {}", out[0]);
        }
    }
}
