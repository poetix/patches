---
id: "0077"
title: "E015: Port connectivity awareness"
priority: medium
created: 2026-03-05
---

## Summary

Modules currently have no way to know which of their ports are wired up in the
active patch. This matters for two reasons:

1. **Performance.** A filter with an unmodulated frequency input recomputes
   coefficients every sample for no benefit. Knowing the port is unconnected lets
   the module skip that work entirely.

2. **Stereo conventions.** Stereo modules need to distinguish "right channel is
   unpatched" from "right channel is patched but silent" in order to implement the
   standard convention of mirroring the left signal into the right path when only
   the left port is connected.

The planner already has full connectivity information at plan-build time. This epic
surfaces it to modules via a `PortConnectivity` value delivered on each plan
activation, following the same pattern as `parameter_updates` (ADR 0012).

See ADR 0013 for the full design rationale.

## Tickets

- T-0078 — `PortConnectivity` type and `Module::set_connectivity`
- T-0079 — Compute connectivity in `build_patch` and call `set_connectivity` on new modules
- T-0080 — `connectivity_updates` in `ExecutionPlan`, `ModulePool::set_connectivity`, plan-swap application
- T-0081 — Integration test for connectivity notification

## Acceptance criteria

- [ ] `PortConnectivity` struct exists in `patches-core` with `inputs: Box<[bool]>` and
      `outputs: Box<[bool]>`.
- [ ] `Module::set_connectivity` exists with a default no-op implementation.
- [ ] The planner computes connectivity for each node during `build_slots` and calls
      `set_connectivity` on new modules before they are placed in `new_modules`.
- [ ] `ExecutionPlan::connectivity_updates` carries diffs for surviving modules whose
      connectivity changed between builds.
- [ ] `ModulePool::set_connectivity` applies updates during plan swap on the audio thread.
- [ ] `NodeState` stores the previous `PortConnectivity` for change detection.
- [ ] An integration test verifies that a module receives correct connectivity on first
      plan activation and receives an update when a cable is added or removed.
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

`set_connectivity` is called on the control thread for new modules and on the audio
thread for surviving modules (during plan swap). Implementations must not allocate
or block. The default no-op satisfies this; it is documented on the trait.
