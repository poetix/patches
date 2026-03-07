---
id: "0085"
title: Build unified `Oscillator` module with four waveform outputs
priority: high
created: 2026-03-06
epic: "E016"
depends_on: ["0084"]
---

## Summary

`SineOscillator` is extended into a full multi-waveform `Oscillator` module.
It keeps the existing `voct` and `fm` inputs (and `fm_type` / `frequency`
parameters) and adds a `pulse_width` input. It exposes four outputs — `sine`,
`triangle`, `sawtooth`, `square` — all driven by the same phase accumulator.
Only connected outputs are computed each sample (using `set_connectivity`).
The module is registered under the name `"Oscillator"`. The old `"SineOscillator"`
registration is removed.

## Acceptance criteria

- [ ] A new `struct Oscillator` in `patches-modules/src/oscillator.rs` replaces
      `SineOscillator`.
- [ ] Module descriptor:
      - name: `"Oscillator"`
      - inputs: `voct` (index 0), `fm` (index 1), `pulse_width` (index 2)
      - outputs: `sine` (index 0), `triangle` (index 1), `sawtooth` (index 2),
        `square` (index 3)
      - parameters: `frequency` (Float, min 0.01, max 20000.0, default 0.0),
        `fm_type` (Enum, variants `["linear", "logarithmic"]`, default `"linear"`)
- [ ] `Oscillator::prepare` constructs `UnitPhaseAccumulator::new(sample_rate, C0_FREQ)`.
- [ ] `set_connectivity` records which outputs are connected and which of
      `voct`, `fm`, `pulse_width` inputs are live.
- [ ] `process` advances the phase accumulator once per sample (using
      `advance` or `advance_modulating` depending on connectivity, as per the
      existing `SineOscillator` pattern). Waveform values are computed and
      written to `outputs[i]` only if the corresponding output is connected.
- [ ] Waveform formulae (unsmoothed; PolyBLEP is added in T-0088):
      - **sine**: `lookup_sine(phase)` (existing lookup table)
      - **triangle**: `1.0 - 4.0 * (phase - 0.5).abs()` — peaks at 0 and 0.5,
        range `[-1.0, 1.0]`
      - **sawtooth**: `2.0 * phase - 1.0`, range `[-1.0, 1.0)`
      - **square**: `if phase < duty { 1.0 } else { -1.0 }` where
        `duty = 0.5 + 0.5 * inputs[pulse_width_idx]` (clamped to `[0.01, 0.99]`)
        when `pulse_width` is connected; otherwise `duty = 0.5`
- [ ] `Oscillator` is registered in the default module registry
      (`patches-modules/src/lib.rs`). `SineOscillator` registration is removed.
- [ ] All existing `SineOscillator` tests are ported to `Oscillator` (using
      the `sine` output, index 0).
- [ ] New tests cover:
      - descriptor has 3 inputs and 4 outputs with correct names
      - triangle output completes a consistent full cycle
      - sawtooth output matches `2*phase - 1` over a known period
      - square output is only `±1.0`; duty cycle responds to `pulse_width` input
      - disconnected outputs write nothing (assert `outputs[i]` is unchanged
        when connectivity marks that output disconnected)
- [ ] `cargo build`, `cargo test`, `cargo clippy` pass with no new warnings.

## Notes

Keep the `pulse_width` input clamped to `[0.01, 0.99]` to avoid degenerate
all-high or all-low square waves and to bound PolyBLEP correctness in T-0088.

The `frequency` parameter default is changed to `0.0` (offset from C0 reference)
rather than `440.0`. Patches that previously relied on `SineOscillator` with
`frequency: 440.0` should set `frequency: 440.0` on `Oscillator`; the slight
C0 offset (≈ 16 Hz) is negligible for pitched use and documented in T-0084.

The `"SineOscillator"` name should be removed from the registry as part of this
ticket; patches referencing it by name will fail to load with a clear error
until updated (T-0087).
