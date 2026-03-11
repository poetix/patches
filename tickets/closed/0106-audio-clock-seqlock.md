---
id: "0106"
title: AudioClock seqlock
priority: high
created: 2026-03-11
epic: E021
---

## Summary

Introduce `AudioClock` in `patches-engine`: a seqlock that lets the audio
thread publish a `(sample_count: u64, playback_wall_time: Instant)` anchor
after each CPAL callback, and lets the MIDI connector thread read a consistent
snapshot without blocking either side.

## Acceptance criteria

- [ ] `AudioClock::publish(sample_count: u64, playback_wall_time: Instant)`
      can be called from the audio thread without allocating or blocking.
- [ ] `AudioClock::read() -> ClockAnchor` returns a consistent pair; if a write
      is in progress the reader spins until it obtains a clean read.
- [ ] `ClockAnchor` is a plain struct: `{ sample_count: u64, playback_wall_time: Instant }`.
- [ ] Unit tests (single-threaded, no audio hardware):
      - publish then read returns the published values.
      - a torn read is never observed (simulate by verifying sequence parity
        logic in isolation).
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

A seqlock maintains a `sequence: AtomicU64` alongside the data. The writer
increments it to odd before writing and back to even after. The reader checks
that the sequence is even and unchanged across its read; retries otherwise. This
gives wait-free writes and obstruction-free reads with no heap allocation.

The two-field struct cannot be written atomically on common architectures
(128-bit atomic store is not universally available), making a seqlock the
appropriate primitive here.

Lives in `patches-engine` (not `patches-core`) as it is specific to the audio
callback timing model.
