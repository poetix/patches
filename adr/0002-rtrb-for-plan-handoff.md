---
id: "ADR-0002"
title: Use rtrb instead of triple_buffer for lock-free plan handoff
date: 2026-02-28
status: accepted
---

## Context

Ticket 0009 called for replacing `Arc<Mutex<ExecutionPlan>>` in `SoundEngine` with a
lock-free handoff so the audio thread never waits on a mutex. The ticket specified the
`triple_buffer` crate (MPLv2).

Two blockers emerged when attempting to implement it:

1. **`triple_buffer::TripleBuffer::new` requires `T: Clone`**, because it populates all
   three internal slots with clones of the initial value. `ExecutionPlan` holds
   `Box<dyn Module>` and cannot implement `Clone` without adding a `clone_box` method to
   the `Module` trait — a significant, cross-cutting change to `patches-core` and every
   module implementation.

2. **`TripleBufferOutput::read()` returns `&T` (immutable)**, but
   `ExecutionPlan::tick()` requires `&mut self`. A lower-level `output_buffer_mut()` API
   exists, but does not resolve blocker 1.

An alternative was considered: making `ExecutionPlan: Clone` by adding `clone_box` to
`Module`. This was rejected because:
- It pollutes the `Module` trait with an infrastructural concern unrelated to signal
  processing.
- It forces every future module author to implement `clone_box`, increasing the cost of
  adding modules.
- The three-copy overhead of triple buffering is unnecessary: for hot-reload, only one
  in-flight plan needs to be buffered at any time.

## Decision

Use `rtrb` (MIT OR Apache-2.0), a wait-free SPSC ring buffer designed for real-time
audio, with capacity 1.

Ownership model:
- `SoundEngine` holds an `rtrb::Producer<ExecutionPlan>` (the write/control end).
- The audio callback closure captures the `ExecutionPlan` directly as a local owned
  value, plus an `rtrb::Consumer<ExecutionPlan>` (the read end).
- Each callback, before ticking, the closure calls `consumer.pop()`. If a new plan is
  available it replaces the local plan; otherwise the existing plan continues.

```rust
// SoundEngine::new:
let (producer, consumer) = rtrb::RingBuffer::new(1);

// Audio closure captures:
//   mut current_plan: ExecutionPlan
//   mut consumer: rtrb::Consumer<ExecutionPlan>

// Audio callback (no allocation, no lock):
if let Ok(new_plan) = consumer.pop() {
    current_plan = new_plan;
}
current_plan.tick(sample_rate);

// Future SoundEngine::swap_plan (not yet implemented):
// producer.push(new_plan).ok(); // drops silently if slot still full
```

## Consequences

**Positive:**
- No `Clone` requirement on `ExecutionPlan` or `Module`.
- No `Mutex`, no `Arc` — the audio thread owns its plan outright.
- `rtrb` is wait-free on both producer and consumer paths; no system calls.
- MIT OR Apache-2.0 licence — no attribution obligations at distribution time (simpler
  than `triple_buffer`'s MPLv2).
- `swap_plan` is a one-liner when needed.

**Negative:**
- With capacity 1, if the control thread writes two plans before the audio thread
  consumes the first, `push` returns `Err` and the intermediate plan is dropped.
  For hot-reload this is acceptable: only the latest patch matters.
- The audio thread holds the plan by value inside the closure, so `stop()` can no longer
  reclaim the plan directly. The plan is dropped when the stream (and its closure) is
  dropped, which happens inside `stop()` via `self.stream.take()`. This is unchanged
  behaviour from the closure-capture model.

## Alternatives considered

- **`triple_buffer`** (MPLv2): blocked by `T: Clone` requirement; see Context above.
- **`crossbeam-channel` bounded(1)**: similar semantics to `rtrb` but brings in a
  larger dependency. `rtrb` is purpose-built for audio and has a smaller footprint.
- **`arc_swap`**: requires the shared value to be read-only (`Arc<T>`); incompatible
  with `tick(&mut self)`.

## Implemented in

Ticket 0009 (replace Mutex with lock-free plan handoff, epic E002).
