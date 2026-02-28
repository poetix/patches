---
id: "0010"
title: Eliminate .unwrap() from library code
priority: high
created: 2026-02-28
depends_on: []
epic: "E002"
---

## Summary

The project convention is "No `unwrap()` or `expect()` in library code." Several `.unwrap()` calls exist in `patches-engine/src/builder.rs` (in `build_patch` and `kahn_toposort`). These are logically safe given the data flow but violate the convention and will mask bugs if assumptions break during refactoring. Replace them with proper error propagation.

## Acceptance criteria

- [ ] Zero `.unwrap()` or `.expect()` calls in non-test code across `patches-core/src/`, `patches-engine/src/`, and `patches-modules/src/`
- [ ] `BuildError` gains variant(s) to cover the cases currently handled by `.unwrap()` (e.g. internal consistency errors like missing nodes/ports in the descriptor map)
- [ ] `kahn_toposort` returns `Result<Vec<NodeId>, BuildError>` instead of `Vec<NodeId>`, propagating any unexpected state as an error
- [ ] `build_patch` propagates all errors via `?`
- [ ] Existing tests continue to pass unchanged
- [ ] `cargo clippy` is clean

## Notes

**Locations to fix (all in `patches-engine/src/builder.rs`):**

- Line 120: `graph.get_module(id).unwrap().descriptor()` — module id came from `graph.node_ids()`, so this is logically safe, but should use `.ok_or(BuildError::...)?`
- Line 147: `.position(...).unwrap()` — finding `audio_out_node` in the toposort order
- Line 190: `.position(|p| &p.name == out_name).unwrap()` — resolving output port index
- Line 206: `modules.remove(&id).unwrap()` — consuming module from map
- Lines 237–238 and 266: `.unwrap()` calls inside `kahn_toposort` on `in_degree` map lookups

**Suggested `BuildError` variant:**

```rust
BuildError::InternalError(String)
```

A single catch-all for "this should be unreachable but we don't want to panic" is sufficient. These errors indicate a bug in the builder itself, not invalid user input.
