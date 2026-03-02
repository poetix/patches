---
id: "0038"
epic: "E008"
title: Engine-level control-signal distribution at configurable control rate
priority: high
created: 2026-03-02
depends_on: "0037"
---

## Summary

Add a lock-free signal path from the control thread to module instances running on
the audio thread. `SoundEngine` gains a `send_signal` method that enqueues
`(InstanceId, ControlSignal)` pairs into an `rtrb` ring buffer (capacity 64). The
audio callback distributes queued signals to modules at a configurable control rate
using a chunked loop that avoids any branch in the per-sample hot path.

## Acceptance criteria

- [ ] `SoundEngine::new` gains a `control_period: usize` parameter (number of
      samples between control ticks). `PatchEngine::new` propagates this; a
      sensible default (e.g. 64) is used where the caller doesn't need to configure
      it — expose `PatchEngine::with_control_period(graph, period)` if helpful.
- [ ] `SoundEngine` holds an `rtrb::Producer<(InstanceId, ControlSignal)>` (ring
      buffer capacity 64). The corresponding `Consumer` is moved into the audio
      callback closure.
- [ ] `SoundEngine::send_signal(&mut self, id: InstanceId, signal: ControlSignal)
      -> Result<(), ControlSignal>` pushes onto the ring buffer. Returns `Err(signal)`
      if the buffer is full (wait-free; caller decides whether to drop or retry).
- [ ] `PatchEngine::send_signal(&mut self, id: InstanceId, signal: ControlSignal)
      -> Result<(), ControlSignal>` delegates to `SoundEngine`.
- [ ] The audio callback's `fill_buffer` (or equivalent) is refactored to a
      **chunked loop**. State that persists across callbacks:
      - `samples_until_next_control: usize` — initialised to `control_period`.
      - `wi_counter: usize` — write-slot index, replaces the fixed-stride `[0, 1]`
        pair loop.

      Each callback iteration:
      ```
      remaining = frames
      while remaining > 0:
          chunk = min(samples_until_next_control, remaining)
          for _ in 0..chunk:                           // tight inner loop, no branch
              plan.tick(pool, wi_counter % 2)
              write output sample
              wi_counter += 1
          samples_until_next_control -= chunk
          remaining -= chunk
          if samples_until_next_control == 0:
              drain signal ring buffer
              for each (id, signal):
                  binary-search signal_dispatch for id → slot index
                  if found: call plan.slots[index].module.receive_signal(signal)
              samples_until_next_control = control_period
      ```
      The signal drain and module lookup only happen at control ticks, never inside
      the inner `chunk` loop.

- [ ] When a new plan is adopted (ring buffer pop in the callback), the
      `wi_counter` continues uninterrupted; `samples_until_next_control` is
      unchanged (the counter is shared across plan swaps).
- [ ] `ExecutionPlan` includes a `signal_dispatch: Box<[(InstanceId, usize)]>` field
      — a sorted array mapping `InstanceId` to slot index. Built by the `Planner`
      at plan construction time (allocation happens off the audio thread). The
      audio callback uses `signal_dispatch.binary_search_by_key(&id, |(k, _)| *k)`
      to locate the target slot in O(log M) per message.
- [ ] Tests (non-audio, using direct `ExecutionPlan` calls):
      - A signal sent via a mock producer is delivered to the correct module on the
        next control tick and not before.
      - Signals for an `InstanceId` not present in the current plan are silently
        dropped.
      - A full ring buffer causes `send_signal` to return `Err` without panicking.
- [ ] `cargo clippy` clean, `cargo test` green across the workspace.

## Notes

**Ring buffer capacity (64):** At a 64-sample control period and 48 kHz, the
control rate is 750 Hz (~1.3 ms per tick). An OSC controller at 100 messages/sec
would produce at most 1 message every ~7 ticks, well within a 64-slot buffer.
Increase if profiling shows saturation.

**Module lookup at control rate:** The `signal_dispatch` sorted array is built by
the `Planner` at plan construction time, so no allocation occurs on the audio
thread. Binary search gives O(log M) per message — cache-friendly and
branch-predictor-friendly at typical module counts (tens to low hundreds). A
`HashMap` would give O(1) amortised but with worse constant factors at small M;
revisit only if module counts reach thousands.

**`send_signal` takes `&mut self`:** `rtrb::Producer::push` requires `&mut`. This
means signal sending is not callable concurrently from multiple threads. If
multi-sender support is needed in future, wrap the producer in a `Mutex` on the
control-thread side (not the audio-thread side — no mutex touches the audio path).

**`wi_counter` vs. the existing `[0, 1]` pair loop:** The current `fill_buffer`
hard-codes pairs `for wi in [0, 1]`. Replacing this with a `wi_counter` that
increments monotonically (modulo 2) preserves the ping-pong semantics while
allowing arbitrary chunk sizes.

**`control_period = 0` is illegal;** assert or return an error in `SoundEngine::new`.
