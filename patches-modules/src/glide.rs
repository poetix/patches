use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, ModuleShape, OutputPort, ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::{ParameterMap, ParameterValue};

/// A portamento (pitch glide) module.
///
/// Smooths V/OCT pitch values using a one-pole low-pass filter. Because V/OCT
/// is a log-frequency scale (1 V/OCT = 1 octave), interpolating linearly in
/// V/OCT space gives perceptually linear (constant-ratio) glide. The glide
/// time is set via the `"glide_ms"` parameter.
///
/// Constructed via the Module v2 protocol: `describe` → `prepare` →
/// `update_validated_parameters`.
pub struct Glide {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    /// Current smoothed V/OCT value (C2 = 0.0).
    voct: f64,
    alpha: f64,
    beta: f64,
    glide_ms: f64,
    sample_rate: f64,
    // Port fields
    in_port: MonoInput,
    out_port: MonoOutput,
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
            inputs: vec![PortDescriptor { name: "in", index: 0, kind: CableKind::Mono }],
            outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Mono }],
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

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            voct: 0.0,
            alpha: 0.01,
            beta: 0.0,
            glide_ms: 0.0,
            sample_rate: audio_environment.sample_rate,
            in_port: MonoInput::default(),
            out_port: MonoOutput::default(),
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

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_port = MonoInput::from_ports(inputs, 0);
        self.out_port = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        // Input is V/OCT (C2 = 0.0). Interpolate directly in V/OCT space —
        // no ln/exp needed since V/OCT is already a log-frequency scale.
        let input = pool.read_mono(&self.in_port);
        self.voct += self.beta * (input - self.voct);
        pool.write_mono(&self.out_port, self.voct);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use patches_core::{AudioEnvironment, CablePool, Module, ModuleShape, Registry};
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
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

    fn set_ports_for_test(module: &mut Box<dyn Module>) {
        let inputs = vec![InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: true })];
        let outputs = vec![OutputPort::Mono(MonoOutput { cable_idx: 1, connected: true })];
        module.set_ports(&inputs, &outputs);
    }

    #[test]
    fn output_tracks_input_with_glide() {
        let mut g = make_glide_sr(500.0, 44100.0);
        set_ports_for_test(&mut g);
        let start_voct = 1.0_f64;
        let target_voct = 2.0_f64;

        let mut pool = make_pool(2);
        pool[0][1] = CableValue::Mono(start_voct);
        g.process(&mut CablePool::new(&mut pool, 0));
        let after_start = if let CableValue::Mono(v) = pool[1][0] { v } else { panic!(); };

        pool[0][0] = CableValue::Mono(target_voct);
        g.process(&mut CablePool::new(&mut pool, 1));
        let after_step = if let CableValue::Mono(v) = pool[1][1] { v } else { panic!(); };

        assert!(
            after_step < target_voct,
            "expected output {after_step} to be below target {target_voct} (glide should smooth)"
        );
        assert!(
            after_step > after_start,
            "expected output {after_step} to have increased from {after_start}"
        );
    }

    #[test]
    fn zero_glide_ms_tracks_instantly() {
        let mut g = make_glide_sr(0.0, 44100.0);
        set_ports_for_test(&mut g);
        let target_voct = 2.0_f64;
        let mut pool = make_pool(2);
        pool[0][1] = CableValue::Mono(target_voct);
        g.process(&mut CablePool::new(&mut pool, 0));
        if let CableValue::Mono(v) = pool[1][0] {
            assert!(
                (v - target_voct).abs() < 1e-9,
                "expected instant tracking, got {}", v
            );
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn c2_voct_zero_is_not_held() {
        let mut g = make_glide_sr(0.0, 44100.0);
        set_ports_for_test(&mut g);
        let mut pool = make_pool(2);
        // Prime at C3 = 1.0 V/OCT.
        pool[0][1] = CableValue::Mono(1.0);
        g.process(&mut CablePool::new(&mut pool, 0));
        if let CableValue::Mono(v) = pool[1][0] {
            assert!((v - 1.0).abs() < 1e-9);
        }
        // Now target C2 = 0.0 V/OCT.
        pool[0][0] = CableValue::Mono(0.0);
        g.process(&mut CablePool::new(&mut pool, 1));
        if let CableValue::Mono(v) = pool[1][1] {
            assert!(
                v.abs() < 1e-9,
                "C2 (0.0 V/OCT) must not be ignored; got {}", v
            );
        } else { panic!("expected Mono"); }
    }

}
