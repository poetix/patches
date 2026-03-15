---
id: "0084"
title: Refactor `FrequencyControl` — `reference_frequency` constructor parameter and rename `frequency_offset`
priority: high
created: 2026-03-06
epic: "E016"
---

## Summary

`FrequencyControl` currently has a single `base_frequency` field set via
`set_base_frequency`, which is used as the absolute starting frequency before
V/OCT and FM modulation. To make the frequency semantics explicit for
V/OCT-rooted oscillators, `FrequencyControl` needs a fixed `reference_frequency`
established at construction time, and the mutable user-supplied value is renamed
to `frequency_offset` (it is added to `reference_frequency` to produce the base
pitch). `UnitPhaseAccumulator::new` gains a `reference_frequency` parameter so
callers can set it without touching `FrequencyControl` directly.

## Acceptance criteria

- [ ] `FrequencyControl::new(reference_frequency: f32)` replaces the current
      `FrequencyControl::new()`. The `reference_frequency` field is private and
      immutable after construction.
- [ ] `FrequencyControl::base_frequency` is renamed to `frequency_offset: f32`.
      Its initial value is `0.0`.
- [ ] `FrequencyControl::compute` uses `self.reference_frequency + self.frequency_offset`
      as the base pitch before applying any V/OCT or FM modulation.
- [ ] `UnitPhaseAccumulator::new(sample_rate: f32, reference_frequency: f32)` passes
      `reference_frequency` to `FrequencyControl::new`.
- [ ] `UnitPhaseAccumulator::set_base_frequency` is renamed
      `set_frequency_offset`; it sets `frequency_control.frequency_offset` and
      recomputes the phase increment using
      `reference_frequency + frequency_offset`.
- [ ] `SineOscillator::prepare` constructs its `UnitPhaseAccumulator` with
      `reference_frequency = C0_FREQ` (≈ 16.352 Hz, defined as a named constant
      in `frequency.rs` or `oscillator.rs`).
- [ ] `SineOscillator::update_validated_parameters` passes the user `frequency`
      parameter value to `set_frequency_offset` (the parameter name and YAML
      key are unchanged; only the internal method name changes).
- [ ] Existing `SineOscillator` tests pass without modification (behaviour is
      unchanged because `frequency_offset` absorbs the full user-supplied Hz
      value; at default `voct=0` the output is identical to before).
- [ ] `cargo build`, `cargo test`, `cargo clippy` pass with no new warnings.

## Notes

The `compute` formula after this change is:

```
base = reference_frequency + frequency_offset
// V/OCT and FM modulation applied on top of base, as before
```

For `SineOscillator` used as an LFO (e.g. `frequency: 0.2`) the user parameter
sets `frequency_offset = 0.2`, so `base = C0 + 0.2 ≈ 16.55 Hz`. This is a
very slight change in pitch (< 1 semitone at LFO ranges) which is acceptable
for the existing demo patches. If exact pitch precision for LFO use matters,
consider adding a `SineOscillator`-specific parameter note in docs; that
trade-off is out of scope here.

`FrequencyControl` and `UnitPhaseAccumulator` live in
`patches-modules/src/common/frequency.rs`.
