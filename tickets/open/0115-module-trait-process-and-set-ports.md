---
id: "0115"
title: "New `Module` trait: `process(pool_read, pool_write)` + `set_ports`"
priority: high
epic: "E022"
depends_on: ["0113"]
created: 2026-03-11
---

## Summary

Update the `Module` trait and the buffer pool to adopt the new polyphonic cable
model. `process()` takes pool slices directly; port objects are stored on the
module and delivered via a new `set_ports` method. `set_connectivity` is removed, as connectivity is now carried by the `connected`
field on `MonoInput`, `PolyInput`, `MonoOutput`, and `PolyOutput`.

## Acceptance criteria

- [ ] `Module::process` signature changes from the current form to:
      ```rust
      fn process(&mut self, pool_read: &[CableValue], pool_write: &mut [CableValue]);
      ```
- [ ] `Module::set_ports` is added with a default no-op implementation:
      ```rust
      fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {}
      ```
      The method is documented as audio-thread-safe: implementations must not
      allocate or block.
- [ ] `Module::set_connectivity` is removed from the trait. Any existing
      implementations in `patches-modules` are deleted.
- [ ] The buffer pool type changes from `Vec<[f64; 2]>` to `Vec<[CableValue; 2]>`.
      The ping-pong read/write half semantics are preserved.
- [ ] `ExecutionPlan::tick()` is updated to pass the appropriate pool halves as
      `pool_read` and `pool_write` to each module's `process()`.
- [ ] The existing gather-before / scatter-after logic in `tick()` is removed.
      Modules now read and write the pool directly via their stored port objects.
- [ ] `cargo build` compiles across all crates. Tests may fail at this point
      because module implementations have not yet been migrated (T-0118); this is
      expected. Compilation must succeed.
- [ ] `cargo clippy` reports no new warnings beyond pre-existing migration gaps.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

The removal of `set_connectivity` is a breaking API change. The planner's
`connectivity_updates` field in `ExecutionPlan` is removed here as well; port
objects delivered via `set_ports` carry `connected` on each concrete port type.

The buffer pool slot size grows from `[f64; 2]` (16 bytes) to
`[CableValue; 2]` (≈272 bytes). For a 64-cable graph the pool grows from ~1 KB
to ~17 KB — still L1-resident.

This ticket intentionally leaves module implementations broken until T-0117
(`MonoShim`) and T-0118 (migration). The goal is a clean trait boundary first.
