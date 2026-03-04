use patches_core::{
    AudioEnvironment, ControlSignal, InstanceId, Module, ModuleDescriptor,
    ModuleShape, ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::parameter_map::{ParameterMap, ParameterValue};

/// A portamento (pitch glide) module.
///
/// Smooths frequency changes over a configurable time using a one-pole
/// low-pass filter operating in log-frequency space to give perceptually
/// linear glide. The glide time is set via the `"glide_ms"` parameter.
///
/// Constructed via the Module v2 protocol: `describe` → `prepare` →
/// `update_validated_parameters`.
pub struct Glide {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    log_freq: f64,
    alpha: f64,
    beta: f64,
    glide_ms: f64,
    sample_rate: f64,
}

impl Glide {
    fn update_beta(&mut self) {
        let n_samples = self.sample_rate * self.glide_ms / 1000.0;
        if n_samples <= 0.0 {
            self.beta = 1.0;
        } else {
            self.beta = 1.0 - self.alpha.powf(1.0 / n_samples);
        }
    }

    fn set_glide_ms(&mut self, glide_ms: f64) {
        self.glide_ms = glide_ms;
        self.update_beta();
    }
}

impl Module for Glide {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Glide",
            shape: shape.clone(),
            inputs: vec![PortDescriptor { name: "in", index: 0 }],
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
            parameters: vec![ParameterDescriptor {
                name: "glide_ms",
                index: 0,
                parameter_type: ParameterKind::Float {
                    min: 0.0,
                    max: 10_000.0,
                    default: 100.0,
                },
            }],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor) -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor,
            log_freq: 0.0,
            alpha: 0.01,
            beta: 0.0,
            glide_ms: 0.0,
            sample_rate: audio_environment.sample_rate,
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("glide_ms") {
            self.set_glide_ms(*v);
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn receive_signal(&mut self, signal: ControlSignal) {
        if let ControlSignal::ParameterUpdate {
            name: "glide_ms",
            value: ParameterValue::Float(v),
        } = signal
        {
            self.set_glide_ms(v);
        }
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        let log_target = if inputs[0] > 0.0 { inputs[0].ln() } else { self.log_freq };
        self.log_freq += self.beta * (log_target - self.log_freq);
        outputs[0] = self.log_freq.exp();
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

    fn make_glide(glide_ms: f64) -> Box<dyn Module> {
        make_glide_sr(glide_ms, 44100.0)
    }

    fn make_glide_sr(glide_ms: f64, sample_rate: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("glide_ms".into(), ParameterValue::Float(glide_ms));
        let mut r = Registry::new();
        r.register::<Glide>();
        r.create(
            "Glide",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0 },
            &params,
        ).unwrap()
    }

    #[test]
    fn descriptor_ports() {
        let g = make_glide(100.0);
        let desc = g.descriptor();
        assert_eq!(desc.inputs.len(), 1);
        assert_eq!(desc.inputs[0].name, "in");
        assert_eq!(desc.outputs.len(), 1);
        assert_eq!(desc.outputs[0].name, "out");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_glide(100.0);
        let b = make_glide(100.0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn output_tracks_input_with_glide() {
        // With a non-zero glide time the output should not jump immediately to
        // the target frequency. It should be moving toward it but still differ
        // from it after a small number of samples.
        let mut g = make_glide_sr(500.0, 44100.0);
        let target_freq = 880.0_f64;

        // Seed the log_freq by processing one sample at the starting frequency.
        let start_freq = 440.0_f64;
        let mut out = [0.0_f64; 1];
        g.process(&[start_freq], &mut out);
        let after_start = out[0];

        // Now switch to a higher target and process a few samples.
        g.process(&[target_freq], &mut out);
        let after_step = out[0];

        // The output must not have jumped all the way to the target in one sample.
        assert!(
            after_step < target_freq,
            "expected output {after_step} to be below target {target_freq} (glide should smooth)"
        );
        // But it must have moved in the right direction.
        assert!(
            after_step > after_start,
            "expected output {after_step} to have increased from {after_start}"
        );
    }

    #[test]
    fn zero_glide_ms_tracks_instantly() {
        // With glide_ms=0.0 beta must be 1.0 so the output matches the input
        // in the same sample.
        let mut g = make_glide_sr(0.0, 44100.0);
        let target_freq = 880.0_f64;
        let mut out = [0.0_f64; 1];
        g.process(&[target_freq], &mut out);
        assert!(
            (out[0] - target_freq).abs() < 1e-9,
            "expected instant tracking, got {}", out[0]
        );
    }

    #[test]
    fn receive_signal_updates_glide_time() {
        // Start with a long glide, verify smoothing is present. Then switch to
        // zero glide via receive_signal and verify instant tracking.
        let mut g = make_glide_sr(5000.0, 44100.0);

        // Seed the module at 440 Hz.
        let mut out = [0.0_f64; 1];
        g.process(&[440.0], &mut out);

        // Verify that the output does not jump immediately to a new target.
        g.process(&[880.0], &mut out);
        let smooth_out = out[0];
        assert!(
            smooth_out < 880.0,
            "expected smoothed output {smooth_out} to be below 880 Hz with long glide"
        );

        // Now reduce glide to zero via receive_signal.
        g.receive_signal(ControlSignal::ParameterUpdate {
            name: "glide_ms",
            value: ParameterValue::Float(0.0),
        });

        // Process at the target; output should now track immediately.
        g.process(&[880.0], &mut out);
        assert!(
            (out[0] - 880.0).abs() < 1e-9,
            "expected instant tracking after receive_signal(0 ms), got {}", out[0]
        );
    }
}
