---
id: "0003"
title: Sine oscillator module
priority: high
created: 2026-02-28
depends_on: ["0001"]
epic: "E001"
---

## Summary

Implement a sine wave oscillator as the first concrete `Module`. It takes a frequency (in Hz) as an input port and produces a single audio output. The oscillator uses phase accumulation to produce a continuous sine wave at the specified frequency.

## Acceptance criteria

- [ ] `SineOscillator` struct in `patches-modules` implementing `Module`
- [ ] Frequency set at construction time: `SineOscillator::new(frequency: f32)`
- [ ] No input ports
- [ ] Output port: `"out"` (audio signal, `f32` in the range −1.0 to 1.0)
- [ ] Phase is accumulated correctly across calls — the waveform is continuous (no discontinuities between samples)
- [ ] Phase wraps within `[0, 2π)` to avoid float drift over time
- [ ] `cargo test -p patches-modules` passes, including at least:
      - a test that verifies the output completes a full cycle in `sample_rate / frequency` samples
- [ ] `cargo clippy` is clean

## Notes

**Phase accumulation:** Each sample, advance phase by `2π × frequency / sample_rate`. Output is `phase.sin()`.
