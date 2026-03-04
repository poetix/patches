use std::f64::consts::TAU;

use patches_core::{
    AudioEnvironment, ControlSignal, InstanceId, Module, ModuleDescriptor,
    ModuleShape, ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::build_error::BuildError;
use patches_core::parameter_map::{ParameterMap, ParameterValue};

/// A sine wave oscillator whose frequency is set via the `"frequency"` parameter.
///
/// Phase is accumulated continuously across calls so the waveform has no
/// discontinuities between samples. Phase wraps within `[0, 2π)` to prevent
/// floating-point drift over long runs.
///
/// Constructed via the Module v2 protocol: `describe` → `prepare` →
/// `update_validated_parameters`.
pub struct SineOscillator {
    instance_id: InstanceId,
    frequency: f64,
    phase: f64,
    /// Reciprocal of the sample rate, set in `prepare`.
    /// Stored so `phase_increment` can be recalculated on frequency changes
    /// without a division in the hot path.
    sample_rate_reciprocal: f64,
    /// `TAU * frequency * sample_rate_reciprocal`; recomputed whenever either
    /// value changes. Used in `process` as a multiply-only phase step.
    phase_increment: f64,
    descriptor: ModuleDescriptor,
}

impl Module for SineOscillator {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "SineOscillator",
            shape: shape.clone(),
            inputs: vec![],
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
            parameters: vec![ParameterDescriptor {
                name: "frequency",
                index: 0,
                parameter_type: ParameterKind::Float {
                    min: 0.01,
                    max: 20_000.0,
                    default: 440.0,
                },
            }],
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor) -> Self {
        Self {
            instance_id: InstanceId::next(),
            frequency: 0.0,
            phase: 0.0,
            sample_rate_reciprocal: 1.0 / audio_environment.sample_rate,
            phase_increment: 0.0,
            descriptor,
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) -> Result<(), BuildError> {
        if let Some(ParameterValue::Float(v)) = params.get("frequency") {
            self.frequency = *v;
            self.phase_increment = TAU * *v * self.sample_rate_reciprocal;
        }
        Ok(())
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn receive_signal(&mut self, signal: ControlSignal) {
        if let ControlSignal::ParameterUpdate { name: "frequency", value: ParameterValue::Float(v) } = signal {
            self.frequency = v;
            self.phase_increment = TAU * v * self.sample_rate_reciprocal;
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
    use patches_core::{AudioEnvironment, ControlSignal, Module, ModuleShape, Registry};
    use patches_core::parameter_map::{ParameterMap, ParameterValue};

    fn make_osc(frequency: f64) -> Box<dyn Module> {
        make_osc_sr(frequency, 44100.0)
    }

    fn make_osc_sr(frequency: f64, sample_rate: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("frequency".into(), ParameterValue::Float(frequency));
        let mut r = Registry::new();
        r.register::<SineOscillator>();
        r.create(
            "SineOscillator",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0 },
            &params,
        ).unwrap()
    }

    #[test]
    fn descriptor_has_no_inputs_and_one_output() {
        let osc = make_osc(440.0);
        let desc = osc.descriptor();
        assert_eq!(desc.inputs.len(), 0);
        assert_eq!(desc.outputs.len(), 1);
        assert_eq!(desc.outputs[0].name, "out");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_osc(440.0);
        let b = make_osc(440.0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn receive_signal_freq_updates_frequency() {
        let mut osc = make_osc(440.0);
        osc.receive_signal(ControlSignal::ParameterUpdate {
            name: "frequency",
            value: ParameterValue::Float(880.0),
        });
        let mut out = [0.0_f64; 1];
        osc.process(&[], &mut out);
        // At sample 0, phase=0 so sin(0)=0 regardless of frequency.
        // At sample 1, the phase step reflects the updated 880 Hz.
        osc.process(&[], &mut out);
        let step_880 = TAU * 880.0 / 44100.0;
        assert!((out[0] - step_880.sin()).abs() < 1e-10, "unexpected output: {}", out[0]);
    }

    #[test]
    fn receive_signal_unknown_name_is_ignored() {
        let mut osc = make_osc(440.0);
        osc.receive_signal(ControlSignal::ParameterUpdate {
            name: "gain",
            value: ParameterValue::Float(0.5),
        });
        // frequency must remain 440 Hz
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

        let mut osc = make_osc_sr(frequency, sample_rate);
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
