---
id: "0044"
epic: "E009"
title: Remove ModuleInstanceRegistry, held_plan, and ExecutionPlan::initialise
priority: high
created: 2026-03-02
---

## Summary

Cleanup ticket following T-0043. Remove the now-dead `ModuleInstanceRegistry` type from
`patches-core`, the `held_plan` field from `PatchEngine`, and any remaining references to
the old state-preservation machinery. Update ADR-0003 to record that it is superseded.
Close the open tickets whose designs are replaced by E007.

## Acceptance criteria

### `patches-core`
- [ ] `ModuleInstanceRegistry` struct removed from `patches-core/src/registry.rs`
      (or the file removed if it contains nothing else)
- [ ] `ModuleInstanceRegistry` removed from `patches-core/src/lib.rs` re-exports
- [ ] No remaining use of `ModuleInstanceRegistry` anywhere in the workspace

### `patches-engine`
- [ ] `PatchEngine::held_plan: Option<ExecutionPlan>` field removed
- [ ] `PatchEngine::update()` simplified: on `swap_plan` returning `Err`, return
      `PatchEngineError::ChannelFull` immediately without stashing anything
- [ ] `PatchEngine::new()` / `with_control_period()` no longer initialise `held_plan`
- [ ] Any dead imports cleaned up

### `adr/`
- [ ] `adr/0003-planner-state-freshness.md` status updated to `Superseded by ADR-0009`
      with a brief note explaining what replaced it

### Open tickets
- [ ] `tickets/open/0031-state-preservation-across-replans.md` moved to `closed/` with
      a note that it is resolved by E007
- [ ] `tickets/open/0034-held-plan-channel-full-path.md` moved to `closed/` with a note
      that the design is superseded by E007 (no implementation needed)

### General
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean
- [ ] No references to `ModuleInstanceRegistry`, `held_plan`, `into_registry`, or
      `ExecutionPlan::initialise` remain (search the workspace to confirm)

## Notes

`PatchEngineError::ChannelFull` remains valid — the caller still needs to know that a
swap was rejected. What changes is that there is no retained plan on the `PatchEngine`
side; the caller is responsible for retrying with the same or an updated graph. There is
no current design for handling rapid consecutive replans; this is an acknowledged gap,
not a regression.
