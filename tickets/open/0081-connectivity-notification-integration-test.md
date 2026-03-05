---
id: "0081"
title: Integration test for port connectivity notification
priority: medium
epic: "E015"
depends: ["0080"]
created: 2026-03-05
---

## Summary

Add an integration test (in `patches-integration-tests`) that verifies the
end-to-end behaviour of port connectivity notification: a module receives correct
connectivity on first plan activation, and receives an update when a cable is added
or removed in a subsequent replan.

## Acceptance criteria

- [ ] A test module exists (local to the integration test, not published) that
      records each `PortConnectivity` it receives via `set_connectivity`.
- [ ] **Initial connectivity test**: build a plan with one port connected and one
      unconnected; after plan adoption verify the module recorded the correct
      `inputs` and `outputs` booleans.
- [ ] **Added cable test**: start with no connections; replan with a cable added;
      verify the module receives a `connectivity_updates` entry reflecting the new
      connection.
- [ ] **Removed cable test**: start with a cable connected; replan with it removed;
      verify the module receives a `connectivity_updates` entry reflecting the
      disconnection.
- [ ] **No spurious update test**: replan with no topology change; verify
      `connectivity_updates` is empty (the module's recorded history has no new
      entry).
- [ ] Tests run without audio hardware (use the planner and builder directly,
      bypassing `SoundEngine`/CPAL).
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

Follow the pattern established in `planner_v2.rs` for hardware-free planner
integration tests.
