---
id: "0034"
epic: "E006"
title: Held-plan / channel-full path
priority: medium
created: 2026-03-02
---

## Summary

Simulate `swap_plan` returning a plan (channel full) and verify that module state
is preserved through the retry cycle. This exercises the `PatchEngine` held-plan
logic, which is difficult to reach in unit tests because it depends on the
single-slot lock-free channel being full at the moment of a re-plan.

## Acceptance criteria

- [ ] A test-double or seam allows the integration test to force `swap_plan` to
      reject the new plan (simulating a full channel) on the first call
- [ ] Integration test: issue a re-plan; force channel-full; tick samples on the
      old plan; allow the retry to succeed; verify the new plan activates correctly
      and module state is as expected (surviving modules retain state, removed modules
      are dropped)
- [ ] Integration test: verify that a module removed in the rejected plan is not
      dropped prematurely (it must survive until the retry succeeds and the audio
      thread accepts the new plan)
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

The held-plan path in `PatchEngine` is documented in
`adr/0003-planner-state-freshness.md`. The test seam should be the minimal change
needed to make the channel-full condition controllable from a test; prefer a
constructor parameter or wrapper type over modifying production logic.

## Closure note

Superseded by E007 (ADR-0009). The `held_plan` field and the associated
channel-full retry logic have been removed from `PatchEngine` (T-0044).
Module state preservation is now handled automatically by the audio-thread-owned
module pool, making `held_plan` and this test seam unnecessary. No implementation
of this ticket is required.
