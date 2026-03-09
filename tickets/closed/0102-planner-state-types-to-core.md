---
id: "0102"
title: Move planner state types to patches-core
priority: medium
created: 2026-03-09
epic: E020
depends-on: ~
---

## Summary

`NodeState`, `PlannerState`, `BufferAllocState`, `ModuleAllocState`, and
`ModuleAllocDiff` live in `patches-engine/src/builder.rs` but have no engine
dependency — they are pure data and bookkeeping logic. Moving them to
`patches-core` is the foundation step for E020, because every subsequent
ticket depends on these types being importable by core code.

`NodeState.pool_index` is an engine-pool detail that the decision phase never
reads (the action phase recovers it from `module_diff.slot_map` which it
already holds). It is removed in this ticket; the action phase is adjusted
accordingly.

## Acceptance criteria

- [ ] `NodeState`, `PlannerState`, `BufferAllocState`, `ModuleAllocState`, and
      `ModuleAllocDiff` are defined in `patches-core` (a new module,
      e.g. `patches_core::planner`).
- [ ] `NodeState.pool_index` is removed. The engine action phase obtains the
      pool index from `module_diff.slot_map` instead (it already does this;
      the field is just no longer written back into `NodeState`).
- [ ] `patches-engine` imports these types from `patches-core`; its own
      definitions are deleted.
- [ ] All existing tests pass unchanged.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.

## Notes

A single `pub mod planner` (or `pub mod builder`) in `patches-core` is a
reasonable home. The module should be re-exported from the crate root so
callers use `patches_core::PlannerState` etc. without a deep path.

`ModuleAllocState::diff` is the only method on these types; it moves with the
struct. Its only dependency is `InstanceId` and `BuildError` — the error
variant `ModulePoolExhausted` is an engine concern, so this ticket may need to
introduce a lightweight `PlanError` in core (or temporarily keep the error
type in engine and return it by value). This is intentionally left to
implementor judgement; a `PlanError` introduced here can be expanded in T-0104.
