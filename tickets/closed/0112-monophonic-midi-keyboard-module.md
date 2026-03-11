---
id: "0112"
title: MonophonicMidiKeyboard module
priority: medium
created: 2026-03-11
epic: E021
depends_on: ["0111"]
---

## Summary

Add a `MonophonicMidiKeyboard` module to `patches-modules` that implements
`ReceivesMidi` and translates MIDI note and controller messages from a
monophonic keyboard into control-voltage-style outputs suitable for patching
into oscillators and envelope generators.

## Output ports

| Port | Signal | Description |
|------|--------|-------------|
| `v_oct` | V/oct pitch | MIDI note number converted to volts-per-octave (note 69 = A4 = 0 V; each semitone = 1/12 V) |
| `trigger` | Trigger pulse | Emits 1.0 for one block after a note-on, then returns to 0.0 |
| `gate` | Gate | 1.0 while any note is held or sustain pedal (CC 64) is active; 0.0 otherwise |
| `mod` | Modwheel | CC 1 value normalised to [0.0, 1.0] |
| `pitch` | Pitchwheel | Pitchbend value normalised to [-1.0, 1.0] (centre = 0.0) |

## Acceptance criteria

- [ ] `MonophonicMidiKeyboard` in `patches-modules` implements both `Module`
      and `ReceivesMidi`.
- [ ] Five output ports in the order listed above; no input ports.
- [ ] `receive_midi` handles:
  - `NoteOn` (velocity > 0): record pitch, set gate high, arm trigger.
  - `NoteOn` with velocity 0 (treated as NoteOff): clear held note; gate drops
    unless sustain is active.
  - `NoteOff`: same as NoteOn-vel-0.
  - CC 1 (mod wheel): update mod output.
  - CC 64 (sustain pedal): update sustain state; when sustain is released and
    no note is held, drop gate.
  - PitchBend: update pitch output.
  - All other messages silently ignored.
- [ ] `process` fills each output buffer for the block:
  - `v_oct`: constant `(note - 69) / 12.0` for all samples.
  - `trigger`: 1.0 for all samples on the first block after note-on, then 0.0;
    trigger is cleared at the end of `process`.
  - `gate`: constant 1.0 or 0.0 for all samples.
  - `mod`: constant normalised CC value.
  - `pitch`: constant normalised pitchbend value.
- [ ] `as_midi_receiver` returns `Some(self)`.
- [ ] Unit tests cover:
  - Note-on sets `v_oct`, raises `gate`, raises `trigger`.
  - Second `process` call clears `trigger`.
  - Note-off drops `gate` (when sustain inactive).
  - Sustain pedal holds `gate` after note-off; releasing sustain drops `gate`.
  - Mod wheel CC updates `mod` output.
  - Pitchbend updates `pitch` output with correct normalisation.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

V/oct convention: MIDI note 69 (A4) = 0 V; each semitone = 1/12 V. This
matches the convention used by modular synthesisers so that a VCO with a
1 V/oct input can be driven directly.

Pitchbend normalisation: the MIDI pitchbend range is [0, 16383] with centre at
8192. Normalise to [-1.0, 1.0] as `(raw - 8192) / 8192.0`.

The module is monophonic: only the most-recently-pressed note affects `v_oct`.
Last-note priority is the simplest correct behaviour; a note stack for
lowest/highest priority can be added in a follow-up if needed.
