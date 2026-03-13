use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, Module, ModuleDescriptor,
    ModuleShape, OutputPort, ParameterDescriptor, ParameterKind, PolyInput, PolyOutput, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::{ParameterMap, ParameterValue};

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Clone, Copy)]
struct AdsrVoice {
    stage: Stage,
    level: f64,
    prev_trigger: f64,
    /// Recomputed from the current level on entry to Release.
    release_inc: f64,
}

impl AdsrVoice {
    const fn idle() -> Self {
        Self { stage: Stage::Idle, level: 0.0, prev_trigger: 0.0, release_inc: 0.0 }
    }
}

/// Polyphonic ADSR envelope generator.
///
/// Maintains one envelope state machine per voice. Shared ADSR parameters apply to all
/// voices. Each voice is driven by its own trigger/gate channel from the poly inputs.
///
/// ## Input ports
/// | Index | Name      | Kind |
/// |-------|-----------|------|
/// | 0     | `trigger` | Poly |
/// | 1     | `gate`    | Poly |
///
/// ## Output ports
/// | Index | Name  | Kind |
/// |-------|-------|------|
/// | 0     | `out` | Poly |
pub struct PolyAdsr {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    voice_count: usize,
    voices: Vec<AdsrVoice>,
    // Shared parameters
    attack_inc: f64,
    decay_inc: f64,
    sustain: f64,
    release_secs: f64,
    sample_rate: f64,
    // Port fields
    in_trigger: PolyInput,
    in_gate: PolyInput,
    out_env: PolyOutput,
}

impl Module for PolyAdsr {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "PolyAdsr",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "trigger", index: 0, kind: CableKind::Poly },
                PortDescriptor { name: "gate",    index: 0, kind: CableKind::Poly },
            ],
            outputs: vec![
                PortDescriptor { name: "out", index: 0, kind: CableKind::Poly },
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
        let voice_count = audio_environment.poly_voices.min(16);
        Self {
            instance_id,
            descriptor,
            voice_count,
            voices: vec![AdsrVoice::idle(); voice_count],
            attack_inc: 0.0,
            decay_inc: 0.0,
            sustain: 0.0,
            release_secs: 0.0,
            sample_rate: audio_environment.sample_rate,
            in_trigger: PolyInput::default(),
            in_gate: PolyInput::default(),
            out_env: PolyOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("attack") {
            let secs = *v;
            self.attack_inc = 1.0 / (secs * self.sample_rate);
        }
        if let Some(ParameterValue::Float(v)) = params.get("decay") {
            let secs = *v;
            self.decay_inc = (1.0 - self.sustain) / (secs * self.sample_rate);
        }
        if let Some(ParameterValue::Float(v)) = params.get("sustain") {
            self.sustain = *v;
            // Recompute decay_inc since it depends on sustain.
            // decay_inc can only be recomputed if we know the decay time; re-read it.
            // We use a best-effort recompute: iterate params again for decay.
            if let Some(ParameterValue::Float(d)) = params.get("decay") {
                self.decay_inc = (1.0 - self.sustain) / (d * self.sample_rate);
            }
        }
        if let Some(ParameterValue::Float(v)) = params.get("release") {
            self.release_secs = *v;
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }
    fn instance_id(&self) -> InstanceId { self.instance_id }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_trigger = PolyInput::from_ports(inputs, 0);
        self.in_gate    = PolyInput::from_ports(inputs, 1);
        self.out_env    = PolyOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let trigger_arr = pool.read_poly(&self.in_trigger);
        let gate_arr    = pool.read_poly(&self.in_gate);

        let mut out = [0.0f64; 16];

        for i in 0..self.voice_count {
            let v = &mut self.voices[i];
            let trigger = trigger_arr[i];
            let gate    = gate_arr[i];

            let trigger_rose = trigger >= 0.5 && v.prev_trigger < 0.5;
            v.prev_trigger = trigger;

            if trigger_rose {
                v.stage = Stage::Attack;
            }

            match v.stage {
                Stage::Idle => {}
                Stage::Attack => {
                    v.level += self.attack_inc;
                    if v.level >= 1.0 {
                        v.level = 1.0;
                        v.stage = Stage::Decay;
                    }
                }
                Stage::Decay => {
                    v.level -= self.decay_inc;
                    if v.level <= self.sustain {
                        v.level = self.sustain;
                        v.stage = Stage::Sustain;
                    }
                }
                Stage::Sustain => {
                    v.level = self.sustain;
                    if gate < 0.5 {
                        v.release_inc = v.level / (self.release_secs * self.sample_rate);
                        v.level -= v.release_inc;
                        if v.level <= 0.0 {
                            v.level = 0.0;
                            v.stage = Stage::Idle;
                        } else {
                            v.stage = Stage::Release;
                        }
                    }
                }
                Stage::Release => {
                    v.level -= v.release_inc;
                    if v.level <= 0.0 {
                        v.level = 0.0;
                        v.stage = Stage::Idle;
                    }
                }
            }

            out[i] = v.level.clamp(0.0, 1.0);
        }

        pool.write_poly(&self.out_env, out);
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{AudioEnvironment, CablePool, CableValue, Module, ModuleShape, Registry};
    use patches_core::parameter_map::{ParameterMap, ParameterValue};

    fn make_adsr(attack: f64, decay: f64, sustain: f64, release: f64, voices: usize) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("attack".into(),  ParameterValue::Float(attack));
        params.insert("decay".into(),   ParameterValue::Float(decay));
        params.insert("sustain".into(), ParameterValue::Float(sustain));
        params.insert("release".into(), ParameterValue::Float(release));
        let mut r = Registry::new();
        r.register::<PolyAdsr>();
        r.create(
            "PolyAdsr",
            &AudioEnvironment { sample_rate: 10.0, poly_voices: voices },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        )
        .unwrap()
    }

    fn make_pool() -> Vec<[CableValue; 2]> {
        vec![
            [CableValue::Poly([0.0; 16]); 2], // 0: trigger
            [CableValue::Poly([0.0; 16]); 2], // 1: gate
            [CableValue::Poly([0.0; 16]); 2], // 2: out
        ]
    }

    fn set_ports_for_test(m: &mut Box<dyn Module>) {
        use patches_core::{InputPort, OutputPort, PolyInput, PolyOutput};
        let inputs = vec![
            InputPort::Poly(PolyInput { cable_idx: 0, scale: 1.0, connected: true }),
            InputPort::Poly(PolyInput { cable_idx: 1, scale: 1.0, connected: true }),
        ];
        let outputs = vec![
            OutputPort::Poly(PolyOutput { cable_idx: 2, connected: true }),
        ];
        m.set_ports(&inputs, &outputs);
    }

    fn tick_with(m: &mut dyn Module, trig: [f64; 16], gate: [f64; 16], pool: &mut Vec<[CableValue; 2]>, n: usize) -> [f64; 16] {
        let wi = n % 2;
        let ri = 1 - wi;
        pool[0][ri] = CableValue::Poly(trig);
        pool[1][ri] = CableValue::Poly(gate);
        m.process(&mut CablePool::new(pool, wi));
        match pool[2][wi] {
            CableValue::Poly(v) => v,
            _ => panic!("expected Poly"),
        }
    }

    fn arr(val: f64, voice: usize) -> [f64; 16] {
        let mut a = [0.0f64; 16];
        a[voice] = val;
        a
    }

    #[test]
    fn idle_output_is_zero() {
        let mut m = make_adsr(0.5, 0.5, 0.5, 0.5, 2);
        set_ports_for_test(&mut m);
        let mut pool = make_pool();
        let out = tick_with(m.as_mut(), [0.0; 16], [0.0; 16], &mut pool, 0);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 0.0);
    }

    #[test]
    fn attack_rises_on_trigger_for_single_voice() {
        // attack=0.5s at 10Hz → 5 samples, inc=0.2
        let mut m = make_adsr(0.5, 1.0, 0.5, 0.5, 2);
        set_ports_for_test(&mut m);
        let mut pool = make_pool();
        let out = tick_with(m.as_mut(), arr(1.0, 0), arr(1.0, 0), &mut pool, 0);
        assert!((out[0] - 0.2).abs() < 1e-12, "expected 0.2, got {}", out[0]);
        // Voice 1 not triggered
        assert_eq!(out[1], 0.0);
    }

    #[test]
    fn two_voices_are_independent() {
        // attack=0.1s (1 sample), decay=0.5s (5 samples), sustain=0.5
        let mut m = make_adsr(0.1, 0.5, 0.5, 0.5, 2);
        set_ports_for_test(&mut m);
        let mut pool = make_pool();

        // Trigger voice 0
        tick_with(m.as_mut(), arr(1.0, 0), arr(1.0, 0), &mut pool, 0);
        // Voice 0 in Decay; trigger voice 1
        let out = tick_with(m.as_mut(), arr(1.0, 1), arr(1.0, 1), &mut pool, 1);
        // Voice 0 should be decaying (1.0 - 0.1 = 0.9), voice 1 just started attack
        assert!((out[0] - 0.9).abs() < 1e-10, "voice 0 decay, expected 0.9 got {}", out[0]);
        assert!((out[1] - 1.0).abs() < 1e-10, "voice 1 attack, expected 1.0 got {}", out[1]);
    }
}
