use patches_core::{
    AudioEnvironment, CablePool, CableValue, InputPort, InstanceId, MidiEvent, Module, ModuleDescriptor,
    ModuleShape, MonoOutput, OutputPort, PortDescriptor, ReceivesMidi,
};
use patches_core::CableKind;
use patches_core::parameter_map::ParameterMap;

/// Semitones per octave, used to convert MIDI note numbers to V/oct.
const VOCT_SCALING: f64 = 1.0 / 12.0;

/// Maximum number of simultaneously held keys tracked in the note stack.
/// Releasing a key pops back to the most recently pressed key still held.
const NOTE_STACK_SIZE: usize = 16;

/// Fixed-capacity stack of held MIDI note numbers, ordered oldest-to-newest.
///
/// All operations are O(n) on `NOTE_STACK_SIZE` with no heap allocation.
struct NoteStack {
    notes: [u8; NOTE_STACK_SIZE],
    count: usize,
}

impl NoteStack {
    const fn new() -> Self {
        Self { notes: [0; NOTE_STACK_SIZE], count: 0 }
    }

    /// Push `note` onto the top of the stack.
    ///
    /// If the note is already present it is moved to the top (re-press without
    /// release). If the stack is full the oldest note is evicted to make room.
    fn push(&mut self, note: u8) {
        // Remove any existing occurrence so we don't track it twice.
        self.remove(note);
        if self.count == NOTE_STACK_SIZE {
            // Evict the oldest note by shifting the entire stack left.
            self.notes.copy_within(1..NOTE_STACK_SIZE, 0);
            self.count -= 1;
        }
        self.notes[self.count] = note;
        self.count += 1;
    }

    /// Remove `note` from the stack (at any position). No-op if not present.
    fn remove(&mut self, note: u8) {
        if let Some(pos) = self.notes[..self.count].iter().position(|&n| n == note) {
            self.notes.copy_within(pos + 1..self.count, pos);
            self.count -= 1;
        }
    }

    /// The most recently pressed note still held, or `None` if empty.
    fn top(&self) -> Option<u8> {
        if self.count > 0 { Some(self.notes[self.count - 1]) } else { None }
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Translates MIDI note and controller messages from a monophonic keyboard
/// into CV-style outputs.
///
/// Uses a last-note-priority stack: pressing a new key updates pitch
/// immediately; releasing the top key falls back to the previously held key
/// (if any) without re-triggering.
///
/// ## Output ports
///
/// | Index | Name      | Signal                                                              |
/// |-------|-----------|---------------------------------------------------------------------|
/// | 0     | `v_oct`   | V/oct pitch; MIDI note 0 = C0 = 0 V, 1/12 V per semitone          |
/// | 1     | `trigger` | 1.0 for one sample after each note-on, then 0.0                    |
/// | 2     | `gate`    | 1.0 while any note is held or sustain (CC 64) is active            |
/// | 3     | `mod`     | CC 1 (mod wheel) normalised to [0.0, 1.0]                          |
/// | 4     | `pitch`   | Pitchbend normalised to [-1.0, 1.0]                                 |
pub struct MonoMidiIn {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,

    /// Stack of physically held keys, oldest at index 0, newest at top.
    stack: NoteStack,
    /// MIDI note number currently driving `v_oct`. Persists after all keys are
    /// released so the oscillator pitch does not snap to 0.
    current_note: u8,
    /// True while sustain pedal (CC 64) is depressed.
    sustain: bool,
    /// True during the one sample immediately after a note-on.
    trigger_armed: bool,
    /// Current mod wheel value normalised to [0.0, 1.0].
    mod_value: f64,
    /// Current pitchbend value normalised to [-1.0, 1.0].
    pitch_value: f64,
    // Output port fields
    out_v_oct: MonoOutput,
    out_trigger: MonoOutput,
    out_gate: MonoOutput,
    out_mod: MonoOutput,
    out_pitch: MonoOutput,
}

impl Module for MonoMidiIn {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "MidiIn",
            shape: shape.clone(),
            inputs: vec![],
            outputs: vec![
                PortDescriptor { name: "v_oct",   index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "trigger", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "gate",    index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "mod",     index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "pitch",   index: 0, kind: CableKind::Mono },
            ],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(
        _audio_environment: &AudioEnvironment,
        descriptor: ModuleDescriptor,
        instance_id: InstanceId,
    ) -> Self {
        Self {
            instance_id,
            descriptor,
            stack: NoteStack::new(),
            current_note: 60, // sensible middle-range default; overwritten on first note-on
            sustain: false,
            trigger_armed: false,
            mod_value: 0.0,
            pitch_value: 0.0,
            out_v_oct: MonoOutput::default(),
            out_trigger: MonoOutput::default(),
            out_gate: MonoOutput::default(),
            out_mod: MonoOutput::default(),
            out_pitch: MonoOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, _inputs: &[InputPort], outputs: &[OutputPort]) {
        self.out_v_oct = MonoOutput::from_ports(outputs, 0);
        self.out_trigger = MonoOutput::from_ports(outputs, 1);
        self.out_gate = MonoOutput::from_ports(outputs, 2);
        self.out_mod = MonoOutput::from_ports(outputs, 3);
        self.out_pitch = MonoOutput::from_ports(outputs, 4);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        pool.write_mono(&self.out_v_oct, self.current_note as f64 * VOCT_SCALING);

        let trigger_val = if self.trigger_armed {
            self.trigger_armed = false;
            1.0
        } else {
            0.0
        };
        pool.write_mono(&self.out_trigger, trigger_val);

        let gate_val = if !self.stack.is_empty() || self.sustain { 1.0 } else { 0.0 };
        pool.write_mono(&self.out_gate, gate_val);
        pool.write_mono(&self.out_mod, self.mod_value);
        pool.write_mono(&self.out_pitch, self.pitch_value);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_midi_receiver(&mut self) -> Option<&mut dyn ReceivesMidi> {
        Some(self)
    }
}

impl ReceivesMidi for MonoMidiIn {
    fn receive_midi(&mut self, event: MidiEvent) {
        let status = event.bytes[0] & 0xF0;
        let b1 = event.bytes[1];
        let b2 = event.bytes[2];

        match status {
            // Note On (velocity 0 treated as Note Off per MIDI spec)
            0x90 if b2 > 0 => {
                self.stack.push(b1);
                self.current_note = b1;
                self.trigger_armed = true;
            }
            // Note Off (or Note On with velocity 0)
            0x80 | 0x90 => {
                self.stack.remove(b1);
                // Fall back to the previously held key (if any), without retriggering.
                if let Some(prev) = self.stack.top() {
                    self.current_note = prev;
                }
                // If stack is now empty, current_note holds its last value so
                // the oscillator pitch does not jump to 0.
            }
            // Control Change
            0xB0 => match b1 {
                1 => {
                    self.mod_value = b2 as f64 / 127.0;
                }
                64 => {
                    // Sustain pedal: >= 64 = on
                    self.sustain = b2 >= 64;
                }
                _ => {}
            },
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
    use patches_core::{AudioEnvironment, CablePool, InstanceId, MidiEvent, Module, ModuleShape, Registry};

    fn make_keyboard() -> Box<dyn Module> {
        let mut r = Registry::new();
        r.register::<MonoMidiIn>();
        r.create(
            "MidiIn",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels: 0, length: 0 },
            &patches_core::parameter_map::ParameterMap::new(),
            InstanceId::next(),
        )
        .unwrap()
    }

    fn note_on(note: u8, vel: u8) -> MidiEvent {
        MidiEvent { bytes: [0x90, note, vel] }
    }

    fn note_off(note: u8) -> MidiEvent {
        MidiEvent { bytes: [0x80, note, 0] }
    }

    fn cc(number: u8, value: u8) -> MidiEvent {
        MidiEvent { bytes: [0xB0, number, value] }
    }

    fn pitch_bend(raw: u16) -> MidiEvent {
        // raw is 14-bit [0, 16383], centre 8192
        MidiEvent { bytes: [0xE0, (raw & 0x7F) as u8, ((raw >> 7) & 0x7F) as u8] }
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

    fn set_all_outputs_connected(module: &mut Box<dyn Module>) {
        // 5 outputs: 0=v_oct, 1=trigger, 2=gate, 3=mod, 4=pitch
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 0, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 1, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 2, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 3, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: true }),
        ];
        module.set_ports(&[], &outputs);
    }

    fn tick(m: &mut Box<dyn Module>, pool: &mut Vec<[CableValue; 2]>, tick_count: usize) -> [f64; 5] {
        let wi = tick_count % 2;
        m.process(&mut CablePool::new(pool, wi));
        let mut out = [0.0f64; 5];
        for (i, v) in out.iter_mut().enumerate() {
            if let CableValue::Mono(val) = pool[i][wi] { *v = val; }
        }
        out
    }

    // ── NoteStack unit tests ─────────────────────────────────────────────────

    #[test]
    fn stack_push_and_top() {
        let mut s = NoteStack::new();
        s.push(60);
        assert_eq!(s.top(), Some(60));
        s.push(64);
        assert_eq!(s.top(), Some(64));
    }

    #[test]
    fn stack_remove_top_reveals_previous() {
        let mut s = NoteStack::new();
        s.push(60);
        s.push(64);
        s.remove(64);
        assert_eq!(s.top(), Some(60));
    }

    #[test]
    fn stack_remove_middle_preserves_order() {
        let mut s = NoteStack::new();
        s.push(60);
        s.push(62);
        s.push(64);
        s.remove(62);
        assert_eq!(s.top(), Some(64));
        assert_eq!(s.count, 2);
        assert_eq!(s.notes[0], 60);
        assert_eq!(s.notes[1], 64);
    }

    #[test]
    fn stack_push_duplicate_moves_to_top() {
        let mut s = NoteStack::new();
        s.push(60);
        s.push(64);
        s.push(60); // re-press 60 while 64 is held
        assert_eq!(s.top(), Some(60));
        assert_eq!(s.count, 2);
    }

    #[test]
    fn stack_evicts_oldest_when_full() {
        let mut s = NoteStack::new();
        for i in 0..NOTE_STACK_SIZE as u8 {
            s.push(i);
        }
        assert_eq!(s.count, NOTE_STACK_SIZE);
        // Pushing one more evicts note 0
        s.push(100);
        assert_eq!(s.count, NOTE_STACK_SIZE);
        assert_eq!(s.top(), Some(100));
        assert!(!s.notes[..NOTE_STACK_SIZE].contains(&0));
    }

    // ── Module behaviour tests ───────────────────────────────────────────────

    #[test]
    fn descriptor_has_five_outputs_no_inputs() {
        let m = make_keyboard();
        let d = m.descriptor();
        assert_eq!(d.inputs.len(), 0);
        assert_eq!(d.outputs.len(), 5);
        assert_eq!(d.outputs[0].name, "v_oct");
        assert_eq!(d.outputs[1].name, "trigger");
        assert_eq!(d.outputs[2].name, "gate");
        assert_eq!(d.outputs[3].name, "mod");
        assert_eq!(d.outputs[4].name, "pitch");
    }

    #[test]
    fn note_on_sets_voct_gate_trigger() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        let out = tick(&mut m, &mut pool, 0);
        assert_eq!(out[0], 5.0,  "v_oct: note 60 should be 5.0");
        assert_eq!(out[1], 1.0,  "trigger should be high on first tick after note-on");
        assert_eq!(out[2], 1.0,  "gate should be high while note held");
    }

    #[test]
    fn trigger_clears_after_one_tick() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);
        m.as_midi_receiver().unwrap().receive_midi(note_on(69, 100));
        tick(&mut m, &mut pool, 0); // consume trigger
        let out = tick(&mut m, &mut pool, 1);
        assert_eq!(out[1], 0.0, "trigger should be 0 on the second tick");
        assert_eq!(out[2], 1.0, "gate should still be high");
    }

    #[test]
    fn voct_correct_for_various_notes() {
        let cases: &[(u8, f64)] = &[
            (0,  0.0),
            (12, 1.0),
            (60, 5.0),
            (69, 69.0 / 12.0),
            (1,  1.0 / 12.0),
        ];
        for &(note, expected) in cases {
            let mut m = make_keyboard();
            set_all_outputs_connected(&mut m);
            let mut pool = make_pool(5);
            m.as_midi_receiver().unwrap().receive_midi(note_on(note, 100));
            let out = tick(&mut m, &mut pool, 0);
            let diff = (out[0] - expected).abs();
            assert!(diff < 1e-10, "note {note}: expected v_oct {expected}, got {}", out[0]);
        }
    }

    #[test]
    fn note_off_drops_gate_when_no_sustain() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        tick(&mut m, &mut pool, 0);
        m.as_midi_receiver().unwrap().receive_midi(note_off(60));
        let out = tick(&mut m, &mut pool, 1);
        assert_eq!(out[2], 0.0, "gate should drop after note-off with no sustain");
    }

    #[test]
    fn releasing_top_note_falls_back_to_previous_note() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        tick(&mut m, &mut pool, 0);
        m.as_midi_receiver().unwrap().receive_midi(note_on(64, 100));
        tick(&mut m, &mut pool, 1);

        m.as_midi_receiver().unwrap().receive_midi(note_off(64));
        let out = tick(&mut m, &mut pool, 2);
        assert_eq!(out[2], 1.0,  "gate should stay high (60 is still held)");
        assert_eq!(out[0], 5.0,  "v_oct should revert to note 60 (5.0 V)");
        assert_eq!(out[1], 0.0,  "no trigger on fallback");
    }

    #[test]
    fn releasing_non_top_note_does_not_change_pitch() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        m.as_midi_receiver().unwrap().receive_midi(note_on(64, 100));
        tick(&mut m, &mut pool, 0);

        m.as_midi_receiver().unwrap().receive_midi(note_off(60));
        let out = tick(&mut m, &mut pool, 1);
        assert_eq!(out[2], 1.0,                "gate stays high");
        assert_eq!(out[0], 64.0 * VOCT_SCALING, "v_oct stays at 64");
    }

    #[test]
    fn sustain_holds_gate_after_note_off() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);
        m.as_midi_receiver().unwrap().receive_midi(cc(64, 127)); // sustain on
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        tick(&mut m, &mut pool, 0);
        m.as_midi_receiver().unwrap().receive_midi(note_off(60));
        let out = tick(&mut m, &mut pool, 1);
        assert_eq!(out[2], 1.0, "gate should remain high while sustain is active");
    }

    #[test]
    fn sustain_release_drops_gate_when_no_note_held() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);
        m.as_midi_receiver().unwrap().receive_midi(cc(64, 127)); // sustain on
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        tick(&mut m, &mut pool, 0);
        m.as_midi_receiver().unwrap().receive_midi(note_off(60));
        m.as_midi_receiver().unwrap().receive_midi(cc(64, 0)); // sustain off
        let out = tick(&mut m, &mut pool, 1);
        assert_eq!(out[2], 0.0, "gate should drop when sustain released with no note held");
    }

    #[test]
    fn mod_wheel_updates_mod_output() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);
        m.as_midi_receiver().unwrap().receive_midi(cc(1, 127));
        let out = tick(&mut m, &mut pool, 0);
        assert_eq!(out[3], 1.0, "mod at CC 127 should be 1.0");

        m.as_midi_receiver().unwrap().receive_midi(cc(1, 0));
        let out = tick(&mut m, &mut pool, 1);
        assert_eq!(out[3], 0.0, "mod at CC 0 should be 0.0");

        m.as_midi_receiver().unwrap().receive_midi(cc(1, 64));
        let out = tick(&mut m, &mut pool, 2);
        let expected = 64.0 / 127.0;
        let diff = (out[3] - expected).abs();
        assert!(diff < 1e-10, "mod at CC 64 should be {expected}, got {}", out[3]);
    }

    #[test]
    fn pitchbend_normalises_correctly() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);

        m.as_midi_receiver().unwrap().receive_midi(pitch_bend(8192));
        let out = tick(&mut m, &mut pool, 0);
        assert_eq!(out[4], 0.0, "pitchbend centre should be 0.0");

        m.as_midi_receiver().unwrap().receive_midi(pitch_bend(16383));
        let out = tick(&mut m, &mut pool, 1);
        let expected = (16383.0 - 8192.0) / 8192.0;
        let diff = (out[4] - expected).abs();
        assert!(diff < 1e-10, "pitchbend full-up should be ~1.0, got {}", out[4]);

        m.as_midi_receiver().unwrap().receive_midi(pitch_bend(0));
        let out = tick(&mut m, &mut pool, 2);
        assert_eq!(out[4], -1.0, "pitchbend full-down should be -1.0");
    }

    #[test]
    fn unknown_cc_is_ignored() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);
        m.as_midi_receiver().unwrap().receive_midi(cc(7, 100));
        let out = tick(&mut m, &mut pool, 0);
        assert_eq!(out[3], 0.0, "unknown CC should not affect mod output");
    }

    #[test]
    fn note_on_velocity_zero_treated_as_note_off() {
        let mut m = make_keyboard();
        set_all_outputs_connected(&mut m);
        let mut pool = make_pool(5);
        m.as_midi_receiver().unwrap().receive_midi(note_on(60, 100));
        tick(&mut m, &mut pool, 0);
        m.as_midi_receiver().unwrap().receive_midi(MidiEvent { bytes: [0x90, 60, 0] });
        let out = tick(&mut m, &mut pool, 1);
        assert_eq!(out[2], 0.0, "NoteOn vel=0 should drop gate");
        assert_eq!(out[1], 0.0, "NoteOn vel=0 should not fire trigger");
    }
}
