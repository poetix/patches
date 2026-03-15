---
id: "0004"
title: Audio output module
priority: high
created: 2026-02-28
depends_on: ["0001"]
epic: "E001"
---

## Summary

Implement an `AudioOut` module that acts as the sink node in a patch graph. It has two input ports (`"left"` and `"right"`) and no outputs. The engine identifies this module's inputs as the final stereo signal to be written to the audio output bus.

## Acceptance criteria

- [ ] `AudioOut` struct in `patches-modules` implementing `Module`
- [ ] Input ports: `"left"` and `"right"` (`f32` audio signals)
- [ ] No output ports
- [ ] Each call to `process` stores the received left/right sample values internally, accessible via `last_left() -> f32` and `last_right() -> f32` (or equivalent)
- [ ] The engine (0006) retrieves the stored samples after each tick to write to the hardware buffer — `AudioOut` does not call any audio API itself
- [ ] `cargo test -p patches-modules` passes
- [ ] `cargo clippy` is clean

## Notes

**Sink design:** `AudioOut` is a passive sink. It does not know about CPAL or any audio backend. The sound engine is responsible for reading its output after each tick and forwarding it to the hardware. This keeps `patches-modules` free of backend dependencies.

**One `AudioOut` per patch:** For now, assume exactly one `AudioOut` node per graph. The patch builder (0005) may enforce this as a validation step.
