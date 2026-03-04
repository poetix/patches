---
id: "0066"
title: Migrate ClockSequencer to Module v2
priority: high
epic: "E013"
created: 2026-03-04
---

## Summary

Rewrite `patches-modules/src/clock_sequencer.rs` so that `ClockSequencer` implements the
`Module` v2 contract. BPM, beats-per-bar, and quavers-per-beat are declared as typed
parameters. Remove the `new(bpm, beats_per_bar, quavers_per_beat)` constructor. Update
`receive_signal` to match `ControlSignal::ParameterUpdate`. Update tests.

## Acceptance criteria

- [ ] `ClockSequencer` has no `new()` constructor or `initialise()` method.
- [ ] `ClockSequencer::describe(shape)` returns:
      - `module_name`: `"ClockSequencer"`
      - 0 inputs
      - 4 outputs: `"bar"` / 0, `"beat"` / 1, `"quaver"` / 2, `"semiquaver"` / 3
      - 3 parameters:
        - `"bpm"`:             `Float { min: 1.0, max: 300.0, default: 120.0 }`
        - `"beats_per_bar"`:   `Int   { min: 1,   max: 16,    default: 4 }`
        - `"quavers_per_beat"`: `Int  { min: 1,   max: 4,     default: 2 }`
- [ ] `prepare()` stores `audio_environment` and `descriptor`; extracts `sample_rate`;
      initialises `beat_phase` to `0.0` and `beat_count` to `0`. Other fields zero.
- [ ] `update_validated_parameters()` reads all three parameters (if present) and
      recomputes `beat_phase_delta = bpm / (60.0 * sample_rate)`.
- [ ] `receive_signal` matches `ControlSignal::ParameterUpdate { name, value }` for
      `"bpm"` (Float), `"beats_per_bar"` (Int), and `"quavers_per_beat"` (Int). Updates
      the stored value and recomputes `beat_phase_delta` as needed. No longer matches
      `ControlSignal::Float`.
- [ ] `process` behaviour unchanged.
- [ ] `as_any` implemented.
- [ ] All existing tests pass, rewritten to use a local registry with explicit parameter maps:
      - time signature tests (4/4, 6/8)
      - BPM timing
      - receive_signal update tests
      - (all 7 existing tests or equivalent)
- [ ] `cargo clippy -p patches-modules` and `cargo test -p patches-modules` clean.

## Notes

The outputs are indexed 0–3 matching their position in `process`'s output slice, not
arbitrary indices. Ensure descriptor output indices match this layout.

`receive_signal` must handle `ParameterValue::Int(v)` for `beats_per_bar` and
`quavers_per_beat`. The `as i64` cast followed by `as u32` is appropriate (bounds are
enforced by parameter validation before the signal is queued).
