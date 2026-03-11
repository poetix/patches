---
id: "0110"
title: MIDI device connector thread
priority: high
created: 2026-03-11
epic: E021
depends_on: ["0109"]
---

## Summary

Introduce a MIDI connector in `patches-engine` (or `patches-player`) that opens
attached MIDI input ports using `midir`, receives events on a dedicated thread,
and pushes them into `EventQueue` with sample-accurate target positions computed
via `AudioClock` + `EventScheduler`.

## Acceptance criteria

- [ ] `MidiConnector::open(clock: Arc<AudioClock>, queue: EventQueueProducer,
      scheduler: EventScheduler) -> Result<MidiConnector, MidiError>` opens all
      available MIDI input ports.
- [ ] Incoming MIDI events are timestamped with `Instant::now()` as close to
      receipt as possible, then `EventScheduler::stamp` is called and the result
      pushed to `EventQueue`.
- [ ] `MidiConnector::close()` (or `Drop`) cleanly joins the connector thread.
- [ ] If `EventQueue` is full, the event is dropped and a count of dropped
      events is tracked (accessible for diagnostics; no allocation or blocking).
- [ ] `cargo build`, `cargo clippy` clean.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

`midir` is not yet a dependency — confirm with the team before adding it to
`Cargo.toml`. It is a pure Rust crate with no unsafe code and minimal
transitive dependencies.

Port selection policy (all ports vs. named port) can be simple for now — open
all available input ports. A more sophisticated selection mechanism can come
later.

This ticket cannot be fully unit-tested without MIDI hardware; a smoke test
that constructs the connector with a mock queue and verifies it doesn't panic on
open/close is sufficient.
