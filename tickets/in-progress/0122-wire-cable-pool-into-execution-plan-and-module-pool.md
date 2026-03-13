---
id: "0122"
title: Wire CablePool into ExecutionPlan::tick and ModulePool::process
priority: high
created: 2026-03-12
epic: E023
depends-on: "0121"
---

## Summary

Update `patches-engine` to construct a `CablePool` at the tick boundary and
pass it down through `ExecutionPlan::tick` and `ModulePool::process`. Fix
test stub modules in `patches-engine` that implement the old `Module::process`
signature.

## Acceptance criteria

- [ ] `ExecutionPlan::tick` signature changed to:
  ```rust
  pub fn tick(&mut self, pool: &mut ModulePool, cable_pool: &mut CablePool<'_>)
  ```
- [ ] `ModulePool::process` signature changed to:
  ```rust
  pub fn process(&mut self, idx: usize, cable_pool: &mut CablePool<'_>)
  ```
- [ ] Call site in `AudioCallback` (or equivalent) constructs `CablePool::new(buffer_pool, wi)`
  before calling `tick`, and still flips `wi` after each tick as before.
- [ ] Test stub modules in `patches-engine/src/pool.rs` (`ConstSource`,
  `RecordingSink`, etc.) updated to use the new `process` signature and
  `CablePool` methods.
- [ ] `cargo test -p patches-engine` passes.
- [ ] `cargo clippy -p patches-engine` clean.
