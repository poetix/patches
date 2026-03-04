---
id: "0061"
title: Migrate Sum to Module v2
priority: high
epic: "E013"
created: 2026-03-04
---

## Summary

Rewrite `patches-modules/src/sum.rs` so that `Sum` implements the `Module` v2 contract.
`Sum` is shape-sensitive: the number of inputs equals `shape.channels`. Remove the
`new(size)` constructor. Update tests to use a local `Registry`.

## Acceptance criteria

- [ ] `Sum` no longer has a `new()` constructor.
- [ ] `Sum::describe(shape)` returns a `ModuleDescriptor` with:
      - `module_name`: `"Sum"`
      - `shape.channels` inputs, each named `"in"` with `index` 0..channels-1
      - 1 output named `"out"` / index 0
      - 0 parameters
- [ ] `prepare()` stores `audio_environment` and `descriptor`; derives `size` from
      `descriptor.shape.channels` (the canonical source of truth, not `descriptor.inputs.len()`).
- [ ] `update_validated_parameters()` is a no-op (no parameters declared); returns `Ok(())`.
- [ ] `process` behaviour unchanged: `outputs[0] = inputs[..size].iter().sum()`.
- [ ] `as_any` implemented.
- [ ] All existing tests pass, rewritten to use a local registry with
      `ModuleShape { channels: N }` to produce the desired number of inputs:
      - `descriptor_shape_size_3`
      - `size_1_passes_input_unchanged`
      - `size_3_sums_inputs`
      - `instance_ids_are_distinct`
- [ ] `cargo clippy -p patches-modules` and `cargo test -p patches-modules` clean.

## Notes

`Sum` is the primary example of how `ModuleShape` encodes structural variation. A `Sum`
with 3 inputs is created with `ModuleShape { channels: 3 }`. The descriptor records this
in `shape` and in the length of `inputs`; callers use the descriptor to learn the port
layout at runtime.
