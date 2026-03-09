---
id: "0104"
title: Move NodeDecision, classify_nodes, and allocate_buffers to patches-core
priority: medium
created: 2026-03-09
epic: E020
depends-on: "0103"
---

## Summary

`NodeDecision`, `classify_nodes`, and `allocate_buffers` (together with the
`BufferAllocation` intermediate struct) are the core of the decision phase.
They depend only on types that will be in `patches-core` after T-0102 and
T-0103. Moving them completes the migration of decision logic and leaves only
the top-level wiring (`make_decisions`, `PlanDecisions`) and the action phase
in engine.

This ticket also finalises the error type split. Decision-phase functions
should return a `PlanError` defined in `patches-core`. Engine's `BuildError`
gains `From<PlanError>` so existing call sites need minimal changes.

## Acceptance criteria

- [ ] `NodeDecision<'a>` is defined in `patches-core`.
- [ ] `classify_nodes` is a free function (or associated function) in
      `patches-core`, taking `&GraphIndex`, `&[NodeId]`, and `&PlannerState`.
- [ ] `BufferAllocation` and `allocate_buffers` are defined in `patches-core`.
      `allocate_buffers` may be a free function or a method on
      `BufferAllocState`; whichever reads most naturally.
- [ ] `PlanError` is defined in `patches-core` with at least the variants:
      `NoSink`, `MultipleSinks`, `BufferPoolExhausted`, `Internal(String)`.
- [ ] Engine's `BuildError` implements `From<PlanError>`.
- [ ] `patches-engine` imports these types from `patches-core`; its own
      definitions are deleted.
- [ ] All existing tests (including `classify_nodes` unit tests) pass
      unchanged or are moved to `patches-core` test modules.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.

## Notes

`classify_nodes` tests currently live in `patches-engine/src/builder.rs`.
They use only `patches_core` and `patches_modules` types and can move to
`patches-core` tests unchanged (or as integration tests in a test module).
`patches_modules` is a dev-dependency of `patches-core`'s tests already if
needed, or the tests can be left in engine as integration-style tests.
