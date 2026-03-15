---
id: "0008"
title: Crossfade mixer module and dual-oscillator example
priority: medium
created: 2026-02-28
depends_on: ["0007"]
epic: "E001"
---

## Summary

Add a `Crossfade` module to `patches-modules` that averages two input signals,
and update the `sine_tone` example to use two `SineOscillator` instances pitched
a major third apart (440 Hz and ~554 Hz), mixed through the new module before
reaching `AudioOut`.

## Acceptance criteria

- [ ] `Crossfade` struct in `patches-modules/src/crossfade.rs`
- [ ] Two input ports `"a"` and `"b"`, one output port `"out"`
- [ ] `process`: `out = (a + b) / 2.0`
- [ ] Unit tests: descriptor shape, and that the output is the average of the two inputs
- [ ] `Crossfade` re-exported from `patches-modules`
- [ ] `sine_tone` example updated: two oscillators (440 Hz and 554.37 Hz) → `Crossfade` → `AudioOut` left + right
- [ ] `cargo build --example sine_tone` succeeds
- [ ] `cargo clippy` clean
- [ ] `cargo test` clean

## Notes

554.37 Hz is A4 (440 Hz) raised by a just major third: `440.0 * 2f32.powf(4.0 / 12.0)`.

The `Crossfade` name follows the user's specification. Structurally it is a
stereo mixer at a fixed 50/50 blend; a variable blend ratio can be a future
ticket if needed.
