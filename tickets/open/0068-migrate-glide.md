---
id: "0068"
title: Migrate Glide to Module v2
priority: high
epic: "E013"
created: 2026-03-04
---

## Summary

Rewrite `patches-modules/src/glide.rs` so that `Glide` implements the `Module` v2
contract. The glide time is a typed `ParameterMap` entry. Remove the `new(glide_ms)`
constructor and `initialise()`. Update `receive_signal` to match
`ControlSignal::ParameterUpdate`. Update tests.

## Acceptance criteria

- [ ] `Glide` has no `new()` constructor or `initialise()` method.
- [ ] `Glide::describe(shape)` returns:
      - `module_name`: `"Glide"`
      - 1 input: `"in"` / index 0
      - 1 output: `"out"` / index 0
      - 1 parameter: `"glide_ms"`, `Float { min: 0.0, max: 10_000.0, default: 100.0 }`
- [ ] `prepare()` stores `audio_environment` and `descriptor`; extracts `sample_rate`;
      initialises `log_freq` to `0.0`, `alpha` to `0.01`. Does not call `update_beta`
      (deferred to `update_validated_parameters`).
- [ ] `update_validated_parameters()` reads `"glide_ms"` and calls `update_beta()` to
      recompute `beta`.
- [ ] `receive_signal` matches `ControlSignal::ParameterUpdate { name: "glide_ms", value: ParameterValue::Float(v) }` and calls `set_glide_ms`. No longer matches
      `ControlSignal::Float`.
- [ ] `process` behaviour unchanged.
- [ ] `as_any` implemented.
- [ ] Tests cover:
      - Descriptor ports.
      - Registry creation with default `glide_ms`.
      - Output smoothly transitions toward a target frequency.
      - `receive_signal` updates glide time.
      - `instance_ids_are_distinct`.
- [ ] `cargo clippy -p patches-modules` and `cargo test -p patches-modules` clean.

## Notes

`glide_ms = 0.0` is a valid edge case (the `n_samples = 0` case makes `beta`
undefined via division by zero). The existing `update_beta` should guard against this:
if `n_samples <= 0.0` treat `beta = 1.0` (instant tracking, no glide). Verify this
behaviour is handled correctly during migration.
