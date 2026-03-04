---
id: "0064"
title: Migrate SawtoothOscillator and SquareOscillator to Module v2
priority: high
epic: "E013"
created: 2026-03-04
---

## Summary

Rewrite `patches-modules/src/waveforms.rs` so that both `SawtoothOscillator` and
`SquareOscillator` implement the `Module` v2 contract. Both are V/OCT oscillators with
a `base_voct` parameter and derive their sample rate from `AudioEnvironment`. Remove
`new()` constructors and `initialise()`. Update tests to use a local `Registry`.

## Acceptance criteria

### SawtoothOscillator

- [ ] `SawtoothOscillator` has no `new()` constructor or `initialise()` method.
- [ ] `SawtoothOscillator::describe(shape)` returns:
      - `module_name`: `"SawtoothOscillator"`
      - 1 input: `"voct"` / index 0
      - 1 output: `"out"` / index 0
      - 1 parameter: `"base_voct"`, `Float { min: -4.0, max: 8.0, default: 0.0 }`
- [ ] `prepare()` stores `audio_environment` and `descriptor`; extracts `sample_rate`;
      initialises `base_voct` to `0.0` and `phase` to `0.0`.
- [ ] `update_validated_parameters()` reads `"base_voct"` and stores it.
- [ ] `process` behaviour unchanged: uses `C2_FREQ * 2^(base_voct + inputs[0])` for
      frequency; output `2.0 * phase - 1.0`.

### SquareOscillator

- [ ] `SquareOscillator` has no `new()` constructor or `initialise()` method.
- [ ] `SquareOscillator::describe(shape)` returns:
      - `module_name`: `"SquareOscillator"`
      - 2 inputs: `"voct"` / index 0, `"pulse_width"` / index 0
      - 1 output: `"out"` / index 0
      - 1 parameter: `"base_voct"`, `Float { min: -4.0, max: 8.0, default: 0.0 }`
- [ ] `prepare()` and `update_validated_parameters()` mirror SawtoothOscillator.
- [ ] `process` behaviour unchanged: `pulse_width = 0.5 + 0.5 * inputs[1]`;
      output `1.0` if `phase < pulse_width`, else `-1.0`.

### Both

- [ ] `as_any` implemented on both.
- [ ] All existing tests pass, rewritten to use a local registry:
      - `instance_ids_are_distinct`
      - `descriptor_ports` (or equivalent)
      - `output_completes_full_cycle_in_period_samples` (sawtooth)
      - pulse-width tests (square)
- [ ] `cargo clippy -p patches-modules` and `cargo test -p patches-modules` clean.

## Notes

Both oscillators compute frequency per sample from the `voct` audio input, so the
`base_voct` parameter is applied additively at audio rate. The `update_validated_parameters`
call only updates the stored `base_voct` field; no precomputed phase increment is needed
(frequency is computed fresh each sample from the input).
