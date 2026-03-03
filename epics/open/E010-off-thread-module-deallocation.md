---
id: "E010"
title: Off-thread module deallocation via cleanup ring buffer
priority: medium
created: 2026-03-03
---

## Summary

When the audio callback adopts a new `ExecutionPlan`, it currently drops tombstoned
`Box<dyn Module>` values inline (see `patches-engine/src/engine.rs` — the tombstone loop
inside `build_stream`). Dropping a heap-allocated value calls `dealloc`, which is not
bounded in time and violates the project's no-allocation-on-audio-thread invariant.

This epic introduces a dedicated cleanup thread and an `rtrb` ring buffer to move module
deallocation off the audio thread. The audio callback takes ownership of each tombstoned
module and pushes it to the ring buffer; the cleanup thread drains and drops modules in a
non-real-time context.

See `adr/0010-off-thread-module-deallocation.md` for the full design rationale, sizing
analysis, and alternatives considered.

## Motivation

Allocator calls (`dealloc`) on the audio thread can cause unbounded latency spikes:

- The system allocator may acquire an internal lock (e.g. `ptmalloc` arena lock) while
  another thread is allocating.
- On some OS/allocator combinations, large frees trigger page-reclamation work.
- Real-time kernels (e.g. PREEMPT_RT) cannot preempt allocator critical sections.

Hot-reload is a core feature of Patches — it is used frequently during live performance.
Each reload can remove modules, and those drops currently land on the audio callback.

## Tickets

| Ticket | Title |
|--------|-------|
| T-0051 | Cleanup thread and ring buffer infrastructure |
| T-0052 | Redirect tombstone drops to cleanup channel |
| T-0053 | Integration test: tombstoned modules dropped off the audio thread |

## Definition of done

- All three tickets closed.
- `cargo build`, `cargo test`, `cargo clippy` all clean.
- The tombstone loop in `build_stream` no longer drops any `Box<dyn Module>` directly.
- Integration test verifies that module `Drop` runs on the cleanup thread, not the audio
  callback thread.
- No new `unwrap()` or `expect()` in library code.
