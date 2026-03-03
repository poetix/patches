use patches_core::{AudioEnvironment, ControlSignal, InstanceId, Module, ModuleDescriptor, PortDescriptor};

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
pub struct AdsrEnvelope {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    // Construction-time parameters
    attack_secs: f64,
    decay_secs: f64,
    sustain: f64,
    release_secs: f64,
    // Stored sample rate (set in initialise, needed to compute release_inc at runtime)
    sample_rate: f64,
    // Per-sample increments (computed in initialise or on stage entry)
    attack_inc: f64,
    decay_inc: f64,
    release_inc: f64,
    // Runtime state
    stage: Stage,
    level: f64,
    prev_trigger: f64,
}

impl AdsrEnvelope {
    pub fn new(attack_secs: f64, decay_secs: f64, sustain: f64, release_secs: f64) -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                inputs: vec![
                    PortDescriptor { name: "trigger", index: 0 },
                    PortDescriptor { name: "gate",    index: 0 },
                ],
                outputs: vec![
                    PortDescriptor { name: "out", index: 0 },
                ],
            },
            attack_secs,
            decay_secs,
            sustain,
            release_secs,
            sample_rate: 44100.0,
            attack_inc: 0.0,
            decay_inc: 0.0,
            release_inc: 0.0,
            stage: Stage::Idle,
            level: 0.0,
            prev_trigger: 0.0,
        }
    }
}

impl Module for AdsrEnvelope {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn initialise(&mut self, env: &AudioEnvironment) {
        self.sample_rate = env.sample_rate;
        self.attack_inc = 1.0 / (self.attack_secs * env.sample_rate);
        self.decay_inc = (1.0 - self.sustain) / (self.decay_secs * env.sample_rate);
        // release_inc is recalculated on entry to Release using the current level
    }

    fn receive_signal(&mut self, _signal: ControlSignal) {}

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        let trigger = inputs[0];
        let gate = inputs[1];

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

        outputs[0] = self.level.clamp(0.0, 1.0);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::AudioEnvironment;

    // Use a low sample rate to keep sample counts small.
    fn env() -> AudioEnvironment {
        AudioEnvironment { sample_rate: 10.0 }
    }

    fn tick(env: &mut AdsrEnvelope, trigger: f64, gate: f64) -> f64 {
        let mut out = [0.0f64];
        env.process(&[trigger, gate], &mut out);
        out[0]
    }

    #[test]
    fn idle_output_is_zero() {
        let mut adsr = AdsrEnvelope::new(0.5, 0.5, 0.5, 0.5);
        adsr.initialise(&env());
        assert_eq!(tick(&mut adsr, 0.0, 0.0), 0.0);
        assert_eq!(tick(&mut adsr, 0.0, 0.0), 0.0);
    }

    #[test]
    fn attack_rises_linearly_to_one() {
        // attack=0.5s at 10Hz → 5 samples, inc=0.2
        let mut adsr = AdsrEnvelope::new(0.5, 1.0, 0.5, 0.5);
        adsr.initialise(&env());

        // Trigger rising edge (gate held high throughout)
        let v0 = tick(&mut adsr, 1.0, 1.0); // sample 1: level = 0.0 + 0.2 = 0.2
        assert!((v0 - 0.2).abs() < 1e-12, "expected 0.2, got {v0}");

        let v1 = tick(&mut adsr, 0.0, 1.0); // 0.4
        assert!((v1 - 0.4).abs() < 1e-12, "expected 0.4, got {v1}");

        let v2 = tick(&mut adsr, 0.0, 1.0); // 0.6
        assert!((v2 - 0.6).abs() < 1e-12, "expected 0.6, got {v2}");

        let v3 = tick(&mut adsr, 0.0, 1.0); // 0.8
        assert!((v3 - 0.8).abs() < 1e-12, "expected 0.8, got {v3}");

        let v4 = tick(&mut adsr, 0.0, 1.0); // 1.0 → clamp, transitions to Decay
        assert!((v4 - 1.0).abs() < 1e-12, "expected 1.0, got {v4}");
    }

    #[test]
    fn decay_falls_to_sustain() {
        // attack=0.1s (1 sample), decay=0.5s (5 samples), sustain=0.5
        // decay_inc = (1.0 - 0.5) / (0.5 * 10) = 0.5/5 = 0.1
        let mut adsr = AdsrEnvelope::new(0.1, 0.5, 0.5, 1.0);
        adsr.initialise(&env());

        // Trigger → attack completes in 1 sample (inc=1.0)
        let v_attack = tick(&mut adsr, 1.0, 1.0);
        assert!((v_attack - 1.0).abs() < 1e-12, "attack should reach 1.0, got {v_attack}");

        // Decay: 5 steps from 1.0 down to 0.5
        let expected = [0.9, 0.8, 0.7, 0.6, 0.5];
        for (i, &exp) in expected.iter().enumerate() {
            let v = tick(&mut adsr, 0.0, 1.0);
            assert!(
                (v - exp).abs() < 1e-12,
                "decay sample {i}: expected {exp}, got {v}"
            );
        }

        // Now in Sustain — level holds
        let v_sus = tick(&mut adsr, 0.0, 1.0);
        assert!((v_sus - 0.5).abs() < 1e-12, "sustain holds at 0.5, got {v_sus}");
    }

    #[test]
    fn sustain_holds_while_gate_high() {
        // Fast attack (1 sample), fast decay (1 sample), sustain=0.6
        let mut adsr = AdsrEnvelope::new(0.1, 0.1, 0.6, 1.0);
        adsr.initialise(&env());

        tick(&mut adsr, 1.0, 1.0); // attack: 1.0
        tick(&mut adsr, 0.0, 1.0); // decay: 0.6

        // Several sustain samples
        for _ in 0..5 {
            let v = tick(&mut adsr, 0.0, 1.0);
            assert!((v - 0.6).abs() < 1e-12, "sustain should hold at 0.6, got {v}");
        }
    }

    #[test]
    fn release_falls_to_zero() {
        // attack=0.1s, decay=0.1s, sustain=0.5, release=0.5s (5 samples)
        // release_inc = 0.5 / (0.5 * 10) = 0.1
        let mut adsr = AdsrEnvelope::new(0.1, 0.1, 0.5, 0.5);
        adsr.initialise(&env());

        tick(&mut adsr, 1.0, 1.0); // attack → 1.0
        tick(&mut adsr, 0.0, 1.0); // decay → 0.5

        // Gate drops → Release
        let r0 = tick(&mut adsr, 0.0, 0.0); // enters release: 0.5 - 0.1 = 0.4
        assert!((r0 - 0.4).abs() < 1e-12, "release[0]: expected 0.4, got {r0}");

        let r1 = tick(&mut adsr, 0.0, 0.0); // 0.3
        assert!((r1 - 0.3).abs() < 1e-12, "release[1]: expected 0.3, got {r1}");

        let r2 = tick(&mut adsr, 0.0, 0.0); // 0.2
        assert!((r2 - 0.2).abs() < 1e-12, "release[2]: expected 0.2, got {r2}");

        let r3 = tick(&mut adsr, 0.0, 0.0); // 0.1
        assert!((r3 - 0.1).abs() < 1e-12, "release[3]: expected 0.1, got {r3}");

        let r4 = tick(&mut adsr, 0.0, 0.0); // 0.0 → Idle
        assert!((r4 - 0.0).abs() < 1e-12, "release[4]: expected 0.0, got {r4}");

        // Back to Idle
        let after = tick(&mut adsr, 0.0, 0.0);
        assert_eq!(after, 0.0, "idle after release");
    }

    #[test]
    fn retrigger_mid_release_restarts_attack() {
        let mut adsr = AdsrEnvelope::new(0.1, 0.1, 0.5, 0.5);
        adsr.initialise(&env());

        tick(&mut adsr, 1.0, 1.0); // attack → 1.0
        tick(&mut adsr, 0.0, 1.0); // decay → 0.5
        tick(&mut adsr, 0.0, 0.0); // release starts → 0.4
        tick(&mut adsr, 0.0, 0.0); // 0.3

        // Retrigger: should restart Attack from current level (0.3)
        let v = tick(&mut adsr, 1.0, 1.0); // attack_inc = 1.0, so 0.3 + 1.0 → clamped 1.0
        assert!((v - 1.0).abs() < 1e-12, "retrigger should reach 1.0, got {v}");
    }

    #[test]
    fn output_clamped_to_unit_range() {
        let mut adsr = AdsrEnvelope::new(0.1, 0.1, 0.5, 0.5);
        adsr.initialise(&env());

        for _ in 0..20 {
            let v = tick(&mut adsr, 1.0, 1.0);
            assert!(v >= 0.0 && v <= 1.0, "output out of range: {v}");
        }
    }

    #[test]
    fn descriptor_shape() {
        let m = AdsrEnvelope::new(0.1, 0.1, 0.5, 0.1);
        let desc = m.descriptor();
        assert_eq!(desc.inputs.len(), 2);
        assert_eq!(desc.outputs.len(), 1);
        assert_eq!(desc.inputs[0].name, "trigger");
        assert_eq!(desc.inputs[1].name, "gate");
        assert_eq!(desc.outputs[0].name, "out");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = AdsrEnvelope::new(0.1, 0.1, 0.5, 0.1);
        let b = AdsrEnvelope::new(0.1, 0.1, 0.5, 0.1);
        assert_ne!(a.instance_id(), b.instance_id());
    }
}
