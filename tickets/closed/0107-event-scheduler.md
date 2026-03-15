---
id: "0107"
title: EventScheduler (pure stamping logic)
priority: high
created: 2026-03-11
epic: E021
depends_on: ["0106"]
---

## Summary

Introduce `EventScheduler` in `patches-engine`: a small struct that holds
scheduling configuration (sample rate, lookahead in samples) and converts a
`(ClockAnchor, event_wall_time: Instant)` pair into a `target_sample: u64`.
Pure logic with no side effects; independently unit-testable.

## Acceptance criteria

- [ ] `EventScheduler::new(sample_rate: f32, lookahead_samples: u64) -> Self`.
- [ ] `EventScheduler::stamp(anchor: &ClockAnchor, event_wall_time: Instant) -> u64`
      returns `anchor.sample_count + elapsed_samples + lookahead_samples` where
      `elapsed_samples = (event_wall_time − anchor.playback_wall_time).as_secs_f32() * sample_rate`
      rounded to the nearest integer.
- [ ] If `event_wall_time` is before `anchor.playback_wall_time` (event arrived
      before the anchor's reference point), `elapsed_samples` may be negative;
      the result is clamped to `anchor.sample_count` (never returns a sample
      position before the anchor).
- [ ] Unit tests cover: event at anchor time, event ahead of anchor, event
      behind anchor (clamped), non-trivial lookahead offset.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

The lookahead (default suggestion: 128 samples = two 64-sample sub-blocks ≈
2.7 ms at 48 kHz) absorbs OS thread-scheduling jitter so the audio thread
reliably sees events before their target position. It should be configurable
rather than hardcoded.
