---
id: "0049"
epic: "E011"
title: Encapsulate signal dispatch inside ExecutionPlan
priority: medium
created: 2026-03-03
---

## Summary

The binary-search-and-dispatch logic for routing a `ControlSignal` to its target
pool slot was duplicated: once in `AudioCallback::dispatch_signals` and once in a
private test helper in `planner.rs`. Both accessed the `pub signal_dispatch` field
directly. This ticket moves the dispatch logic into `ExecutionPlan::dispatch_signal`,
makes `signal_dispatch` private, and updates all call sites.

## Acceptance criteria

- [x] `ExecutionPlan::dispatch_signal(id: InstanceId, signal: ControlSignal, pool: &mut ModulePool)` added; performs a binary search on `signal_dispatch` and calls `pool.receive_signal` on the resolved slot. No-op for unknown `id`.
- [x] `signal_dispatch` field visibility reduced from `pub` to private.
- [x] `AudioCallback::dispatch_signals` updated to call `self.current_plan.dispatch_signal(...)`.
- [x] `planner.rs` test helper `dispatch_signal` removed; tests updated to call `plan.dispatch_signal(id, signal, &mut pool)` directly.
- [x] `signal_for_unknown_id_is_silently_dropped` test updated to use `received_count` (from `receiver_graph()`) instead of the removed bool return value, making the assertion stronger.
- [x] `cargo clippy` clean, all tests pass.
