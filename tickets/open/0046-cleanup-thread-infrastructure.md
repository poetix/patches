---
id: "T-0046"
title: Cleanup thread and ring buffer infrastructure
priority: medium
created: 2026-03-03
---

## Summary

Add the infrastructure required for off-thread module deallocation: an `rtrb` ring buffer
whose producer lives in the audio callback and whose consumer is drained by a new cleanup
thread. Wire both into `SoundEngine::start` and `SoundEngine::stop`. This ticket does not
yet change the tombstone loop — that is T-0047.

## Acceptance criteria

- [ ] A `rtrb::RingBuffer<Box<dyn Module>>` is created in `SoundEngine::start` with
      capacity equal to `module_pool_capacity`.
- [ ] The producer is threaded into `build_stream` alongside the existing channel
      arguments and stored inside the audio callback closure.
- [ ] A cleanup thread is spawned in `SoundEngine::start` with `std::thread::Builder` and
      the name `"patches-cleanup"`. Its body loops:
      - Pop and immediately drop any available `Box<dyn Module>` from the consumer.
      - Exit when `consumer.is_abandoned()` is true and the consumer is empty.
      - Sleep briefly (`std::thread::sleep(Duration::from_millis(1))`) when the consumer
        is empty but not yet abandoned, to avoid busy-waiting.
- [ ] `SoundEngine` stores the `JoinHandle<()>` for the cleanup thread.
- [ ] `SoundEngine::stop` joins the cleanup thread handle after dropping the stream, so all
      tombstoned modules are guaranteed to be dropped before `stop` returns. Returns
      gracefully if the handle has already been consumed (idempotent stop).
- [ ] `cargo build`, `cargo test`, `cargo clippy` all pass.

## Notes

`build_stream` already takes several arguments; add `cleanup_tx: rtrb::Producer<Box<dyn
Module>>` as the final parameter and move it into the callback closure. The producer is
`Send` because `Box<dyn Module>` is `Send` (the `Module` trait already requires `Send`).

`PendingState` in `engine.rs` does not need to include the cleanup channel — the channel
is created and the thread spawned inside `start`, not stored in `pending`.

After this ticket the cleanup producer is moved into the closure but the tombstone loop
still drops modules on the audio thread (T-0047 changes that). The producer will be unused
until T-0047; a `#[allow(unused)]` or a placeholder `let _ = cleanup_tx;` inside the
closure is acceptable for the intermediate state, provided the compiler does not warn.

The `SoundEngine::stop` join does not need a timeout — the cleanup thread exits promptly
once the producer is dropped, and the producer is dropped with the stream.
