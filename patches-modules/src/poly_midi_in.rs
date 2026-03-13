use patches_core::{
    AudioEnvironment, CablePool, InputPort, InstanceId, MidiEvent, Module, ModuleDescriptor,
    ModuleShape, MonoOutput, OutputPort, PolyOutput, PortDescriptor, ReceivesMidi,
};
use patches_core::CableKind;
use patches_core::parameter_map::ParameterMap;

const VOCT_SCALING: f64 = 1.0 / 12.0;

#[derive(Clone, Copy)]
struct Voice {
    note: u8,
    active: bool,
    /// Tick counter when this voice was last allocated; used for LIFO steal ordering.
    allocation_tick: u64,
    trigger_armed: bool,
}

impl Voice {
    const fn idle() -> Self {
        Self { note: 0, active: false, allocation_tick: 0, trigger_armed: false }
    }
}

/// Polyphonic MIDI-to-CV converter with LIFO note stealing.
///
/// Maintains a pool of `poly_voices` voices (from [`AudioEnvironment`]). When a new
/// note-on arrives and all voices are occupied the most-recently-allocated voice is
/// stolen (LIFO). Releasing a note deactivates the corresponding voice.
///
/// ## Output ports
///
/// | Index | Name      | Kind | Signal                                                |
/// |-------|-----------|------|-------------------------------------------------------|
/// | 0     | `v_oct`   | Poly | V/oct pitch per voice (MIDI 0 = 0 V, 1/12 V/semitone)|
/// | 1     | `trigger` | Poly | 1.0 for one sample after each note-on, then 0.0       |
/// | 2     | `gate`    | Poly | 1.0 while the note for that voice is physically held  |
/// | 3     | `mod`     | Mono | CC 1 (mod wheel) normalised to [0.0, 1.0]             |
/// | 4     | `pitch`   | Mono | Pitchbend normalised to [-1.0, 1.0]                   |
pub struct PolyMidiIn {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    voice_count: usize,
    voices: [Voice; 16],
    /// Incremented each `process` call; used to timestamp voice allocations.
    tick_count: u64,
    mod_value: f64,
    pitch_value: f64,
    // Output port fields
    out_v_oct: PolyOutput,
    out_trigger: PolyOutput,
    out_gate: PolyOutput,
    out_mod: MonoOutput,
    out_pitch: MonoOutput,
}

impl PolyMidiIn {
    /// Find a free voice, or steal the most-recently-allocated one (LIFO).
    fn find_or_steal_voice(&self) -> usize {
        for i in 0..self.voice_count {
            if !self.voices[i].active {
                return i;
            }
        }
        // All voices active — steal the one allocated most recently.
        let mut steal_idx = 0;
        let mut max_tick = 0u64;
        for i in 0..self.voice_count {
            if self.voices[i].allocation_tick >= max_tick {
                max_tick = self.voices[i].allocation_tick;
                steal_idx = i;
            }
        }
        steal_idx
    }
}

impl Module for PolyMidiIn {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "PolyMidiIn",
            shape: shape.clone(),
            inputs: vec![],
            outputs: vec![
                PortDescriptor { name: "v_oct",   index: 0, kind: CableKind::Poly },
                PortDescriptor { name: "trigger", index: 0, kind: CableKind::Poly },
                PortDescriptor { name: "gate",    index: 0, kind: CableKind::Poly },
                PortDescriptor { name: "mod",     index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "pitch",   index: 0, kind: CableKind::Mono },
            ],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            voice_count: audio_environment.poly_voices.min(16),
            voices: [Voice::idle(); 16],
            tick_count: 0,
            mod_value: 0.0,
            pitch_value: 0.0,
            out_v_oct: PolyOutput::default(),
            out_trigger: PolyOutput::default(),
            out_gate: PolyOutput::default(),
            out_mod: MonoOutput::default(),
            out_pitch: MonoOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

    fn descriptor(&self) -> &ModuleDescriptor { &self.descriptor }

    fn instance_id(&self) -> InstanceId { self.instance_id }

    fn set_ports(&mut self, _inputs: &[InputPort], outputs: &[OutputPort]) {
        self.out_v_oct   = PolyOutput::from_ports(outputs, 0);
        self.out_trigger = PolyOutput::from_ports(outputs, 1);
        self.out_gate    = PolyOutput::from_ports(outputs, 2);
        self.out_mod     = MonoOutput::from_ports(outputs, 3);
        self.out_pitch   = MonoOutput::from_ports(outputs, 4);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        let mut v_oct   = [0.0f64; 16];
        let mut trigger = [0.0f64; 16];
        let mut gate    = [0.0f64; 16];

        for i in 0..self.voice_count {
            let v = &mut self.voices[i];
            v_oct[i] = v.note as f64 * VOCT_SCALING;
            if v.trigger_armed {
                v.trigger_armed = false;
                trigger[i] = 1.0;
            }
            if v.active {
                gate[i] = 1.0;
            }
        }

        pool.write_poly(&self.out_v_oct,   v_oct);
        pool.write_poly(&self.out_trigger, trigger);
        pool.write_poly(&self.out_gate,    gate);
        pool.write_mono(&self.out_mod,     self.mod_value);
        pool.write_mono(&self.out_pitch,   self.pitch_value);

        self.tick_count += 1;
    }

    fn as_any(&self) -> &dyn std::any::Any { self }

    fn as_midi_receiver(&mut self) -> Option<&mut dyn ReceivesMidi> { Some(self) }
}

impl ReceivesMidi for PolyMidiIn {
    fn receive_midi(&mut self, event: MidiEvent) {
        let status = event.bytes[0] & 0xF0;
        let b1 = event.bytes[1];
        let b2 = event.bytes[2];

        match status {
            // Note On (velocity 0 treated as Note Off per MIDI spec)
            0x90 if b2 > 0 => {
                let idx = self.find_or_steal_voice();
                let v = &mut self.voices[idx];
                v.note = b1;
                v.active = true;
                v.allocation_tick = self.tick_count;
                v.trigger_armed = true;
            }
            // Note Off (or Note On velocity 0)
            0x80 | 0x90 => {
                for i in 0..self.voice_count {
                    if self.voices[i].active && self.voices[i].note == b1 {
                        self.voices[i].active = false;
                        break;
                    }
                }
            }
            // Control Change
            0xB0 => {
                if b1 == 1 {
                    self.mod_value = b2 as f64 / 127.0;
                }
            }
            // Pitch Bend: 14-bit value, LSB in b1, MSB in b2; centre = 8192
            0xE0 => {
                let raw = ((b2 as u16) << 7) | (b1 as u16);
                self.pitch_value = (raw as f64 - 8192.0) / 8192.0;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{AudioEnvironment, CablePool, CableValue, InstanceId, MidiEvent, Module, ModuleShape, Registry};

    fn make_kbd(poly_voices: usize) -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<PolyMidiIn>();
        r.create(
            "PolyMidiIn",
            &AudioEnvironment { sample_rate: 44100.0, poly_voices },
            &ModuleShape { channels: 0, length: 0 },
            &patches_core::parameter_map::ParameterMap::new(),
            InstanceId::next(),
        )
        .unwrap()
    }

    fn note_on(note: u8, vel: u8) -> MidiEvent { MidiEvent { bytes: [0x90, note, vel] } }
    fn note_off(note: u8) -> MidiEvent { MidiEvent { bytes: [0x80, note, 0] } }

    /// Make a ping-pong pool for 5 cables (3 poly + 2 mono) pre-filled with Poly zeros.
    fn make_pool() -> Vec<[CableValue; 2]> {
        let mut pool = vec![[CableValue::Poly([0.0; 16]); 2]; 3];
        pool.push([CableValue::Mono(0.0); 2]);
        pool.push([CableValue::Mono(0.0); 2]);
        pool
    }

    fn connect_all(m: &mut Box<dyn Module>) {
        use patches_core::{OutputPort, PolyOutput, MonoOutput};
        let outputs = vec![
            OutputPort::Poly(PolyOutput { cable_idx: 0, connected: true }),
            OutputPort::Poly(PolyOutput { cable_idx: 1, connected: true }),
            OutputPort::Poly(PolyOutput { cable_idx: 2, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 3, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: true }),
        ];
        m.set_ports(&[], &outputs);
    }

    fn read_poly_at(pool: &[[CableValue; 2]], cable: usize, wi: usize) -> [f64; 16] {
        match pool[cable][wi] {
            CableValue::Poly(v) => v,
            _ => panic!("expected Poly"),
        }
    }

    #[test]
    fn note_on_sets_v_oct_gate_trigger_for_voice_zero() {
        let mut m = make_kbd(4);
        connect_all(&mut m);
        let mut pool = make_pool();
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        m.process(&mut CablePool::new(&mut pool, 0));
        let v_oct   = read_poly_at(&pool, 0, 0);
        let trigger = read_poly_at(&pool, 1, 0);
        let gate    = read_poly_at(&pool, 2, 0);
        assert!((v_oct[0] - 5.0).abs() < 1e-10, "v_oct[0] should be 5.0 for note 60");
        assert_eq!(trigger[0], 1.0, "trigger[0] should fire");
        assert_eq!(gate[0],    1.0, "gate[0] should be high");
        // Other voices idle
        for i in 1..4 {
            assert_eq!(gate[i], 0.0, "voice {i} gate should be 0");
        }
    }

    #[test]
    fn trigger_clears_after_one_tick() {
        let mut m = make_kbd(4);
        connect_all(&mut m);
        let mut pool = make_pool();
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        m.process(&mut CablePool::new(&mut pool, 0)); // consume trigger
        m.process(&mut CablePool::new(&mut pool, 1));
        let trigger = read_poly_at(&pool, 1, 1);
        assert_eq!(trigger[0], 0.0, "trigger should clear after first tick");
    }

    #[test]
    fn two_notes_go_to_separate_voices() {
        let mut m = make_kbd(4);
        connect_all(&mut m);
        let mut pool = make_pool();
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        m.process(&mut CablePool::new(&mut pool, 0));
        m.as_midi_receiver().unwrap().receive_midi(note_on(64, 100));
        m.process(&mut CablePool::new(&mut pool, 1));
        let v_oct = read_poly_at(&pool, 0, 1);
        let gate  = read_poly_at(&pool, 2, 1);
        assert!((v_oct[0] - 5.0).abs() < 1e-10,       "voice 0: note 60");
        assert!((v_oct[1] - 64.0 / 12.0).abs() < 1e-10, "voice 1: note 64");
        assert_eq!(gate[0], 1.0, "voice 0 gate high");
        assert_eq!(gate[1], 1.0, "voice 1 gate high");
    }

    #[test]
    fn note_off_drops_gate_for_that_voice() {
        let mut m = make_kbd(4);
        connect_all(&mut m);
        let mut pool = make_pool();
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        m.process(&mut CablePool::new(&mut pool, 0));
        m.as_midi_receiver().unwrap().receive_midi(note_off(60));
        m.process(&mut CablePool::new(&mut pool, 1));
        let gate = read_poly_at(&pool, 2, 1);
        assert_eq!(gate[0], 0.0, "gate should drop after note-off");
    }

    #[test]
    fn lifo_steal_takes_most_recently_allocated() {
        // Fill 2 voices, then add a third — should steal voice 1 (most recent)
        let mut m = make_kbd(2);
        connect_all(&mut m);
        let mut pool = make_pool();

        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100)); // voice 0, tick 0
        m.process(&mut CablePool::new(&mut pool, 0));
        m.as_midi_receiver().unwrap().receive_midi(note_on(64, 100)); // voice 1, tick 1
        m.process(&mut CablePool::new(&mut pool, 1));

        // Both voices full — next note steals voice 1 (most recent, LIFO)
        m.as_midi_receiver().unwrap().receive_midi(note_on(67, 100));
        m.process(&mut CablePool::new(&mut pool, 0));
        let v_oct = read_poly_at(&pool, 0, 0);
        // voice 1 should now hold note 67
        assert!((v_oct[1] - 67.0 / 12.0).abs() < 1e-10,
            "LIFO steal: voice 1 should now carry note 67, got {}", v_oct[1]);
    }
}
