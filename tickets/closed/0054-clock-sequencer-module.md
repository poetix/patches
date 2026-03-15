---
id: "0054"
title: ClockSequencer module
priority: high
epic: "E012"
created: 2026-03-03
---

## Summary

Add a `ClockSequencer` module to `patches-modules` that generates bar, beat, quaver,
and semiquaver trigger pulses from a configurable BPM and time signature. All four
outputs are derived from a single beat-phase accumulator, keeping them perfectly
phase-locked. Supports both simple (quavers_per_beat=2) and compound
(quavers_per_beat=3) time.

## Acceptance criteria

- [ ] `ClockSequencer::new(bpm: f32, beats_per_bar: u32, quavers_per_beat: u32)` compiles.
- [ ] Four output ports: `bar/0`, `beat/1`, `quaver/2`, `semiquaver/3`.
- [ ] Outputs are `0.0` on all other samples and `1.0` on the one sample at each boundary.
- [ ] `bar` fires simultaneously with a `beat` every `beats_per_bar` beats.
- [ ] `beat` fires every beat; `quaver` every `1/quavers_per_beat` of a beat; `semiquaver` every half-quaver.
- [ ] `receive_signal` handles `"bpm"`, `"beats_per_bar"`, `"quavers_per_beat"` (Float casts to u32 for the latter two).
- [ ] Unit tests: correct pulse count over a known number of samples for 4/4 and 6/8.
- [ ] `cargo clippy` and `cargo test -p patches-modules` clean.

## Notes

All subdivision outputs derive from one `beat_phase: f32 ∈ [0.0, 1.0)` accumulator:

```
beat_phase += bpm / (60.0 * sample_rate);
```

Subdivision crossing is detected by comparing integer bucket indices before and after
the increment:

```
// semiquaver
let buckets = quavers_per_beat * 2;
let old_bucket = (old_phase * buckets as f32) as u64;
let new_bucket = (new_phase * buckets as f32) as u64;
if new_bucket > old_bucket || beat_fired { semiquaver = 1.0; }
```

(Beat wrap is handled separately: when `beat_phase >= 1.0`, subtract 1.0 and increment
`beat_count`; bar fires when `beat_count % beats_per_bar == 0`.)

No allocations in `process`. All state is primitive fields.
