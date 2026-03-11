---
id: "0111"
title: ReceivesMidi trait, planner identification, ExecutionPlan::midi_receiver_indices
priority: high
created: 2026-03-11
epic: E021
depends_on: ["0109"]
---

## Summary

Replace the default no-op `Module::receive_signal` pattern for MIDI delivery
with an explicit opt-in `ReceivesMidi` trait. The planner identifies modules
that implement it during `build_slots`, and the indices are carried in
`ExecutionPlan` so the audio callback can route events without per-tick
dynamic dispatch overhead.

## Acceptance criteria

- [ ] `ReceivesMidi` trait in `patches-core`:
      `fn receive_midi(&mut self, event: MidiEvent)`.
- [ ] `Module` gains a default method:
      `fn as_midi_receiver(&mut self) -> Option<&mut dyn ReceivesMidi> { None }`.
      Modules that want MIDI override this to return `Some(self)`.
- [ ] `ExecutionPlan` gains `midi_receiver_indices: Vec<usize>` — pool slot
      indices of all MIDI-capable modules in this plan.
- [ ] `ModulePool` gains `receive_midi(&mut self, idx: usize, event: MidiEvent)`
      — calls `as_midi_receiver` on the slot and forwards the event if `Some`.
- [ ] `Planner::build_slots` calls `as_midi_receiver` on each freshly-placed
      module and records the pool index when it returns `Some`.
- [ ] `SubBlockDispatcher` (T-0109) calls `ModulePool::receive_midi` for each
      index in `midi_receiver_indices` for each dispatched event.
- [ ] Existing `receive_signal` / `ControlSignal` mechanism is left untouched
      (it serves non-MIDI parameter delivery and is a separate concern).
- [ ] Unit tests: planner correctly populates `midi_receiver_indices` for a
      graph containing a mix of MIDI-capable and non-MIDI-capable modules.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

Using a gateway method (`as_midi_receiver`) rather than `Any` downcasting keeps
the implementation simple and avoids `std::any` in the trait object chain.
Modules that don't implement `ReceivesMidi` pay zero cost — the default returns
`None` and the pool method is a no-op for those indices.
