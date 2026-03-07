---
id: "E016"
title: Unified Oscillator — consolidate waveforms under a single module with PolyBLEP anti-aliasing
created: 2026-03-06
tickets: ["0084", "0085", "0086", "0087", "0088", "0089"]
---

## Summary

The current codebase has three separate oscillator modules (`SineOscillator`,
`SawtoothOscillator`, `SquareOscillator`) with no shared infrastructure and two
different frequency-control paradigms (Hz-based `FrequencyControl` vs. raw
V/OCT phase advance). All four basic waveforms (sine, triangle, sawtooth,
square) should be outputs of a single `Oscillator` module driven by one phase
accumulator, with connectivity-gated generation so that only connected outputs
are computed. The sawtooth and square outputs should be band-limited using
PolyBLEP.

As a prerequisite, `FrequencyControl` is refactored to separate a fixed
`reference_frequency` (passed at construction time) from a user-tunable
`frequency_offset` (set via parameter), making the frequency semantics explicit
and enabling V/OCT oscillators whose root pitch is tied to a named note (C0).

## Tickets

| ID   | Title                                                                         | Priority | Depends on |
|------|-------------------------------------------------------------------------------|----------|------------|
| 0084 | Refactor `FrequencyControl`: `reference_frequency` + rename `frequency_offset` | high    | —          |
| 0085 | Build unified `Oscillator` module with four waveform outputs                  | high     | 0084       |
| 0086 | Remove `SawtoothOscillator` and `SquareOscillator`; migrate tests             | medium   | 0085       |
| 0087 | Update example patches to use `Oscillator`                                    | medium   | 0086       |
| 0088 | PolyBLEP anti-aliasing for sawtooth and square outputs                        | medium   | 0085       |
| 0089 | Add `phase_mod` input to `Oscillator`                                         | medium   | 0085       |

## Definition of done

- `FrequencyControl::new(reference_frequency)` takes the root pitch in Hz;
  the field previously called `base_frequency` is renamed `frequency_offset`
  and represents a Hz offset from `reference_frequency`.
- `UnitPhaseAccumulator::new(sample_rate, reference_frequency)` passes
  `reference_frequency` through to `FrequencyControl`.
- `SineOscillator` (still registered as `"SineOscillator"` until T-0085
  replaces it) constructs its accumulator with `reference_frequency = C0`
  (≈ 16.35 Hz); user-visible behaviour is unchanged — existing patches
  continue to work with the same parameter values.
- A single module registered as `"Oscillator"` exposes outputs `sine`,
  `triangle`, `sawtooth`, and `square`; inputs `voct` and `fm`; and a
  `pulse_width` input controlling the square duty cycle.
- Waveform outputs are generated only when their output port is connected
  (via `set_connectivity`), avoiding unnecessary computation.
- `SawtoothOscillator` and `SquareOscillator` are removed from `waveforms.rs`;
  their tests are adapted to exercise `Oscillator` outputs.
- `demo_synth.yaml` and `mutual_fm.yaml` are updated to reference `Oscillator`
  instead of `SineOscillator`, `SawtoothOscillator`, or `SquareOscillator`.
  Any Rust example code referencing those types is updated accordingly.
- Sawtooth and square outputs apply PolyBLEP correction at each discontinuity
  to suppress aliasing.
- `Oscillator` has a `phase_mod` input; when connected, `(phase + input).fract()`
  replaces `phase` for all waveform computations that sample.
- `cargo build`, `cargo test`, `cargo clippy` clean with no new warnings.
- No `unwrap()` or `expect()` in library code.
