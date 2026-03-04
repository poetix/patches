---
id: "0063"
title: Migrate AudioOut to Module v2
priority: high
epic: "E013"
created: 2026-03-04
---

## Summary

Rewrite `patches-modules/src/audio_out.rs` so that `AudioOut` implements the `Module`
v2 contract. Remove the `AudioOutFactory` struct and its `Factory` / `ParameterSet`
imports (dead code from an earlier prototype). Update tests to use a local `Registry`.

## Acceptance criteria

- [ ] `AudioOutFactory`, `ParameterSet`, and `Factory` imports removed entirely.
- [ ] `AudioOut` no longer has a `new(descriptor)` constructor.
- [ ] `AudioOut::describe(shape)` returns a `ModuleDescriptor` with:
      - `module_name`: `"AudioOut"`
      - 2 inputs: `"left"` / index 0, `"right"` / index 0
      - 0 outputs
      - 0 parameters
- [ ] `prepare()` stores `descriptor`; ignores `audio_environment`. Assigns a fresh
      `InstanceId`. Initialises `last_left` and `last_right` to `0.0`.
- [ ] `update_validated_parameters()` is a no-op; returns `Ok(())`.
- [ ] `process` behaviour unchanged: stores `inputs[0]` and `inputs[1]`.
- [ ] `as_any` and `as_sink` implemented (as before).
- [ ] `Sink` impl unchanged.
- [ ] `Default` impl removed.
- [ ] All existing tests pass, rewritten to use a local registry (fixing existing broken
      test code — the current file has `sink` / `module` variable name mismatch):
      - `descriptor_has_two_inputs_and_no_outputs`
      - `instance_ids_are_distinct`
      - `process_stores_left_and_right_samples`
- [ ] `cargo clippy -p patches-modules` and `cargo test -p patches-modules` clean.

## Notes

The current `audio_out.rs` imports `patches_core::module::ParameterSet` and
`patches_core::module::Factory` which do not exist in the current `patches-core`. These
imports cause compilation failures. Removing them and the `AudioOutFactory` struct is
part of this ticket.

The test file also has a variable name mismatch (`sink` vs `module`) that should be
fixed as part of writing the new tests.
