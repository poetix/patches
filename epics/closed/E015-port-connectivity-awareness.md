---
id: "E015"
title: Port connectivity awareness
created: 2026-03-05
adr: "0013"
tickets: ["0078", "0079", "0080", "0081"]
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

| ID   | Title                                                                     | Priority | Depends on |
|------|---------------------------------------------------------------------------|----------|------------|
| 0078 | `PortConnectivity` type and `Module::set_connectivity`                    | medium   | —          |
| 0079 | Compute connectivity in `build_patch` and notify new modules              | medium   | 0078       |
| 0080 | `connectivity_updates` in `ExecutionPlan`, `ModulePool`, plan-swap apply  | medium   | 0079       |
| 0081 | Integration test for connectivity notification                            | medium   | 0080       |

## Definition of done

- `PortConnectivity` struct exists in `patches-core` with `inputs: Box<[bool]>` and
  `outputs: Box<[bool]>`, indexed to match `ModuleDescriptor` port order.
- `Module::set_connectivity` exists with a default no-op implementation, documented
  as audio-thread-safe (no allocation, no blocking).
- The planner computes connectivity for each node during `build_slots` and calls
  `set_connectivity` on new modules before they are placed in `new_modules`.
- `NodeState` stores `connectivity: PortConnectivity` for change detection across
  successive builds.
- `ExecutionPlan::connectivity_updates: Vec<(usize, PortConnectivity)>` carries
  updates only for surviving modules whose connectivity changed.
- `ModulePool::set_connectivity` applies updates during plan swap on the audio thread,
  using `if let Some(m)` — no `unwrap()`.
- An integration test verifies initial connectivity, add-cable, remove-cable, and
  no-spurious-update cases without audio hardware.
- `cargo build`, `cargo test`, `cargo clippy` clean with no new warnings.
- No `unwrap()` or `expect()` in library code.
