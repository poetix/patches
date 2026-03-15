---
id: "0022"
epic: "E004"
title: Remove toposort from build_patch
priority: low
created: 2026-03-01
---

## Summary

`build_patch` currently runs Kahn's topological sort over the module graph before
assembling the `ExecutionPlan`. The sort is unnecessary: `ExecutionPlan::tick()`
uses a two-slot ring buffer (`[f32; 2]` per cable, alternating `wi`/`ri`) that
gives every module a 1-sample-delayed view of all its inputs regardless of
processing order. Any slot ordering produces correct output. This is also stated
explicitly in CLAUDE.md: *"The 1-sample cable delay means modules can run in any
order."*

The toposort and its helper function (`kahn_toposort`) add ~65 lines of code,
a non-trivial alloc-heavy build-time path, and a `VecDeque` dependency in the
builder — none of which earn their keep.

## Acceptance criteria

- [ ] `kahn_toposort` function removed from `patches-engine/src/builder.rs`
- [ ] `build_patch` iterates modules in a deterministic order that does not
      require a toposort (e.g. ascending `NodeId`)
- [ ] `BuildError::InternalError` variants introduced solely to support the
      toposort removed if no longer needed
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

**Determinism.** Replace the toposort with a sort by `NodeId` (which is already
`Ord`). `NodeId`s are assigned in insertion order so this gives a stable,
predictable execution sequence without any graph traversal.

**No behavioural change.** Because of the 1-sample delay, output is
mathematically identical regardless of module ordering. Existing tests that
assert on audio output (`tick_produces_bounded_audio_output`,
`input_scale_is_applied_at_tick_time`) will continue to pass unchanged.

**Future parallelism.** The design note in CLAUDE.md says parallelism is a
"contained change to `ExecutionPlan::tick()` and the builder's buffer layout".
A future parallel executor can identify independent modules at that point without
needing a stored toposort order; it only needs to know which buffer indices each
module reads and writes, which `ModuleSlot` already carries.
