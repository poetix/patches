---
id: "E008"
title: Control-rate signalling
created: 2026-03-02
tickets: ["0037", "0038", "0041"]
---

## Summary

The audio engine currently has no way for the control thread to update live module
parameters (e.g. oscillator frequency, filter cutoff) without rebuilding the entire
`ExecutionPlan`. This epic adds a lightweight, lock-free signal path from the
control thread to individual module instances running on the audio thread, operating
at a fixed, configurable control rate that is orders of magnitude lower than the
audio sample rate.

The design follows the same lock-free boundary principle as plan swapping: an
`rtrb` ring buffer carries `(InstanceId, ControlSignal)` pairs from the control
thread to the audio callback, which drains the queue on each control tick.
Control-rate dispatch avoids any branch in the per-sample hot path by chunking
sample processing — the callback computes how many samples to the next control tick
(or end of the CPAL buffer, whichever is smaller), runs that many samples in a
tight inner loop, distributes signals, and repeats.

## Acceptance criteria

- [ ] All tickets closed.
- [ ] `cargo clippy` clean, `cargo test` green across the workspace.
- [ ] The audio thread acquires no locks and performs no heap allocation in the
      signal-distribution path.
- [ ] The frequency-sweep example compiles and runs, demonstrating audible pitch
      change driven purely by `send_signal` calls from the control thread.

## Tickets

| ID   | Title                                                   | Priority |
|------|---------------------------------------------------------|----------|
| 0037 | Add `ControlSignal` enum and `Module::receive_signal`   | high     |
| 0038 | Engine-level signal distribution with chunked control rate | high  |
| 0041 | Example: frequency sweep via control signals            | medium   |

## Notes

`ControlSignal` is a typed enum (not a stringly-typed map) so that new signal
kinds (OSC bundles, MIDI events, structured automation) can be added as variants
without breaking existing module implementations. Module implementations should use
a wildcard arm on `receive_signal` match expressions for forward compatibility.

Tickets must be worked in order: 0037 (trait infrastructure) before 0038 (engine
wiring) before 0041 (example).
