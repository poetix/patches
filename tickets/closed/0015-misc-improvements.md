---
id: "0015"
title: Miscellaneous small improvements
priority: low
created: 2026-02-28
depends_on: ["0011", "0012", "0013"]
epic: "E002"
---

## Summary

A collection of minor improvements that individually don't warrant their own tickets but collectively tighten up the codebase. Each item is independent and can be applied in any order within this ticket.

## Acceptance criteria

### Fuse tick loops (efficiency)

- [ ] `ExecutionPlan::tick` fuses phases 1–3 (read inputs, process, write outputs) into a single per-slot loop, keeping phase 4 (advance all buffers) separate
- [ ] Behaviour is identical — existing tick tests pass without modification

### NodeId privacy (API hygiene)

- [ ] `NodeId`'s inner `usize` field is private (not directly constructible outside `patches-core`)
- [ ] Graph tests that construct `NodeId(99)` for the "unknown node" error path are refactored to use a stale id (add then remove a module)

### Edge indexing (efficiency)

- [ ] `ModuleGraph` indexes edges by destination `(NodeId, port_name)` for O(1) duplicate-input checks in `connect`, replacing the current linear scan
- [ ] `disconnect` and `remove_module` remain correct with the new indexing

### Trim re-exports from `patches-modules` (organisation)

- [ ] `patches-modules/src/lib.rs` only re-exports types that downstream consumers of modules actually need (e.g. `Module`, `ModuleDescriptor`), not all of `patches-core`
- [ ] Or: remove the re-exports entirely, requiring consumers to depend on `patches-core` directly (preferred — it's already a workspace member)

### Flatten cable buffer pool (efficiency, parallelism-readiness)

- [ ] Replace `Vec<SampleBuffer>` in `ExecutionPlan` with a flat `Vec<[f32; 2]>` and a single `write_phase: bool` (see [ADR-0001](../../adr/0001-flatten-cable-buffer-pool.md))
- [ ] Remove the `SampleBuffer` type from `patches-core`
- [ ] Update builder to allocate into the flat pool
- [ ] Add helper methods or inline accessors for read/write clarity

### Rename Crossfade to Mix (clarity)

- [ ] `Crossfade` renamed to `Mix` (or `Average`) throughout — the module does a fixed 50/50 blend, not a variable crossfade
- [ ] Module file renamed from `crossfade.rs` to `mix.rs`
- [ ] `sine_tone` example updated

### Add `as_any_mut` to Module trait (future-proofing)

- [ ] `Module` trait gains `fn as_any_mut(&mut self) -> &mut dyn std::any::Any` with implementations on all modules
- [ ] No callers required in this ticket — this is forward preparation for mutable downcasting (e.g. parameter changes on a running module)

### Verification

- [ ] `cargo test` passes
- [ ] `cargo clippy` is clean
- [ ] `cargo run --example sine_tone` still works

## Notes

**Depends on 0011, 0012, 0013** because those tickets change the types and traits that several of these items touch. Landing them first avoids merge conflicts.

**Each sub-item can be a separate commit** for clean review.
