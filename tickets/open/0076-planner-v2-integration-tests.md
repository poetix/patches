---
id: "0076"
title: Planner v2 integration tests
priority: medium
epic: "E014"
depends: ["0075"]
created: 2026-03-04
---

## Summary

Add integration tests in `patches-integration-tests` that exercise the full v2 planning
pipeline end-to-end: graph diffing, parameter diffs, module survival, and the new startup
sequence. These tests validate the contract from ADR 0012 at the integration level.

## Acceptance criteria

- [ ] **Surviving modules are not re-instantiated.** Build a plan, then update with an
      identical graph. Verify that no `new_modules` appear in the second plan and the
      same `InstanceId`s are preserved.
- [ ] **New node triggers instantiation.** Add a node to the graph and rebuild. Verify
      it appears in `new_modules` with a fresh `InstanceId`.
- [ ] **Removed node triggers tombstone.** Remove a node and rebuild. Verify it appears
      in `tombstones`.
- [ ] **Type change triggers tombstone + new.** Change a node's module type and rebuild.
      Verify the old slot is tombstoned and a new module is instantiated.
- [ ] **Parameter-only change produces diffs.** Change a parameter value on a surviving
      node. Verify `parameter_updates` contains the diff and `new_modules` is empty.
- [ ] **State preservation across parameter updates.** Build a plan with an oscillator,
      tick it to advance phase, then apply a parameter-only update. Verify the oscillator
      phase is preserved (the module instance was not replaced).
- [ ] **Initial plan builds with real AudioEnvironment.** Verify that `PatchEngine::start()`
      produces a plan whose modules were built with a non-zero sample rate.
- [ ] `cargo clippy` and `cargo test` clean.

## Notes

Some of these tests may overlap with unit tests written in earlier tickets. The
integration tests exercise the full stack (graph → planner → plan → module pool) rather
than individual functions.

Tests that need to verify module state (e.g. oscillator phase preservation) can use
`as_any()` to downcast and inspect internal state, following the pattern established in
existing integration tests.
