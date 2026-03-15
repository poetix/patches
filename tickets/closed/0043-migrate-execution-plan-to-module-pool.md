---
id: "0043"
epic: "E009"
title: Migrate ExecutionPlan and SoundEngine to module pool
priority: high
created: 2026-03-02
---

## Summary

The core structural migration of E009. `ModuleSlot` stops owning `Box<dyn Module>` and
instead holds a pool index. `ExecutionPlan` gains `new_modules` and `tombstones`.
`SoundEngine` pre-allocates the module pool and installs/removes modules on plan
acceptance. `build_patch` is updated to use `ModuleAllocState` (from T-0042) and no
longer takes a `registry` parameter. This is the largest change in the epic; it must
leave the codebase compiling and all tests passing before merge.

## Acceptance criteria

### `ModuleSlot`
- [ ] `module: Box<dyn Module>` field replaced by `pool_index: usize`

### `ExecutionPlan`
- [ ] Gains `new_modules: Vec<(usize, Box<dyn Module>)>` — pool index + instance for
      each module being introduced to the pool by this plan
- [ ] Gains `tombstones: Vec<usize>` — pool indices of modules removed from the graph
- [ ] `tick()` signature changes to `tick(pool: &mut [Option<Box<dyn Module>>], buffer_pool: &mut [[f32; 2]], wi: usize)` and accesses modules as `pool[slot.pool_index].as_mut().unwrap()`
- [ ] `last_left()` / `last_right()` accept the module pool by reference and access
      `pool[slots[audio_out_index].pool_index]`
- [ ] `signal_dispatch` entries map `InstanceId → pool_index` (value semantics unchanged;
      the slot is now a pool index rather than a slot-vec index)
- [ ] `ExecutionPlan::initialise()` removed — new modules are initialised before being
      placed in `new_modules` (see `SoundEngine::swap_plan` below)
- [ ] `ExecutionPlan::into_registry()` removed

### `build_patch`
- [ ] `registry: Option<&mut ModuleInstanceRegistry>` parameter removed
- [ ] `module_alloc: &ModuleAllocState` parameter added (alongside existing
      `alloc: &BufferAllocState`)
- [ ] Returns `(ExecutionPlan, BufferAllocState, ModuleAllocState)` (or a combined struct)
- [ ] Populates `new_modules` with freshly constructed instances for new pool slots
- [ ] Populates `tombstones` with pool indices of removed modules
- [ ] Surviving modules (in both old and new graph with same InstanceId) appear in neither
      list — they stay in the pool untouched

### `SoundEngine`
- [ ] `SoundEngine::new()` gains `module_pool_capacity: usize` parameter
- [ ] Pre-allocates `module_pool: Box<[Option<Box<dyn Module>>]>` of that capacity,
      all `None`
- [ ] `module_pool` is moved into the audio closure alongside `buffer_pool`
- [ ] On plan acceptance in the audio callback:
      1. Install `new_modules`: `pool[idx] = Some(module)` for each entry
      2. Process `tombstones`: `pool[idx].take()` for each entry (drops `Box<dyn Module>`)
      3. Zero `to_zero` buffer slots (unchanged)
      4. Replace `current_plan`
- [ ] `swap_plan` initialises each module in `plan.new_modules` before pushing:
      ```rust
      for (_, module) in &mut plan.new_modules {
          module.initialise(&AudioEnvironment { sample_rate: sr });
      }
      ```
      (No-op if engine not yet started — sample rate unknown)
- [ ] `tick()` and `last_left/right` calls updated to pass both pools
- [ ] `DEFAULT_POOL_CAPACITY` constant for module pool (1024 is a reasonable default)

### `Planner`
- [ ] Holds `module_alloc_state: ModuleAllocState` alongside existing `alloc_state`
- [ ] `build()` calls `build_patch` with `&self.module_alloc_state` and threads the
      returned `ModuleAllocState` forward
- [ ] `prev_plan: Option<ExecutionPlan>` parameter removed from `Planner::build()`

### Existing tests
- [ ] All unit tests in `builder.rs` and `planner.rs` updated for new signatures
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

**`tick` signature.** The two pool references (`module_pool` and `buffer_pool`) can be
passed as separate parameters or bundled in a small struct. Separate parameters are
simpler for now.

**`signal_dispatch` index meaning.** `signal_dispatch` stores `(InstanceId, usize)` pairs
used for binary search. The `usize` was previously a slot-vec index; it becomes a
pool index. The audio callback's dispatch loop changes from
`plan.slots[slot_idx].module.receive_signal(signal)` to
`pool[pool_idx].as_mut().unwrap().receive_signal(signal)`.

**`last_left` / `last_right` through pool.** These call `as_sink()` on the AudioOut
module. With the pool the call becomes:
```rust
pool[self.slots[self.audio_out_index].pool_index]
    .as_ref()
    .and_then(|m| m.as_sink())
    .map_or(0.0, |s| s.last_left())
```

**Ordering dependency.** T-0042 must be merged first. T-0044 (removing
`ModuleInstanceRegistry` and `held_plan`) can follow once this ticket is merged.
