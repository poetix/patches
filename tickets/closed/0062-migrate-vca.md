---
id: "0062"
title: Migrate Vca to Module v2
priority: high
epic: "E013"
created: 2026-03-04
---

## Summary

Rewrite `patches-modules/src/vca.rs` so that `Vca` implements the `Module` v2 contract.
`Vca` is stateless (no parameters, no sample-rate dependency), making this the simplest
migration in the epic.

## Acceptance criteria

- [ ] `Vca` no longer has a `new()` constructor.
- [ ] `Vca::describe(shape)` returns a `ModuleDescriptor` with:
      - `module_name`: `"Vca"`
      - 2 inputs: `"in"` / index 0, `"cv"` / index 0
      - 1 output: `"out"` / index 0
      - 0 parameters
- [ ] `prepare()` stores `descriptor`; ignores `audio_environment` (no sample-rate
      dependence). Assigns a fresh `InstanceId`.
- [ ] `update_validated_parameters()` is a no-op; returns `Ok(())`.
- [ ] `process` behaviour unchanged: `outputs[0] = inputs[0] * inputs[1]`.
- [ ] `as_any` implemented.
- [ ] `Default` impl removed (no longer meaningful without a constructor).
- [ ] All existing tests pass, rewritten to use a local registry:
      - `descriptor_shape`
      - `multiplies_signal_by_cv`
      - `zero_cv_silences_signal`
      - `negative_cv_inverts_phase`
      - `instance_ids_are_distinct`
- [ ] `cargo clippy -p patches-modules` and `cargo test -p patches-modules` clean.

## Notes

The `Default` impl in the current code is only valid because `new()` exists. It can be
dropped. No other module depends on `Vca::default()`.
