---
id: "0074"
title: Parameter diffs in ExecutionPlan
priority: high
epic: "E014"
depends: ["0070", "0073"]
created: 2026-03-04
---

## Summary

Extend the graph-diffing planner (T-0073) to detect parameter changes on surviving
modules and carry them as diffs in the `ExecutionPlan`. The audio callback applies
parameter diffs on plan adoption via `update_validated_parameters`, which is now
infallible (T-0070). This means a parameter-only edit no longer requires module
re-instantiation.

## Acceptance criteria

- [ ] `ExecutionPlan` gains a `parameter_updates: Vec<(usize, ParameterMap)>` field,
      where each entry is `(pool_index, diff_map)`.
- [ ] For surviving modules, the planner compares the previous `ParameterMap` (from
      `PlannerState`) with the current one (from the `ModuleGraph` node). Only changed
      keys appear in `diff_map`.
- [ ] Parameter diffs are validated off the audio thread via `Module::validate_parameters`
      (or the descriptor's parameter spec) before being placed in the plan.
- [ ] The audio callback applies parameter updates on plan adoption:
      ```rust
      for (idx, params) in &new_plan.parameter_updates {
          self.module_pool.update_parameters(*idx, params);
      }
      ```
- [ ] `ModulePool` gains an `update_parameters(&mut self, idx: usize, params: &ParameterMap)`
      method that calls `module.update_validated_parameters(params)` on the module at that
      slot (infallible — no `Result` to handle on the audio thread).
- [ ] If a surviving module has no parameter changes, no entry appears in
      `parameter_updates` for that module.
- [ ] New modules (instantiated via `Registry::create()`) do **not** appear in
      `parameter_updates` — their parameters are set during construction.
- [ ] Unit tests verify:
      - Parameter-only change produces `parameter_updates` and no `new_modules`.
      - Topology change (add/remove node) still works correctly alongside parameter diffs.
      - Unchanged parameters produce an empty `parameter_updates`.
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

The `PlannerState` from T-0073 already stores the last-known `ParameterMap` per node.
This ticket adds the diff computation and the delivery mechanism.

`update_validated_parameters` is infallible (T-0070), so `ModulePool::update_parameters`
does not need error handling — it just calls through.

See ADR 0012 § "Parameter diffs are carried in the ExecutionPlan".
