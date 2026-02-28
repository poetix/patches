---
id: "0009"
title: Replace Mutex with rtrb in SoundEngine
priority: high
created: 2026-02-28
depends_on: []
epic: "E002"
---

## Summary

The audio callback in `SoundEngine` currently shares the `ExecutionPlan` with the
non-audio side via `Arc<Mutex<ExecutionPlan>>` and uses `try_lock` to avoid blocking.
Replace this with a lock-free design using `rtrb` (a wait-free SPSC ring buffer
designed for real-time audio).

The originally specified `triple_buffer` crate was rejected — see ADR-0002 for the
reasoning. `rtrb` (MIT OR Apache-2.0) requires no `Clone` on `ExecutionPlan` and is
wait-free on both producer and consumer paths.

## Acceptance criteria

- [x] Add `rtrb` as a dependency of `patches-engine`
- [x] `SoundEngine` holds an `rtrb::Producer<ExecutionPlan>` (the control/write end)
- [x] The audio callback closure captures the `ExecutionPlan` directly (by value) and
      an `rtrb::Consumer<ExecutionPlan>`; it uses `consumer.pop()` at the top of each
      callback to adopt a new plan if one has been written
- [x] The audio callback accesses the plan without any mutex, lock, or `try_lock`
- [x] `SoundEngine::stop()` still cleanly reclaims resources — the stream is dropped
- [x] `swap_plan(new_plan: ExecutionPlan)` is implemented; the producer end is held
      by `SoundEngine`
- [x] `cargo test` passes
- [x] `cargo clippy` is clean
- [ ] `cargo run --example sine_tone` still plays audio correctly (manual check)

## Notes

See ADR-0002 for the full rationale for choosing `rtrb` over `triple_buffer`.

**Ownership model:**

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

// Future SoundEngine::swap_plan:
// producer.push(new_plan).ok();
```

**`SoundEngine` struct:** The `plan` field becomes `rtrb::Producer<ExecutionPlan>`.
No `Arc`, no `Mutex`. The read end is moved into the audio closure at `start()` time.
