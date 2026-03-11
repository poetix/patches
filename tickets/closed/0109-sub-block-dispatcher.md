---
id: "0109"
title: Wire SubBlockDispatcher into audio callback
priority: high
created: 2026-03-11
epic: E021
depends_on: ["0106", "0107", "0108"]
---

## Summary

Introduce `SubBlockDispatcher` in `patches-engine` and integrate it into
`AudioCallback`. It drives the 64-sample sub-block loop: for each chunk it
drains `EventQueue` and forwards events to the `ModulePool` slots listed in
`ExecutionPlan::midi_receiver_indices`. It also publishes the `AudioClock`
anchor after each full buffer callback.

## Acceptance criteria

- [ ] `AudioCallback` maintains a running `sample_counter: u64` incremented by
      `sub_block_size` each chunk.
- [ ] After processing each full output buffer, `AudioClock::publish` is called
      with `(sample_counter, OutputCallbackInfo::timestamp.playback)`.
- [ ] Each 64-sample chunk, `EventQueueConsumer::drain_window` is called and
      each yielded event is delivered to all pool indices in
      `ExecutionPlan::midi_receiver_indices` via `ModulePool::receive_midi`
      (introduced in T-0111; stub with a no-op pool method for this ticket if
      T-0111 is not yet merged).
- [ ] The sub-block size (64) is a named constant, not a magic number.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

The clock anchor must be published using `playback_wall_time` (when the first
sample of the buffer reaches the DAC), not the callback invocation time, so
that the MIDI connector thread's timestamp conversion stays accurate across
varying callback timing.

`HeadlessEngine` in `patches-integration-tests` should be updated to exercise
the sub-block loop if feasible without audio hardware.
