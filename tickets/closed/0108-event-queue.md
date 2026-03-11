---
id: "0108"
title: EventQueue (windowed ring-buffer drain)
priority: high
created: 2026-03-11
epic: E021
---

## Summary

Introduce `EventQueue` in `patches-engine`: a thin wrapper around the existing
`rtrb` ring buffer that accepts `(target_sample: u64, MidiEvent)` pairs from
the MIDI connector thread and exposes a `drain_window` method for the audio
thread to consume events falling within a 64-sample sub-block window.

## Acceptance criteria

- [ ] `EventQueue::new(capacity: usize) -> (EventQueueProducer, EventQueueConsumer)`.
- [ ] `EventQueueProducer::push(target_sample: u64, event: MidiEvent) -> Result<(), Full>`
      — non-blocking, no allocation.
- [ ] `EventQueueConsumer::drain_window(window_start: u64, sub_block_size: u64)`
      returns an iterator of `(offset: usize, MidiEvent)` where `offset` is
      `(target_sample − window_start) as usize` clamped to `[0, sub_block_size)`.
- [ ] Events with `target_sample < window_start` are late: they are yielded with
      `offset = 0` (applied at the start of the current chunk).
- [ ] Events with `target_sample >= window_start + sub_block_size` are future:
      they remain in the buffer and are not yielded.
- [ ] Unit tests (single-threaded):
      - mix of past, current-window, and future events: correct partitioning.
      - late events yield offset 0.
      - future events are not consumed and appear in the next window.
      - empty queue returns empty iterator.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

`rtrb` is already a dependency of `patches-engine`. `MidiEvent` can be a
placeholder type (`pub struct MidiEvent { pub bytes: [u8; 3] }`) for this
ticket; it will be fleshed out when needed.

The queue does not need to be sorted by `target_sample` — events are pushed in
approximately time order by a single producer thread, and the 64-sample
granularity means out-of-order delivery within a window is rare and
inconsequential.
