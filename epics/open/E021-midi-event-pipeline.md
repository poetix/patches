---
id: "E021"
title: Sample-accurate MIDI event pipeline
created: 2026-03-11
updated: 2026-03-11
tickets: ["0106", "0107", "0108", "0109", "0110", "0111", "0112"]
---

## Summary

End-to-end pipeline for delivering MIDI events from attached devices to modules
with sub-millisecond timing accuracy. Events are timestamped against the audio
device's sample clock and scheduled to the nearest 64-sample sub-block, giving
≈1.33 ms granularity at 48 kHz — below the perceptual JND for rhythmic timing
in all practical musical contexts.

The design decomposes into independently testable components (clock anchor,
scheduler, queue, dispatcher) that are wired into the audio callback last.
Module MIDI reception is opt-in via a dedicated trait rather than a default
no-op on `Module`, allowing the planner to identify and route events only to
modules that declare interest.

## Design

### Timing model

The audio device's sample clock is the master timebase. After each CPAL
callback, the audio thread publishes a clock anchor: the pair
`(sample_count, playback_wall_time)` where `playback_wall_time` is the
wall-clock instant at which `sample_count` will reach the DAC
(`OutputCallbackInfo::timestamp.playback`).

When a MIDI event arrives on the connector thread, it reads the anchor and
computes:

```
target_sample = sample_count
              + (event_wall_time − playback_wall_time) × sample_rate
              + lookahead_samples
```

`lookahead_samples` (default: 128 = two sub-blocks ≈ 2.7 ms at 48 kHz) absorbs
thread-scheduling jitter and ensures the audio thread always sees events before
their target position. It adds a fixed, bounded latency that is below the
perceptual threshold.

### Sub-block dispatch

The audio callback processes the output buffer in 64-sample chunks. After each
chunk, it drains the event queue for events whose `target_sample` falls within
`[current_sample, current_sample + 64)`. Late events (`target < current_sample`)
are clamped to offset 0 of the current chunk and logged.

### Clock anchor publication

The anchor is a two-field struct that cannot be written atomically on common
architectures. A seqlock is used: the writer increments a sequence counter to
odd before writing and back to even after; readers retry if the counter changes
during their read. One writer (audio thread), one reader (MIDI connector thread);
no blocking on either side.

### Module opt-in

A separate `ReceivesMidi` trait carries `receive_midi(&mut self, event:
MidiEvent)`. `Module` gains a default `as_midi_receiver` method returning
`None`; modules that want MIDI override it to return `Some(self)`. The planner
calls this during `build_slots` and records the pool indices of MIDI-capable
modules in `ExecutionPlan::midi_receiver_indices`. The dispatcher broadcasts
each incoming event to all indexed modules.

## Tickets

| ID   | Title                                              | Priority | Depends on       |
|------|----------------------------------------------------|----------|------------------|
| 0106 | `AudioClock` seqlock                               | high     | —                |
| 0107 | `EventScheduler` (pure stamping logic)             | high     | 0106             |
| 0108 | `EventQueue` (windowed ring-buffer drain)          | high     | —                |
| 0109 | Wire `SubBlockDispatcher` into audio callback      | high     | 0106, 0107, 0108 |
| 0110 | MIDI device connector thread                       | high     | 0109             |
| 0111 | `ReceivesMidi` trait, planner identification,      | high     | 0109             |
|      | `ExecutionPlan::midi_receiver_indices`             |          |                  |
| 0112 | `MidiMixKnobs` module                              | medium   | 0111             |

## Definition of done

- MIDI events from an attached device arrive at targeted modules within ≈1.33 ms
  quantisation (64-sample sub-blocks at 48 kHz).
- All infrastructure components (`AudioClock`, `EventScheduler`, `EventQueue`)
  have unit tests exercisable without audio hardware or live threads.
- Modules opt into MIDI via `ReceivesMidi`; the planner correctly identifies and
  indexes them in `ExecutionPlan`.
- `MidiMixKnobs` module maps CC values from an Akai MIDImix to normalised
  voltages on its output ports.
- `cargo build`, `cargo test`, `cargo clippy` clean with no new warnings.
- No `unwrap()` or `expect()` in library code.
