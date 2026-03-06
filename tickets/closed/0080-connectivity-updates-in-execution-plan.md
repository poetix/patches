---
id: "0080"
title: connectivity_updates in ExecutionPlan and plan-swap application
priority: medium
epic: "E015"
depends: ["0079"]
created: 2026-03-05
---

## Summary

Extend `ExecutionPlan` with a `connectivity_updates` field for surviving modules
whose port connectivity changed between builds. Add `ModulePool::set_connectivity`
and apply the updates in the audio callback during plan adoption, mirroring the
existing `parameter_updates` mechanism.

## Acceptance criteria

- [ ] `ExecutionPlan` gains `connectivity_updates: Vec<(usize, PortConnectivity)>`,
      where each entry is `(pool_index, new_connectivity)`.
- [ ] During `build_slots`, for each surviving node the planner compares the newly
      computed `PortConnectivity` against `NodeState::connectivity`. If they differ,
      an entry is pushed to `connectivity_updates` and `NodeState::connectivity` is
      updated.
- [ ] `ModulePool` gains:
      ```rust
      pub fn set_connectivity(&mut self, idx: usize, conn: PortConnectivity) {
          if let Some(m) = self.modules[idx].as_mut() {
              m.set_connectivity(conn);
          }
      }
      ```
- [ ] The audio callback applies `connectivity_updates` during plan adoption,
      after installing new modules and applying parameter updates:
      ```rust
      for (idx, conn) in plan.connectivity_updates.drain(..) {
          pool.set_connectivity(idx, conn);
      }
      ```
- [ ] When connectivity is unchanged between builds, `connectivity_updates` is
      empty and no extra work occurs on the audio thread.
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

`connectivity_updates` is drained (not iterated) so the `PortConnectivity` values
are moved into the modules and the `Vec` allocation is dropped at plan-swap time.
This is the same pattern used for `parameter_updates` and is consistent with the
plan-deallocation discussion in ADR 0012 § "Plan deallocation".

See ADR 0013 § "`connectivity_updates` in `ExecutionPlan`" and
"Applying updates on plan adoption".
