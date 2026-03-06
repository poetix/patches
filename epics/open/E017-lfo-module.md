---
id: "E017"
title: LFO module — low-frequency oscillator with six waveform outputs
created: 2026-03-06
tickets: ["0090", "0091"]
---

## Summary

The patch system has no dedicated LFO module. `Oscillator` is technically usable
at sub-audio frequencies but carries V/OCT / FM inputs and PolyBLEP that are
irrelevant at LFO rates. This epic introduces an `Lfo` module tailored for
modulation use: a rate parameter spanning 0.01–20 Hz, six waveform outputs
(including sample-and-hold random), a phase offset parameter, a polarity mode,
a sync trigger input for note-locked behaviour, and a rate CV input for speed
modulation.

## Tickets

| ID   | Title                                                              | Priority | Depends on |
|------|--------------------------------------------------------------------|----------|------------|
| 0090 | `Lfo` core module — six outputs, rate, phase offset, polarity mode | high     | —          |
| 0091 | `Lfo` sync trigger input and rate CV input                         | medium   | 0090       |

## Definition of done

- A module registered as `"Lfo"` exists in `patches-modules`.
- Outputs: `sine`, `triangle`, `saw_up`, `saw_down`, `square`, `random`.
- `rate` Float parameter, min 0.01, max 20.0, default 1.0 (Hz).
- `phase_offset` Float parameter, min 0.0, max 1.0, default 0.0; shifts the
  read phase for all outputs.
- `mode` Enum parameter, variants `["bipolar", "unipolar_positive",
  "unipolar_negative"]`, default `"bipolar"`; scales and offsets all outputs.
- `sync` trigger input: a rising edge resets the phase accumulator to 0.
- `rate_cv` signal input: added directly to `rate` (in Hz) before computing the
  phase increment; cable scaling controls depth.
- `random` output holds a value drawn from a private PRNG each time the phase
  wraps; the held value is in `[-1.0, 1.0]` before polarity mapping.
- Outputs are only computed when connected (connectivity-gated via
  `set_connectivity`).
- No smoothing or anti-aliasing on any output.
- `cargo build`, `cargo test`, `cargo clippy` clean with no new warnings.
- No `unwrap()` or `expect()` in library code.
