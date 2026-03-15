---
id: "0065"
title: Migrate AdsrEnvelope to Module v2
priority: high
epic: "E013"
created: 2026-03-04
---

## Summary

Rewrite `patches-modules/src/adsr_envelope.rs` so that `AdsrEnvelope` implements the
`Module` v2 contract. Attack, decay, sustain, and release are declared as typed
`ParameterMap` entries. Remove the `new(a, d, s, r)` constructor. Update tests to use
a local `Registry` with explicit parameter maps.

## Acceptance criteria

- [ ] `AdsrEnvelope` has no `new()` constructor or `initialise()` method.
- [ ] `AdsrEnvelope::describe(shape)` returns:
      - `module_name`: `"AdsrEnvelope"`
      - 2 inputs: `"trigger"` / index 0, `"gate"` / index 0
      - 1 output: `"out"` / index 0
      - 4 parameters:
        - `"attack"`:  `Float { min: 0.001, max: 10.0, default: 0.01 }`
        - `"decay"`:   `Float { min: 0.001, max: 10.0, default: 0.1 }`
        - `"sustain"`: `Float { min: 0.0,   max: 1.0,  default: 0.7 }`
        - `"release"`: `Float { min: 0.001, max: 10.0, default: 0.3 }`
- [ ] `prepare()` stores `audio_environment` and `descriptor`; extracts `sample_rate`;
      initialises all timing fields to zero/default and stage to `Idle`.
- [ ] `update_validated_parameters()` reads all four parameters (if present) and
      recomputes `attack_inc`, `decay_inc`, and `release_inc` from `sample_rate` using the
      same formula as before (`1.0 / (param_secs * sample_rate)`). Stores `sustain` directly.
- [ ] `process` behaviour unchanged.
- [ ] `as_any` implemented.
- [ ] All existing tests pass, rewritten to use a local registry with explicit
      `ParameterMap` entries:
      - `idle_outputs_zero`
      - `attack_rises_to_one` (and related stage tests)
      - `decay_falls_to_sustain`
      - `sustain_holds_at_sustain_level`
      - `release_falls_to_zero`
      - `trigger_restarts_from_current_level`
      - `gate_low_during_attack_goes_to_release`
      - (all 9 existing tests or equivalent)
- [ ] `cargo clippy -p patches-modules` and `cargo test -p patches-modules` clean.

## Notes

The `module.build()` default fills missing parameters from descriptor defaults, so tests
that only care about one parameter can pass a single-entry `ParameterMap` and rely on
defaults for the rest.

```rust
fn make_envelope(attack: f32, decay: f32, sustain: f32, release: f32) -> Box<dyn Module> {
    let mut params = ParameterMap::new();
    params.insert("attack".into(),  ParameterValue::Float(attack));
    params.insert("decay".into(),   ParameterValue::Float(decay));
    params.insert("sustain".into(), ParameterValue::Float(sustain));
    params.insert("release".into(), ParameterValue::Float(release));
    let mut r = Registry { builders: HashMap::new() };
    r.register::<AdsrEnvelope>();
    r.create("AdsrEnvelope", &AudioEnvironment { sample_rate: 44100.0 },
             &ModuleShape { channels: 0 }, &params).unwrap()
}
```
