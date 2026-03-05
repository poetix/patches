---
id: "0082"
title: Add length to ModuleShape and force re-instantiation on shape change
priority: medium
created: 2026-03-05
---

## Summary

`ModuleShape` currently only carries `channels`. Modules such as step sequencers need a
`length` field so that the correct-sized backing array can be pre-allocated at instantiation
time, avoiding reallocations when step parameters are updated at runtime. Alongside this,
`ParameterKind::Array` and `ParameterValue::Array` should each carry a `length`, and
validation should assert they match. Finally, the planner must detect when a node's
`ModuleShape` has changed between builds and, rather than reusing the old instance, tombstone
it and create a fresh one via the normal new-node path.

## Acceptance criteria

### ModuleShape

- [ ] Add `length: usize` to `ModuleShape` (default `0` for modules that don't use it).
- [ ] All existing `ModuleShape { channels }` construction sites updated to include `length: 0`
      (or an appropriate value where a module already uses a length concept).

### Array parameter length

- [ ] Add `length: usize` to `ParameterKind::Array` — the maximum (pre-allocated) number of
      steps the module supports.
- [ ] Add `length: usize` to `ParameterValue::Array` — the actual number of steps present in
      the current value.
- [ ] `validate_parameters` (in `patches-core/src/module.rs`) checks that
      `ParameterValue::Array.length <= ParameterKind::Array.length` and returns an appropriate
      `BuildError` if not.
- [ ] `StepSequencer` descriptor updated to declare a concrete `length` on its `Array`
      parameter kind, matching `ModuleShape::length`.

### Planner shape-change detection

- [ ] `NodeState` gains a `shape: ModuleShape` field so the previously-used shape is
      persisted across builds.
- [ ] In `assign_instance_ids`, the "surviving" branch additionally compares the node's
      current `shape` against `prev.shape`; if they differ, treat the node as
      type-changed: tombstone the old instance and instantiate a fresh one with the new shape.
- [ ] Integration test: build a plan with a sequencer node at `length = 4`, then rebuild with
      `length = 8` on the same `NodeId`; assert that the instance id changes across builds.

## Notes

The purpose of `ModuleShape::length` is to enable pre-allocation of step/slot arrays at
module construction time (`Module::initialise` or the factory), so that subsequent
`update_parameters` calls can write into the existing allocation rather than allocating a
new `Vec`. This upholds the no-allocation-on-audio-thread convention even when parameter
updates arrive live.

`ParameterKind::Array::length` and `ParameterValue::Array::length` mirror each other so
that the core validation layer can enforce the capacity contract without needing to know
anything about step semantics. Content validation (parsing step strings) remains the
module's responsibility in `update_validated_parameters`.

Shape-change re-instantiation fits naturally into the existing planner logic: the "surviving"
branch in `assign_instance_ids` already handles type changes by falling through to
`registry.create`. The same mechanism can be reused for shape changes, with the old instance
being tombstoned via the existing tombstone channel.

Relevant files:
- `patches-core/src/module_descriptor.rs` — `ModuleShape`, `ParameterKind`
- `patches-core/src/parameter_map.rs` — `ParameterValue`
- `patches-core/src/module.rs` — `validate_parameters`
- `patches-modules/src/step_sequencer.rs` — `StepSequencer` descriptor and `update_parameters`
- `patches-engine/src/builder.rs` — `NodeState`, `assign_instance_ids`
