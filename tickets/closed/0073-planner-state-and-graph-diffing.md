---
id: "0073"
title: Introduce PlannerState and graph-diffing build_patch
priority: high
epic: "E014"
depends: ["0071"]
created: 2026-03-04
---

## Summary

Replace the current `build_patch` function (which consumes a `ModuleGraph` containing
live module instances) with a graph-diffing version that borrows a topology-only
`ModuleGraph`, compares it against a `PlannerState` from the previous build, and only
instantiates modules for new or type-changed nodes. The planner owns module identity —
`InstanceId` is assigned by the planner, not by module construction.

## Acceptance criteria

- [ ] A `NodeState` struct exists holding `module_name: &'static str`,
      `instance_id: InstanceId`, and `parameter_map: ParameterMap`.
- [ ] A `PlannerState` struct exists holding `nodes: HashMap<NodeId, NodeState>`,
      `buffer_alloc: BufferAllocState`, and `module_alloc: ModuleAllocState`.
- [ ] `build_patch` has the signature:
      ```rust
      pub fn build_patch(
          graph: &ModuleGraph,
          registry: &Registry,
          env: &AudioEnvironment,
          prev_state: &PlannerState,
          pool_capacity: usize,
          module_pool_capacity: usize,
      ) -> Result<(ExecutionPlan, PlannerState), BuildError>
      ```
- [ ] The graph is borrowed, not consumed.
- [ ] For each node in the new graph, the planner compares against `prev_state`:
      - **Absent → present**: assign new `InstanceId`, instantiate via `Registry::create()`.
      - **Present → absent**: tombstone the module pool slot.
      - **Present, same `module_name`**: surviving — reuse `InstanceId`, do not instantiate.
      - **Present, different `module_name`**: type change — tombstone old, instantiate new.
- [ ] Sink node is identified via `ModuleDescriptor::is_sink` (from the registry's
      `describe()`, not from a live module instance). Exactly one sink required.
- [ ] `InstanceId` is assigned by the planner (e.g. via an `AtomicU64` counter or a
      field on `PlannerState`), not by module construction.
- [ ] The returned `PlannerState` reflects the new graph's topology and assigned IDs.
- [ ] Existing unit tests for `build_patch` are updated or replaced to test the
      diffing behaviour (new, removed, surviving, type-changed nodes).
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

This ticket does **not** include parameter diffing — that is T-0074. Surviving modules
here are simply reused without parameter updates. The `ExecutionPlan` produced by this
ticket carries `new_modules` and `tombstones` as before, but the set of new modules is
smaller (only genuinely new nodes, not all nodes).

The `Planner` struct should be updated to hold `PlannerState` and delegate to the new
`build_patch`. `PlannerState::default()` (or `PlannerState::empty()`) represents the
initial state with no previous graph.

See ADR 0012 § "The planner owns module identity" and "Planning is graph diffing".
