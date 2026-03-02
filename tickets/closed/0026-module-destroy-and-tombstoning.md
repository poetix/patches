---
id: "0026"
epic: "E005"
title: Module::destroy and tombstoning on removal
priority: medium
created: 2026-03-01
status: won't-implement
closed: 2026-03-02
---

## Summary

When a module is removed from the patch graph during a re-plan, it currently
disappears silently: its instance is consumed from the registry, not matched in
the new graph, and dropped. There is no mechanism for a module to release resources
that cannot safely be dropped on whichever thread happens to hold the last reference.

This ticket adds `Module::destroy(&mut self)` as an optional lifecycle hook (default
no-op) and introduces tombstoning: `build_patch` identifies modules in the old
registry that were not claimed by any module in the new graph and returns their
`InstanceId`s as tombstoned. `PatchEngine` removes them from held state immediately
and schedules `destroy()` on a cleanup thread, after the audio thread has accepted
the new plan and can no longer be accessing the old instances.

The planner function itself does not call `destroy()` or mutate any module; it only
returns the set of tombstoned IDs, keeping it pure.

## Decision

Won't implement. See `adr/0007-no-module-destroy-hook.md` for the full reasoning.

In short: the premise that modules need explicit teardown on a specific thread is
false given the current ownership model. Tombstoned modules are always extracted from
the *held plan* (not from the audio thread's running plan), so they are owned
exclusively by the control thread at the point of removal. Rust's `Drop` runs on that
thread and is sufficient for all resource cleanup. Adding `destroy()` and a cleanup
thread solves a problem that does not exist, at the cost of permanent API surface and
complexity.

## Acceptance criteria (not implemented)

- [ ] `Module` trait gains `fn destroy(&mut self) {}` with a default no-op; all
      existing module implementations require no changes
- [ ] `build_patch` return type extended to include `tombstoned: Vec<InstanceId>` —
      the IDs of modules present in the old `BufferAllocState`'s registry (or the
      passed-in `ModuleInstanceRegistry`) that were not claimed by the new graph
- [ ] `Planner::build` surfaces `tombstoned` to the caller
- [ ] `PatchEngine::update` removes tombstoned modules from any held state and
      sends them (as `Box<dyn Module>`) to a cleanup thread via a channel
- [ ] The cleanup thread calls `module.destroy()` on each received module before
      dropping it
- [ ] The cleanup thread is spawned once at `PatchEngine` construction and shut
      down cleanly on `PatchEngine::stop`
- [ ] Unit test: build a graph with module M; re-plan to a graph without M; assert
      `destroy()` is called on M (use a flag or counter in a test-only module impl)
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

**Why a cleanup thread, not the control thread directly?** When `PatchEngine::update`
sends a new plan to the audio thread, the audio thread may not yet have accepted it.
The previous plan — which still holds the old module instances — may be running.
Calling `destroy()` immediately on the control thread would be safe for the
tombstoned modules (they are already out of the registry), but the cleanup thread
pattern makes the timing explicit and provides a natural place to extend with
additional teardown logic in future (e.g. waiting for a module's background I/O to
finish).

**Tombstoned vs. still-running.** The tombstoned modules are extracted from
`PatchEngine`'s *held plan*, not from the audio thread's running plan. By the time
the next `update()` call processes them, the audio thread is running the plan that
replaced the held plan. Sending tombstoned modules to the cleanup thread at this
point is safe.

**destroy() must not block the cleanup thread indefinitely.** Modules with
long-running teardown (e.g. waiting for a network socket to close) should manage
their own background tasks and keep `destroy()` itself non-blocking from the
cleanup thread's perspective.

**Module does not know it is tombstoned.** The planner identifies tombstoned IDs
purely from the set difference between old and new graphs. No flag is set on the
module itself; `destroy()` is called without the module having any awareness of
replanning.
