use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor,
    ModuleShape, ParameterDescriptor, ParameterKind, PortDescriptor,
    PortConnectivity,
};
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
                PortDescriptor { name: "in", index: 0 },
            ],
            outputs: vec![
                PortDescriptor { name: "out", index: 0 },
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
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Int(v)) = params.get("octave") { self.octave = *v; }
        if let Some(ParameterValue::Int(v)) = params.get("semi")   { self.semi = *v; }
        if let Some(ParameterValue::Int(v)) = params.get("cent")   { self.cent = *v; }
        self.offset = Self::recompute_offset(self.octave, self.semi, self.cent);
    }

    fn set_connectivity(&mut self, _connectivity: PortConnectivity) {}

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        outputs[0] = inputs[0] + self.offset;
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

    fn make_tuner(octave: i64, semi: i64, cent: i64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("octave".into(), ParameterValue::Int(octave));
        params.insert("semi".into(),   ParameterValue::Int(semi));
        params.insert("cent".into(),   ParameterValue::Int(cent));
        let mut r = Registry::new();
        r.register::<Tuner>();
        r.create(
            "Tuner",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap()
    }

    #[test]
    fn descriptor_shape() {
        let t = make_tuner(0, 0, 0);
        let desc = t.descriptor();
        assert_eq!(desc.module_name, "Tuner");
        assert_eq!(desc.inputs.len(), 1);
        assert_eq!(desc.inputs[0].name, "in");
        assert_eq!(desc.outputs.len(), 1);
        assert_eq!(desc.outputs[0].name, "out");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_tuner(0, 0, 0);
        let b = make_tuner(0, 0, 0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn zero_offsets_pass_through() {
        let mut t = make_tuner(0, 0, 0);
        let mut out = [0.0_f64];
        t.process(&[3.0], &mut out); // C4 voct
        assert!((out[0] - 3.0).abs() < 1e-12, "zero offset must pass through; got {}", out[0]);
    }

    #[test]
    fn octave_offset_adds_integer() {
        let mut t = make_tuner(1, 0, 0);
        let mut out = [0.0_f64];
        t.process(&[4.0], &mut out); // C4 → C5
        assert!((out[0] - 5.0).abs() < 1e-12, "octave=1 should add 1.0; got {}", out[0]);
    }

    #[test]
    fn semitone_offset_adds_one_twelfth() {
        let mut t = make_tuner(0, 1, 0);
        let mut out = [0.0_f64];
        t.process(&[4.0], &mut out);
        let expected = 4.0 + 1.0 / 12.0;
        assert!((out[0] - expected).abs() < 1e-12, "semi=1 should add 1/12; got {}", out[0]);
    }

    #[test]
    fn cent_offset_adds_one_twelfth_hundredth() {
        let mut t = make_tuner(0, 0, 100);
        let mut out = [0.0_f64];
        t.process(&[4.0], &mut out);
        let expected = 4.0 + 100.0 / 1200.0; // 100 cents = 1 semitone
        assert!((out[0] - expected).abs() < 1e-12, "cent=100 should add 1/12; got {}", out[0]);
    }

    #[test]
    fn combined_offsets_are_additive() {
        let mut t = make_tuner(-1, 3, -50);
        let mut out = [0.0_f64];
        t.process(&[4.0], &mut out);
        let expected = 4.0 - 1.0 + 3.0 / 12.0 - 50.0 / 1200.0;
        assert!((out[0] - expected).abs() < 1e-12, "combined offset mismatch; got {}", out[0]);
    }

    #[test]
    fn partial_update_preserves_unchanged_params() {
        // Simulates the planner sending only the changed key on hot-reload.
        let mut t = make_tuner(1, 7, 12);
        // Now apply a partial update: only change cent.
        let mut partial = ParameterMap::new();
        partial.insert("cent".into(), ParameterValue::Int(0));
        t.update_validated_parameters(&partial);
        let mut out = [0.0_f64];
        t.process(&[4.0], &mut out);
        // octave=1, semi=7, cent=0 — octave and semi must be retained from initial build.
        let expected = 4.0 + 1.0 + 7.0 / 12.0;
        assert!(
            (out[0] - expected).abs() < 1e-12,
            "partial update must preserve octave and semi; got {}, expected {expected}",
            out[0]
        );
    }
}
