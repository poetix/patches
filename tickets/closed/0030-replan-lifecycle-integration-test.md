---
id: "0030"
epic: "E006"
title: Replanning lifecycle integration test
priority: medium
created: 2026-03-02
closed: 2026-03-02
---

## Summary

Implement the first integration tests for the replanning lifecycle: verifying that
modules removed from the graph are dropped at the correct moment, and that freed and
newly allocated buffer pool slots are zeroed before the new plan's first tick.

These tests also establish the `patches-integration-tests` crate and the
`HeadlessEngine` / `DropSpy` test fixtures used by subsequent integration tests in
the epic.

## Acceptance criteria

- [x] `patches-integration-tests` crate added to the workspace (`publish = false`,
      `[[test]]` target, no library source)
- [x] `patches-engine` dev-dependencies restored to their pre-epic state (no
      spurious `patches-core` entry)
- [x] `HeadlessEngine` fixture implemented, mirroring the CPAL audio-callback
      plan-swap sequence:
      - Zero `to_zero` slots
      - `initialise` all modules in the new plan
      - Replace `self.plan` (dropping the old plan and its modules)
      - No method to extract the active plan (enforces the real thread-boundary)
- [x] `DropSpy` module implemented: outputs `1.0`, sets `Arc<AtomicBool>` on drop
- [x] `replan_drops_removed_module` — asserts `DropSpy` is alive after
      `Planner::build` and dead after `adopt_plan` (not during build)
- [x] `replan_zeroes_freed_buffer_slot` — asserts the freed slot is non-zero before
      adoption and `[0.0; 2]` immediately after
- [x] `replan_zeroes_newly_allocated_slot` — pre-contaminates the new slot with a
      sentinel, asserts it is zeroed by `adopt_plan`
- [x] All three tests call `Planner::build(graph, None)` (no `prev_plan`), matching
      the real engine flow where the old plan is never accessible to the control thread
- [x] `CLAUDE.md` updated with `patches-integration-tests` in the workspace layout
- [x] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

**Drop timing.** The critical design point: `DropSpy` must still be alive after
`Planner::build` returns. In production the old plan is owned by the audio callback;
the control thread builds the new plan with `prev_plan = None` and never touches the
running modules. Drop occurs when the audio thread executes `current_plan = new_plan`,
i.e. inside `adopt_plan` in the test fixture.

**`Planner::build` with `prev_plan = None`.** The `Planner` carries
`BufferAllocState` internally across calls, so stable buffer index allocation and
correct `to_zero` generation work correctly even when no old plan is passed. Passing
`prev_plan` is only needed for module state preservation (oscillator phase, etc.),
which is a separate concern.

**`take_plan` not exposed.** An earlier draft of `HeadlessEngine` included a
`take_plan` method modelled on the held-plan retry path. It was removed: the only
case where the control thread legitimately holds an old plan is when `swap_plan`
returns it due to a full channel — in that scenario the plan was built by the control
thread and never entered the engine. Extracting a plan from within the engine would
model a boundary violation that cannot occur in production.
