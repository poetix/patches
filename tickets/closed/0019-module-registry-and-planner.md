---
id: "0019"
title: Module instance registry, Planner, and PatchEngine
priority: high
created: 2026-02-28
---

## Summary

Introduce the infrastructure for stateful re-planning: `ModuleInstanceRegistry`
(a map from `InstanceId` to `Box<dyn Module>`), `ExecutionPlan::into_registry`,
an updated `build_patch` that accepts an optional registry to reuse module instances,
a pure `Planner` struct for testable plan building with state preservation, and a
`PatchEngine` that coordinates planning and audio with the correct ownership model.

## Acceptance criteria

- [ ] `ModuleInstanceRegistry` added to `patches-core` with `new`, `insert`, `take`,
      `instance_ids`, `is_empty`, and `Default` impl
- [ ] `ExecutionPlan::into_registry(self) -> ModuleInstanceRegistry` implemented
- [ ] `build_patch` signature updated to accept `Option<&mut ModuleInstanceRegistry>`;
      when Some, reuses registry instances where InstanceId matches the graph module
- [ ] All existing `build_patch` call sites pass `None` (behaviour unchanged)
- [ ] `Planner` struct added to `patches-engine` with `build(graph, prev_plan)` method
- [ ] `PatchEngine` struct added to `patches-engine` with `new`, `start`, `update`,
      and `stop` methods
- [ ] Unit test: build plan A, tick it N times; call `planner.build(graph, Some(plan_A))`;
      assert the stateful module (e.g. `SineOscillator`) has the same phase in plan B
- [ ] `Planner` and `PatchEngine` exported from `patches-engine`
- [ ] ADR-0003 written documenting the one-generation-behind state freshness trade-off
- [ ] `cargo clippy` clean, all tests passing

## Notes

Part of epic E003. See plan for the ownership model rationale (PatchEngine holds
the "held plan" one generation behind the engine to avoid any engine→control
return channel). `Planner` is intentionally stateless and testable without hardware.
