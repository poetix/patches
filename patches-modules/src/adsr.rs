use patches_core::{
    AudioEnvironment, CableValue, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, ModuleShape, OutputPort, ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::{ParameterMap, ParameterValue};

#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// An ADSR envelope generator.
///
/// Input ports:
///   inputs[0] — trigger (rising edge starts Attack)
///   inputs[1] — gate    (held high keeps Sustain; releasing transitions to Release)
///
/// Output ports:
///   outputs[0] — out (envelope level, always in [0.0, 1.0])
pub struct Adsr {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    // Parameters (set via update_validated_parameters)
    attack_secs: f64,
    decay_secs: f64,
    sustain: f64,
    release_secs: f64,
    // Stored sample rate (set in prepare)
    sample_rate: f64,
    // Per-sample increments (recomputed in update_validated_parameters)
    attack_inc: f64,
    decay_inc: f64,
    release_inc: f64,
    // Runtime state
    stage: Stage,
    level: f64,
    prev_trigger: f64,
    // Port fields
    in_trigger: MonoInput,
    in_gate: MonoInput,
    out_env: MonoOutput,
}

impl Module for Adsr {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Adsr",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "trigger", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "gate",    index: 0, kind: CableKind::Mono },
            ],
            outputs: vec![
                PortDescriptor { name: "out", index: 0, kind: CableKind::Mono },
            ],
            parameters: vec![
                ParameterDescriptor {
                    name: "attack",
                    index: 0,
                    parameter_type: ParameterKind::Float { min: 0.001, max: 10.0, default: 0.01 },
                },
                ParameterDescriptor {
                    name: "decay",
                    index: 0,
                    parameter_type: ParameterKind::Float { min: 0.001, max: 10.0, default: 0.1 },
                },
                ParameterDescriptor {
                    name: "sustain",
                    index: 0,
                    parameter_type: ParameterKind::Float { min: 0.0, max: 1.0, default: 0.7 },
                },
                ParameterDescriptor {
                    name: "release",
                    index: 0,
                    parameter_type: ParameterKind::Float { min: 0.001, max: 10.0, default: 0.3 },
                },
            ],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            attack_secs: 0.0,
            decay_secs: 0.0,
            sustain: 0.0,
            release_secs: 0.0,
            sample_rate: audio_environment.sample_rate,
            attack_inc: 0.0,
            decay_inc: 0.0,
            release_inc: 0.0,
            stage: Stage::Idle,
            level: 0.0,
            prev_trigger: 0.0,
            in_trigger: MonoInput::default(),
            in_gate: MonoInput::default(),
            out_env: MonoOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("attack") {
            self.attack_secs = *v;
            self.attack_inc = 1.0 / (self.attack_secs * self.sample_rate);
        }
        if let Some(ParameterValue::Float(v)) = params.get("decay") {
            self.decay_secs = *v;
            self.decay_inc = (1.0 - self.sustain) / (self.decay_secs * self.sample_rate);
        }
        if let Some(ParameterValue::Float(v)) = params.get("sustain") {
            self.sustain = *v;
            // Recompute decay_inc since it depends on sustain
            if self.decay_secs > 0.0 {
                self.decay_inc = (1.0 - self.sustain) / (self.decay_secs * self.sample_rate);
            }
        }
        if let Some(ParameterValue::Float(v)) = params.get("release") {
            self.release_secs = *v;
            // release_inc is recalculated on entry to Release using the current level
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_trigger = MonoInput::from_ports(inputs, 0);
        self.in_gate = MonoInput::from_ports(inputs, 1);
        self.out_env = MonoOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut [[CableValue; 2]], wi: usize) {
        let ri = 1 - wi;
        let trigger = self.in_trigger.read_from(pool, ri);
        let gate = self.in_gate.read_from(pool, ri);

        let trigger_rose = trigger >= 0.5 && self.prev_trigger < 0.5;
        self.prev_trigger = trigger;

        // Rising trigger: restart Attack from any state and current level
        if trigger_rose {
            self.stage = Stage::Attack;
        }

        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.level += self.attack_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level -= self.decay_inc;
                if self.level <= self.sustain {
                    self.level = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {
                self.level = self.sustain;
                if gate < 0.5 {
                    // Recalculate release slope from current level and begin immediately
                    self.release_inc = self.level / (self.release_secs * self.sample_rate);
                    self.level -= self.release_inc;
                    if self.level <= 0.0 {
                        self.level = 0.0;
                        self.stage = Stage::Idle;
                    } else {
                        self.stage = Stage::Release;
                    }
                }
            }
            Stage::Release => {
                self.level -= self.release_inc;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }

        self.out_env.write_to(pool, wi, self.level.clamp(0.0, 1.0));
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

    fn make_envelope(attack: f64, decay: f64, sustain: f64, release: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("attack".into(),  ParameterValue::Float(attack));
        params.insert("decay".into(),   ParameterValue::Float(decay));
        params.insert("sustain".into(), ParameterValue::Float(sustain));
        params.insert("release".into(), ParameterValue::Float(release));
        let mut r = Registry::new();
        r.register::<Adsr>();
        r.create(
            "Adsr",
            &AudioEnvironment { sample_rate: 10.0 },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

    fn set_ports_for_test(module: &mut Box<dyn Module>) {
        // 0=trigger, 1=gate, 2=out
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: true }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 2, connected: true }),
        ];
        module.set_ports(&inputs, &outputs);
    }

    fn tick(env: &mut dyn Module, trigger: f64, gate: f64, pool: &mut Vec<[CableValue; 2]>, tick_count: usize) -> f64 {
        let wi = tick_count % 2;
        let ri = 1 - wi;
        pool[0][ri] = CableValue::Mono(trigger);
        pool[1][ri] = CableValue::Mono(gate);
        env.process(pool, wi);
        if let CableValue::Mono(v) = pool[2][wi] { v } else { panic!("expected Mono"); }
    }

    #[test]
    fn idle_output_is_zero() {
        let mut adsr = make_envelope(0.5, 0.5, 0.5, 0.5);
        set_ports_for_test(&mut adsr);
        let mut pool = make_pool(3);
        assert_eq!(tick(adsr.as_mut(), 0.0, 0.0, &mut pool, 0), 0.0);
        assert_eq!(tick(adsr.as_mut(), 0.0, 0.0, &mut pool, 1), 0.0);
    }

    #[test]
    fn attack_rises_linearly_to_one() {
        // attack=0.5s at 10Hz → 5 samples, inc=0.2
        let mut adsr = make_envelope(0.5, 1.0, 0.5, 0.5);
        set_ports_for_test(&mut adsr);
        let mut pool = make_pool(3);

        let v0 = tick(adsr.as_mut(), 1.0, 1.0, &mut pool, 0);
        assert!((v0 - 0.2).abs() < 1e-12, "expected 0.2, got {v0}");

        let v1 = tick(adsr.as_mut(), 0.0, 1.0, &mut pool, 1);
        assert!((v1 - 0.4).abs() < 1e-12, "expected 0.4, got {v1}");

        let v2 = tick(adsr.as_mut(), 0.0, 1.0, &mut pool, 2);
        assert!((v2 - 0.6).abs() < 1e-12, "expected 0.6, got {v2}");

        let v3 = tick(adsr.as_mut(), 0.0, 1.0, &mut pool, 3);
        assert!((v3 - 0.8).abs() < 1e-12, "expected 0.8, got {v3}");

        let v4 = tick(adsr.as_mut(), 0.0, 1.0, &mut pool, 4);
        assert!((v4 - 1.0).abs() < 1e-12, "expected 1.0, got {v4}");
    }

    #[test]
    fn decay_falls_to_sustain() {
        // attack=0.1s (1 sample), decay=0.5s (5 samples), sustain=0.5
        // decay_inc = (1.0 - 0.5) / (0.5 * 10) = 0.5/5 = 0.1
        let mut adsr = make_envelope(0.1, 0.5, 0.5, 1.0);
        set_ports_for_test(&mut adsr);
        let mut pool = make_pool(3);

        let v_attack = tick(adsr.as_mut(), 1.0, 1.0, &mut pool, 0);
        assert!((v_attack - 1.0).abs() < 1e-12, "attack should reach 1.0, got {v_attack}");

        let expected = [0.9, 0.8, 0.7, 0.6, 0.5];
        for (i, &exp) in expected.iter().enumerate() {
            let v = tick(adsr.as_mut(), 0.0, 1.0, &mut pool, 1 + i);
            assert!(
                (v - exp).abs() < 1e-12,
                "decay sample {i}: expected {exp}, got {v}"
            );
        }

        let v_sus = tick(adsr.as_mut(), 0.0, 1.0, &mut pool, 6);
        assert!((v_sus - 0.5).abs() < 1e-12, "sustain holds at 0.5, got {v_sus}");
    }

    #[test]
    fn sustain_holds_while_gate_high() {
        let mut adsr = make_envelope(0.1, 0.1, 0.6, 1.0);
        set_ports_for_test(&mut adsr);
        let mut pool = make_pool(3);

        tick(adsr.as_mut(), 1.0, 1.0, &mut pool, 0);
        tick(adsr.as_mut(), 0.0, 1.0, &mut pool, 1);

        for i in 0..5 {
            let v = tick(adsr.as_mut(), 0.0, 1.0, &mut pool, 2 + i);
            assert!((v - 0.6).abs() < 1e-12, "sustain should hold at 0.6, got {v}");
        }
    }

    #[test]
    fn release_falls_to_zero() {
        // attack=0.1s, decay=0.1s, sustain=0.5, release=0.5s (5 samples)
        // release_inc = 0.5 / (0.5 * 10) = 0.1
        let mut adsr = make_envelope(0.1, 0.1, 0.5, 0.5);
        set_ports_for_test(&mut adsr);
        let mut pool = make_pool(3);

        tick(adsr.as_mut(), 1.0, 1.0, &mut pool, 0);
        tick(adsr.as_mut(), 0.0, 1.0, &mut pool, 1);

        let r0 = tick(adsr.as_mut(), 0.0, 0.0, &mut pool, 2);
        assert!((r0 - 0.4).abs() < 1e-12, "release[0]: expected 0.4, got {r0}");

        let r1 = tick(adsr.as_mut(), 0.0, 0.0, &mut pool, 3);
        assert!((r1 - 0.3).abs() < 1e-12, "release[1]: expected 0.3, got {r1}");

        let r2 = tick(adsr.as_mut(), 0.0, 0.0, &mut pool, 4);
        assert!((r2 - 0.2).abs() < 1e-12, "release[2]: expected 0.2, got {r2}");

        let r3 = tick(adsr.as_mut(), 0.0, 0.0, &mut pool, 5);
        assert!((r3 - 0.1).abs() < 1e-12, "release[3]: expected 0.1, got {r3}");

        let r4 = tick(adsr.as_mut(), 0.0, 0.0, &mut pool, 6);
        assert!((r4 - 0.0).abs() < 1e-12, "release[4]: expected 0.0, got {r4}");

        let after = tick(adsr.as_mut(), 0.0, 0.0, &mut pool, 7);
        assert_eq!(after, 0.0, "idle after release");
    }

    #[test]
    fn retrigger_mid_release_restarts_attack() {
        let mut adsr = make_envelope(0.1, 0.1, 0.5, 0.5);
        set_ports_for_test(&mut adsr);
        let mut pool = make_pool(3);

        tick(adsr.as_mut(), 1.0, 1.0, &mut pool, 0);
        tick(adsr.as_mut(), 0.0, 1.0, &mut pool, 1);
        tick(adsr.as_mut(), 0.0, 0.0, &mut pool, 2);
        tick(adsr.as_mut(), 0.0, 0.0, &mut pool, 3);

        let v = tick(adsr.as_mut(), 1.0, 1.0, &mut pool, 4);
        assert!((v - 1.0).abs() < 1e-12, "retrigger should reach 1.0, got {v}");
    }

    #[test]
    fn output_clamped_to_unit_range() {
        let mut adsr = make_envelope(0.1, 0.1, 0.5, 0.5);
        set_ports_for_test(&mut adsr);
        let mut pool = make_pool(3);

        for i in 0..20 {
            let v = tick(adsr.as_mut(), 1.0, 1.0, &mut pool, i);
            assert!((0.0..=1.0).contains(&v), "output out of range: {v}");
        }
    }
}
