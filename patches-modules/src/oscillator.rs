use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor,
    ModuleShape, ParameterDescriptor, ParameterKind, PortDescriptor,
    PortConnectivity
};
use patches_core::parameter_map::{ParameterMap, ParameterValue};
use crate::common::approximate::lookup_sine;
use crate::common::frequency::{UnitPhaseAccumulator, FMMode};

/// A sine wave oscillator whose frequency is set via the `"frequency"` parameter.
///
/// Phase is accumulated continuously across calls so the waveform has no
/// discontinuities between samples. Phase wraps within `[0, 1)` to prevent
/// floating-point drift over long runs.
///
/// Constructed via the Module v2 protocol: `describe` → `prepare` →
/// `update_validated_parameters`.
pub struct SineOscillator {
    instance_id: InstanceId,
    phase_accumulator: UnitPhaseAccumulator,
    descriptor: ModuleDescriptor,
}

impl Module for SineOscillator {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "SineOscillator",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "voct", index: 0 },
                PortDescriptor { name: "fm", index: 0 },
            ],
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
            parameters: vec![
                ParameterDescriptor {
                    name: "frequency",
                    index: 0,
                    parameter_type: ParameterKind::Float {
                        min: 0.01,
                        max: 20_000.0,
                        default: 440.0,
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

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor) -> Self {
        Self {
            instance_id: InstanceId::next(),
            phase_accumulator: UnitPhaseAccumulator::new(audio_environment.sample_rate),
            descriptor,
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("frequency") {
            self.phase_accumulator.set_base_frequency(*v);
        }
        if let Some(ParameterValue::Enum(v)) = params.get("fm_type") {
            let mode = *v;
            let fm_mode = match mode {
                "linear" => FMMode::Linear,
                "logarithmic" => FMMode::Exponential,
                _ => return, // Ignore unknown enum variants.
            };
            self.phase_accumulator.set_fm_mode(fm_mode);
        }
    }

    fn set_connectivity(&mut self, connectivity: PortConnectivity) {
        self.phase_accumulator.set_modulation(
            connectivity.inputs[0],
            connectivity.inputs[1],
        );
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        outputs[0] = lookup_sine(self.phase_accumulator.phase);

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
    use patches_core::{AudioEnvironment, Module, ModuleShape, Registry};
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
            &ModuleShape { channels: 0, length: 0 },
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
