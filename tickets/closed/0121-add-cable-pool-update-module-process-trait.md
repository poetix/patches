---
id: "0121"
title: Add CablePool, update Module::process trait, remove port accessor methods
priority: high
created: 2026-03-12
epic: E023
---

## Summary

Introduce `CablePool<'a>` in `patches-core` as the sole interface through which
modules read and write cable values. Update `Module::process` to accept
`&mut CablePool<'_>` instead of `(&mut [[CableValue; 2]], usize)`. Remove the
`read_from` / `write_to` methods from the port types, which become plain
index-and-metadata structs.

After this ticket `patches-core` will compile cleanly. `patches-engine`,
`patches-modules`, and `patches-integration-tests` will not — resolved by
tickets 0122–0124.

## Acceptance criteria

- [ ] `CablePool<'a>` defined in `patches-core` (suggest `patches-core/src/cables.rs`
  or a new `patches-core/src/cable_pool.rs`):
  ```rust
  pub struct CablePool<'a> {
      pool: &'a mut [[CableValue; 2]],
      wi: usize,
  }

  impl<'a> CablePool<'a> {
      pub fn new(pool: &'a mut [[CableValue; 2]], wi: usize) -> Self;
      pub fn read_mono(&self, input: &MonoInput) -> f32;
      pub fn read_poly(&self, input: &PolyInput) -> [f32; 16];
      pub fn write_mono(&mut self, output: &MonoOutput, value: f32);
      pub fn write_poly(&mut self, output: &PolyOutput, value: [f32; 16]);
  }
  ```
- [ ] `read_mono` and `read_poly` apply the port's `scale` field.
- [ ] `Module::process` signature changed to:
  ```rust
  fn process(&mut self, pool: &mut CablePool<'_>);
  ```
- [ ] `MonoInput::read_from`, `PolyInput::read_from` removed.
- [ ] `MonoOutput::write_to`, `PolyOutput::write_to` removed.
- [ ] Any test stub modules inside `patches-core` updated to compile with the
  new signature.
- [ ] `cargo test -p patches-core` passes.
- [ ] `cargo clippy -p patches-core` clean.

## Notes

`MonoInput` also has a `read` method taking `&[CableValue]` (flat slice). Check
whether it can be removed along with `read_from`, or whether it is used
elsewhere before deleting it.

The `connected` field on port types is unchanged — modules still use
`input.is_connected()` to gate processing.
