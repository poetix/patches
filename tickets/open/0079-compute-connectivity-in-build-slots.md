---
id: "0079"
title: Compute port connectivity in build_slots and notify new modules
priority: medium
epic: "E015"
depends: ["0078"]
created: 2026-03-05
---

## Summary

During `build_patch` Phase 6 (`build_slots`), compute a `PortConnectivity` for
each node and call `set_connectivity` on any module that is being freshly
instantiated (i.e. present in `new_modules`). Store the computed connectivity in
`NodeState` for use in change detection on subsequent builds.

## Acceptance criteria

- [ ] `NodeState` gains a `connectivity: PortConnectivity` field.
- [ ] During `build_slots`, for each node the planner computes `PortConnectivity`
      by examining the edge list:
      - `inputs[i]` is `true` iff any edge targets port `i` of this node.
      - `outputs[j]` is `true` iff any edge originates from port `j` of this node.
- [ ] For nodes whose module is in `new_modules`, `set_connectivity` is called on
      the boxed module before it is pushed into `new_modules`.
- [ ] The computed `PortConnectivity` is stored in the updated `NodeState` for
      this node.
- [ ] `PlannerState::default()` / `PlannerState::empty()` initialises
      `NodeState::connectivity` consistently (empty all-false slices or absent,
      consistent with how a node entering the graph for the first time is treated).
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

This ticket does not yet emit `connectivity_updates` for surviving modules —
that is T-0080. After this ticket, new modules receive correct connectivity;
surviving modules do not yet receive updates on reconnection.

See ADR 0013 § "Computing connectivity in `build_patch`".
