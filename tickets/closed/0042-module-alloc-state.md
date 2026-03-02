---
id: "0042"
epic: "E009"
title: Introduce ModuleAllocState
priority: high
created: 2026-03-02
---

## Summary

Add `ModuleAllocState` to `patches-engine/src/builder.rs` — the control-thread mirror of
the audio thread's module pool, analogous to `BufferAllocState` for the buffer pool. This
is a purely additive change: `build_patch` and `ExecutionPlan` are unchanged in this
ticket. The struct and its unit tests establish the allocation logic before the structural
migration in T-0043.

## Acceptance criteria

- [ ] `ModuleAllocState` added to `builder.rs`:
  ```rust
  pub struct ModuleAllocState {
      /// Maps InstanceId to the pool slot index currently holding that module.
      pub pool_map: HashMap<InstanceId, usize>,
      /// Recycled slot indices available for reuse (LIFO via Vec::pop).
      pub freelist: Vec<usize>,
      /// Next index to allocate when the freelist is empty. Starts at 0
      /// (no permanent-zero slot is needed for modules).
      pub next_hwm: usize,
  }
  ```
- [ ] `ModuleAllocState` implements `Default` (empty `pool_map`, empty `freelist`,
      `next_hwm: 0`)
- [ ] A helper method (or free function) `ModuleAllocState::diff` (or equivalent logic,
      tested directly) that, given a set of `InstanceId`s for the new graph, computes:
      - **surviving** entries: InstanceIds already in `pool_map` (reuse existing index)
      - **new** entries: InstanceIds not in `pool_map` (acquire from freelist or HWM;
        fail with `ModulePoolExhausted` if `>= capacity`)
      - **tombstoned** entries: `pool_map` entries not in the new set (return to freelist)
- [ ] `BuildError::ModulePoolExhausted` variant added
- [ ] Unit tests cover:
      - Fresh alloc: all modules new, HWM advances correctly
      - Stable alloc: same InstanceIds across two builds reuse their indices
      - Tombstone + recycle: removed module's slot appears in freelist; next build reuses it
        before advancing HWM
      - `ModulePoolExhausted` returned when capacity is exceeded
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean; no existing tests broken

## Notes

This ticket intentionally makes no changes to `build_patch`, `ExecutionPlan`, or
`SoundEngine`. It is safe to merge independently as a dead-code addition (the struct is
not yet wired in). T-0043 wires it in.

The capacity check mirrors `BuildError::PoolExhausted` in `BufferAllocState`. The
capacity value will be provided by the caller (`Planner`) and should match the module pool
capacity configured in `SoundEngine`.
