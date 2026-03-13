use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, ModuleShape, OutputPort, ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::{ParameterMap, ParameterValue};

/// Offsets a V/OCT pitch signal by a fixed interval expressed as octaves,
/// semitones, and cents.
///
/// Output = input + octave + semitones/12 + cents/1200
///
/// All three parameters are independent and additive. Setting all to zero
/// passes the signal through unchanged.
pub struct Tuner {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    octave: i64,
    semi: i64,
    cent: i64,
    /// Precomputed offset in V/OCT: octave + semi/12 + cent/1200.
    offset: f64,
    // Port fields
    in_port: MonoInput,
    out_port: MonoOutput,
}

impl Tuner {
    fn recompute_offset(octave: i64, semi: i64, cent: i64) -> f64 {
        octave as f64 + semi as f64 / 12.0 + cent as f64 / 1200.0
    }
}

impl Module for Tuner {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Tuner",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "in", index: 0, kind: CableKind::Mono },
            ],
            outputs: vec![
                PortDescriptor { name: "out", index: 0, kind: CableKind::Mono },
            ],
            parameters: vec![
                ParameterDescriptor {
                    name: "octave",
                    index: 0,
                    parameter_type: ParameterKind::Int { min: -8, max: 8, default: 0 },
                },
                ParameterDescriptor {
                    name: "semi",
                    index: 0,
                    parameter_type: ParameterKind::Int { min: -12, max: 12, default: 0 },
                },
                ParameterDescriptor {
                    name: "cent",
                    index: 0,
                    parameter_type: ParameterKind::Int { min: -100, max: 100, default: 0 },
                },
            ],
            is_sink: false,
        }
    }

    fn prepare(_audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            octave: 0,
            semi: 0,
            cent: 0,
            offset: 0.0,
            in_port: MonoInput::default(),
            out_port: MonoOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Int(v)) = params.get("octave") { self.octave = *v; }
        if let Some(ParameterValue::Int(v)) = params.get("semi")   { self.semi = *v; }
        if let Some(ParameterValue::Int(v)) = params.get("cent")   { self.cent = *v; }
        self.offset = Self::recompute_offset(self.octave, self.semi, self.cent);
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
        let input = pool.read_mono(&self.in_port);
        pool.write_mono(&self.out_port, input + self.offset);
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

    fn make_tuner(octave: i64, semi: i64, cent: i64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("octave".into(), ParameterValue::Int(octave));
        params.insert("semi".into(),   ParameterValue::Int(semi));
        params.insert("cent".into(),   ParameterValue::Int(cent));
        let mut r = Registry::new();
        r.register::<Tuner>();
        r.create(
            "Tuner",
            &AudioEnvironment { sample_rate: 44100.0, poly_voices: 16 },
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
    fn zero_offsets_pass_through() {
        let mut t = make_tuner(0, 0, 0);
        set_ports_for_test(&mut t);
        let mut pool = make_pool(2);
        pool[0][1] = CableValue::Mono(3.0);
        t.process(&mut CablePool::new(&mut pool, 0));
        if let CableValue::Mono(v) = pool[1][0] {
            assert!((v - 3.0).abs() < 1e-12, "zero offset must pass through; got {}", v);
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn octave_offset_adds_integer() {
        let mut t = make_tuner(1, 0, 0);
        set_ports_for_test(&mut t);
        let mut pool = make_pool(2);
        pool[0][1] = CableValue::Mono(4.0);
        t.process(&mut CablePool::new(&mut pool, 0));
        if let CableValue::Mono(v) = pool[1][0] {
            assert!((v - 5.0).abs() < 1e-12, "octave=1 should add 1.0; got {}", v);
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn semitone_offset_adds_one_twelfth() {
        let mut t = make_tuner(0, 1, 0);
        set_ports_for_test(&mut t);
        let mut pool = make_pool(2);
        pool[0][1] = CableValue::Mono(4.0);
        t.process(&mut CablePool::new(&mut pool, 0));
        let expected = 4.0 + 1.0 / 12.0;
        if let CableValue::Mono(v) = pool[1][0] {
            assert!((v - expected).abs() < 1e-12, "semi=1 should add 1/12; got {}", v);
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn cent_offset_adds_one_twelfth_hundredth() {
        let mut t = make_tuner(0, 0, 100);
        set_ports_for_test(&mut t);
        let mut pool = make_pool(2);
        pool[0][1] = CableValue::Mono(4.0);
        t.process(&mut CablePool::new(&mut pool, 0));
        let expected = 4.0 + 100.0 / 1200.0;
        if let CableValue::Mono(v) = pool[1][0] {
            assert!((v - expected).abs() < 1e-12, "cent=100 should add 1/12; got {}", v);
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn combined_offsets_are_additive() {
        let mut t = make_tuner(-1, 3, -50);
        set_ports_for_test(&mut t);
        let mut pool = make_pool(2);
        pool[0][1] = CableValue::Mono(4.0);
        t.process(&mut CablePool::new(&mut pool, 0));
        let expected = 4.0 - 1.0 + 3.0 / 12.0 - 50.0 / 1200.0;
        if let CableValue::Mono(v) = pool[1][0] {
            assert!((v - expected).abs() < 1e-12, "combined offset mismatch; got {}", v);
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn partial_update_preserves_unchanged_params() {
        // Simulates the planner sending only the changed key on hot-reload.
        let mut t = make_tuner(1, 7, 12);
        set_ports_for_test(&mut t);
        // Now apply a partial update: only change cent.
        let mut partial = ParameterMap::new();
        partial.insert("cent".into(), ParameterValue::Int(0));
        t.update_validated_parameters(&partial);
        let mut pool = make_pool(2);
        pool[0][1] = CableValue::Mono(4.0);
        t.process(&mut CablePool::new(&mut pool, 0));
        // octave=1, semi=7, cent=0 — octave and semi must be retained from initial build.
        let expected = 4.0 + 1.0 + 7.0 / 12.0;
        if let CableValue::Mono(v) = pool[1][0] {
            assert!(
                (v - expected).abs() < 1e-12,
                "partial update must preserve octave and semi; got {}, expected {expected}",
                v
            );
        } else { panic!("expected Mono"); }
    }
}
