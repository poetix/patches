---
id: "0115"
title: "New `Module` trait: `process_alias(pool_read, pool_write)` + `set_ports`"
priority: high
epic: "E022"
depends_on: ["0113"]
created: 2026-03-11
---

## Summary

Update the `Module` trait and the buffer pool to adopt the new polyphonic cable
model. A new `process_alias` method is added with a default no-op so that
existing module implementations continue to compile unchanged. Port objects are
stored on the module and delivered via a new `set_ports` method.
`set_connectivity` is removed, as connectivity is now carried by the `connected`
field on `MonoInput`, `PolyInput`, `MonoOutput`, and `PolyOutput`.

The old `process(&mut self, inputs: &[f32], outputs: &mut [f32])` method remains
in the trait (still required) so that all existing implementations compile.
T-0118 will migrate each module to implement `process_alias` and remove its
old `process` implementation. Once all migrations are complete a follow-up
ticket will rename `process_alias` → `process` and remove the old signature.

## Acceptance criteria

- [ ] `Module::process_alias` is added with a default no-op implementation:
      ```rust
      fn process_alias(&mut self, pool_read: &[CableValue], pool_write: &mut [CableValue]) {}
      ```
      The method is documented as audio-thread-safe: implementations must not
      allocate or block.
- [ ] The existing `Module::process(&mut self, inputs: &[f32], outputs: &mut [f32])`
      method is left unchanged (still a required method). No existing module
      implementations are modified by this ticket.
- [ ] `Module::set_ports` is added with a default no-op implementation:
      ```rust
      fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {}
      ```
      The method is documented as audio-thread-safe: implementations must not
      allocate or block.
- [ ] `Module::set_connectivity` is removed from the trait. Any existing
      implementations in `patches-modules` are deleted (the method was already a
      default no-op, so removing it from the trait does not break callers that
      merely implement it).
- [ ] The buffer pool type changes from `Vec<[f32; 2]>` to `Vec<[CableValue; 2]>`.
      The ping-pong read/write half semantics are preserved.
- [ ] `ExecutionPlan::tick()` is updated to call `process_alias` (passing the
      appropriate pool halves) instead of the old `process`. The planner's
      `connectivity_updates` field and its application in `receive_plan` are
      removed here; port objects delivered via `set_ports` carry `connected`.
- [ ] The existing gather-before / scatter-after logic in `tick()` is removed.
      Modules now read and write the pool directly via their stored port objects
      (once migrated; until then their `process_alias` no-ops silently).
- [ ] `cargo build` compiles across all crates without errors. Tests may fail
      because migrated modules have not yet been updated; this is expected.
      Compilation must succeed.
- [ ] `cargo clippy` reports no new warnings beyond pre-existing migration gaps.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

`set_connectivity` removal is safe: the method had a default no-op, so removing
it from the trait only breaks code that calls it explicitly (the engine's
`connectivity_updates` path). That path is removed alongside it.

The buffer pool slot size grows from `[f32; 2]` (16 bytes) to
`[CableValue; 2]` (≈272 bytes). For a 64-cable graph the pool grows from ~1 KB
to ~17 KB — still L1-resident.

This ticket intentionally leaves all module implementations on the old `process`
path until T-0117 (`MonoShim`) and T-0118 (migration). The goal is a clean
trait boundary that compiles first.
