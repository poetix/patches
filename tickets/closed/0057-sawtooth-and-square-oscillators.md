---
id: "0057"
title: Sawtooth and square oscillators
priority: medium
epic: "E012"
created: 2026-03-03
---

## Summary

Add `SawtoothOscillator` and `SquareOscillator` to `patches-modules/src/waveforms.rs`.
Both accept a base V/OCT pitch at construction and a `voct` input port for per-sample
pitch modulation. Frequency is recomputed every sample from the live input.

## Acceptance criteria

- [ ] `SawtoothOscillator::new(base_voct: f32)` and `SquareOscillator::new(base_voct: f32)` compile.
- [ ] Each has one input port `voct/0` and one output port `out/0`.
- [ ] Sawtooth output = `2.0 * phase - 1.0` (range `[-1.0, 1.0)`).
- [ ] Square output = `1.0` when `phase < 0.5`, `-1.0` otherwise.
- [ ] Phase wraps within `[0.0, 1.0)`.
- [ ] Frequency computed as `C2_FREQ * 2_f32.powf(base_voct + inputs[0])` each sample.
- [ ] `initialise` stores `sample_rate` for use in `process`.
- [ ] Both types are re-exported from `patches-modules::lib`.
- [ ] Unit tests: one full cycle produces consistent output; instance IDs are distinct.
- [ ] `cargo clippy` and `cargo test -p patches-modules` clean.

## Notes

`C2_FREQ = 65.406_194_f32` (Hz). Define as a module-level constant in `waveforms.rs`.

Both oscillators share the same phase accumulation logic; consider a private
`advance_phase(phase: &mut f32, freq: f32, sample_rate: f32)` helper in the same file
to avoid duplication, or just inline it — both options are fine.

`2_f32.powf(x)` is in `std`; no new dependency needed.
