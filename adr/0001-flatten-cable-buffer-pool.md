---
id: "ADR-0001"
title: Flatten cable buffer pool with a global write phase
date: 2026-02-28
status: accepted
---

## Context

Each connection (patch cable) between modules uses a `SampleBuffer`: a 2-element ring buffer (`[f32; 2]`) with a `write_index` that toggles between 0 and 1 after each engine tick. The `ExecutionPlan` stores these in a `Vec<SampleBuffer>`, and module slots hold indices into the vec.

All buffers advance in lockstep — the engine calls `advance()` on every buffer at the end of each tick, toggling each one's `write_index` to the same value. This means N copies of the same bit of state are being stored and updated individually.

Each `SampleBuffer` is 24 bytes (`[f32; 2]` + `usize`). The interleaved `write_index` fields reduce cache density by ~33% and the advance loop is O(N) for what is logically a single bit flip.

## Decision

Replace `Vec<SampleBuffer>` with a flat `Vec<[f32; 2]>` and a single `write_phase: bool` on `ExecutionPlan`. Remove the `SampleBuffer` type.

```rust
pub struct ExecutionPlan {
    pub slots: Vec<ModuleSlot>,
    pub buffer_data: Vec<[f32; 2]>,  // one pair per cable
    pub write_phase: bool,           // single global toggle
    pub audio_out_index: usize,
}

// Read (previous tick):
buffer_data[idx][!write_phase as usize]

// Write (current tick):
buffer_data[idx][write_phase as usize]

// Advance:
write_phase = !write_phase;
```

Module slots continue to hold indices into the pool. No module code changes.

## Consequences

**Positive:**
- Denser packing: 16 bytes per cable instead of 24.
- O(1) advance instead of O(N).
- `SampleBuffer` as a standalone type is eliminated — one fewer abstraction.

**Negative:**
- Read/write operations on the pool are slightly less self-documenting than `buffer.read()` / `buffer.write(value)`. Mitigate with helper methods on `ExecutionPlan` or a thin inline wrapper.

**Parallelism implications:**
- The double-buffered design means reads and writes target different slots. This is safe for concurrent access across threads without synchronisation: each write slot is owned by exactly one module (one thread), and the read slot is immutable during processing.
- However, tightly packed `[f32; 2]` pairs (4 per 64-byte cache line) create **false sharing** risk when adjacent buffers are accessed by different cores. This is not a problem for single-threaded execution but must be addressed when parallelism is introduced.
- Mitigation (deferred): the builder should partition buffers by thread affinity so that buffers accessed by the same thread are contiguous, with cache-line padding between partitions. The index indirection makes this reordering invisible to modules.

## Supersedes

The `SampleBuffer` type introduced in ticket 0001.

## Implemented in

Ticket 0015 (miscellaneous improvements, epic E002).
