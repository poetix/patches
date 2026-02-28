---
id: "0009"
title: Replace Mutex with triple-buffer in SoundEngine
priority: high
created: 2026-02-28
depends_on: []
epic: "E002"
---

## Summary

The audio callback in `SoundEngine` currently shares the `ExecutionPlan` with the non-audio side via `Arc<Mutex<ExecutionPlan>>` and uses `try_lock` to avoid blocking. While `try_lock` does not sleep on contention, `Mutex` implementations may still involve system calls and memory barriers, and any future code that holds the lock on the control side will cause the audio thread to output silence for the duration. Replace the `Mutex` with the `triple_buffer` crate to provide a truly lock-free handoff that satisfies the project's real-time audio constraints.

## Acceptance criteria

- [ ] Add `triple_buffer` as a dependency of `patches-engine` (ask before adding)
- [ ] `SoundEngine` stores the `ExecutionPlan` in a `triple_buffer::TripleBuffer`, with the audio callback holding the output (read) end and the control side holding the input (write) end
- [ ] The audio callback (`fill_buffer`) accesses the plan without any mutex, lock, or `try_lock`
- [ ] `SoundEngine::stop()` still cleanly reclaims resources — the stream is dropped, and the engine is restartable
- [ ] A future `swap_plan(new_plan: ExecutionPlan)` method is straightforward to add (the triple-buffer write end is held by `SoundEngine`) — this method does not need to be implemented in this ticket, but the design should not preclude it
- [ ] `cargo test` passes
- [ ] `cargo clippy` is clean
- [ ] `cargo run --example sine_tone` still plays audio correctly

## Notes

**`triple_buffer` API sketch:**

```rust
let (mut write, read) = triple_buffer::TripleBuffer::new(&plan).split();
// Audio thread (owns `read`):
let plan = read.read();
plan.tick(sample_rate);
// Control thread (owns `write`):
write.write(new_plan);
```

The `read` end never blocks; it always sees the most recently published plan (or the current one if nothing new has been written). This is exactly the semantics needed for hot-reload.

**Ownership change:** The audio callback closure currently captures `Arc<Mutex<ExecutionPlan>>`. After this change it captures the `triple_buffer::Output<ExecutionPlan>` directly — no `Arc`, no `Mutex`.

**`SoundEngine::new` signature** may change since the engine no longer needs to wrap the plan in an `Arc`. The `plan` field becomes the `triple_buffer::Input<ExecutionPlan>` (write end), and the read end is moved into the audio closure at `start()` time.
