---
id: "0105"
title: Move PlanDecisions and make_decisions to patches-core; slim engine builder
priority: medium
created: 2026-03-09
epic: E020
depends-on: "0104"
---

## Summary

After T-0102–T-0104, all the ingredients of `make_decisions` live in
`patches-core`. This ticket moves `PlanDecisions` and `make_decisions` there
too, reducing `patches-engine/src/builder.rs` to the action phase only:
minting `InstanceId`s, calling `registry.create`, running `module_alloc.diff`,
assembling `ModuleSlot`s, and constructing `ExecutionPlan`.

The engine's `PatchBuilder::make_decisions` becomes a one-line delegate, or is
removed entirely if callers are updated to call the core function directly.

## Acceptance criteria

- [ ] `PlanDecisions<'a>` is defined in `patches-core`.
- [ ] A `make_decisions` function (free function or method on a lightweight
      type) is defined in `patches-core`, returning
      `Result<PlanDecisions<'_>, PlanError>`.
- [ ] `patches-engine/src/builder.rs` contains only:
      - `ExecutionPlan` and its `tick` / `dispatch_signal` methods
      - `ModuleSlot`
      - `PatchBuilder` with `build_patch` (action phase only)
      - `BuildError` (action-phase variants + `From<PlanError>`)
      - The `build_patch` convenience free function
- [ ] All existing public types re-exported from `patches-engine` where
      external callers (e.g. `patches-player`, integration tests) depend on
      them — or those callers are updated to import from `patches-core` directly.
- [ ] All existing tests pass unchanged.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean across all crates.

## Notes

`patches-player` and `patches-integration-tests` currently import
`PlannerState`, `BuildError` etc. from `patches-engine`. After this ticket,
the preferred import site for decision-phase types is `patches-core`. Callers
should be updated rather than using re-exports, unless the re-export is
genuinely the cleaner choice.
