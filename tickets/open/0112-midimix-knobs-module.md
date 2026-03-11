---
id: "0112"
title: MidiMixKnobs module
priority: medium
created: 2026-03-11
epic: E021
depends_on: ["0111"]
---

## Summary

Add a `MidiMixKnobs` module to `patches-modules` that implements `ReceivesMidi`
and translates CC messages from an Akai MIDImix controller into normalised
voltage outputs. Each knob/fader maps to one output port; the module holds the
last-received CC value as internal state and emits it as a constant signal each
tick.

## Acceptance criteria

- [ ] `MidiMixKnobs` in `patches-modules` implements both `Module` and
      `ReceivesMidi`.
- [ ] Output port count matches the MIDImix's knob/fader layout (8 channel
      strips × 3 knobs + 8 faders = 32 outputs; exact mapping to be confirmed
      against the MIDImix MIDI spec).
- [ ] `receive_midi` updates internal state for the relevant CC number; unknown
      CC numbers are silently ignored.
- [ ] `process` emits the current normalised value (CC value / 127.0) on each
      output port for every sample in the block. No per-sample state mutation.
- [ ] `as_midi_receiver` returns `Some(self)`.
- [ ] Unit tests: CC message for a known knob updates the corresponding output;
      unknown CC is ignored; output value matches `cc_value / 127.0`.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

The MIDImix sends knob/fader changes as CC messages on channel 1. The CC
numbers for each control are fixed by the device (see Akai MIDImix MIDI
implementation chart). A const array mapping CC number → port index is the
simplest implementation.

The module emits a constant value each tick; smoothing (slew limiting) to avoid
zipper noise on fast movements can be added in a follow-up ticket if needed.
