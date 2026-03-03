---
id: "0045"
epic: "E009"
title: Update integration tests for module pool
priority: high
created: 2026-03-02
---

## Summary

Update `patches-integration-tests` for the module pool design and add the state
preservation integration test that was previously impossible. `HeadlessEngine` must be
updated to manage both pools. The `DropSpy` drop-timing tests need revisiting because the
point at which a removed module drops has changed: it now drops when the audio thread
processes the new plan's tombstone list, not when `current_plan` is replaced.

## Acceptance criteria

### `HeadlessEngine` updates
- [ ] `HeadlessEngine` gains a module pool (`Vec<Option<Box<dyn Module>>>` of fixed
      capacity) alongside the existing buffer pool
- [ ] `adopt_plan` updated to mirror the new audio-callback plan-acceptance sequence:
      1. Install `plan.new_modules` into the module pool
      2. Process `plan.tombstones` (`pool[idx].take()`)
      3. Zero `plan.to_zero` buffer slots
      4. Replace `self.plan`
- [ ] `tick` and `last_left/right` calls pass both pools

### Drop timing tests
- [ ] `replan_drops_removed_module` updated: `DropSpy` must still be alive after
      `Planner::build` AND before `adopt_plan`; it must be dead after `adopt_plan`
      (drop now occurs during tombstone processing inside `adopt_plan`, not on
      `current_plan` replacement — the effect on the test assertion is the same, but
      the mechanism is now tombstone-driven rather than plan-drop-driven)
- [ ] Test comment updated to reflect the new mechanism

### State preservation tests in `state_preservation.rs`

Two tests in `state_preservation.rs` are already written for the post-E009 behaviour and
marked `#[ignore]`:

- `replan_preserves_state_for_surviving_instance` — currently fails under the pre-E009
  design (prev_plan=None produces a fresh module); must pass after the module pool lands.
- `replan_fresh_instance_starts_from_default_state` — correct behaviour is unchanged,
  but the test uses the old `tick`/`slot.module` API that T-0043 removes.

- [ ] Both tests re-enabled (remove `#[ignore]`)
- [ ] API calls updated for the module pool (`tick` signature, counter access via pool
      rather than `slot.module`)
- [ ] Both tests pass without passing `prev_plan` to `Planner::build`

### General
- [ ] All existing integration tests pass with no behaviour changes other than those
      described above
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

**Drop timing mechanism change.** Under the old design, removed modules dropped on the
audio thread when `current_plan = new_plan` caused the old plan (and its `ModuleSlot`
owners) to be dropped. Under the new design, removed modules are in the pool, and drop
when `adopt_plan` processes `plan.tombstones` via `pool[idx].take()`. The observable
test result — "module is dead after `adopt_plan`" — is the same; only the comment
explaining *why* needs updating.

**`Counter` module.** The existing `Counter` stub in `patches-engine/src/planner.rs`
tests can be moved to or re-declared in the integration test crate. Reuse the same
`InstanceId`-sharing pattern already used in `planner_reuses_module_instance_across_rebuild`.

**`HeadlessEngine` initialisation.** New modules arrive in `plan.new_modules` already
initialised (done by `Planner::build` with the sample rate). `HeadlessEngine` can supply
a fixed test sample rate (e.g. 44100.0) to `Planner` or call `module.initialise()` in
`adopt_plan` before installing — whichever matches the `SoundEngine` implementation
chosen in T-0043.
