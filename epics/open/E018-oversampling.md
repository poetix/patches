---
id: "E018"
title: Oversampling mode
created: 2026-03-07
updated: 2026-03-08
tickets: ["0092", "0093", "0094", "0095"]
---

## Summary

Add configurable oversampling (1×/2×/4×/8×) to the audio engine. Modules are
initialised at the elevated sample rate so they produce alias-free output (FM,
waveshaping, etc. all benefit automatically). A polyphase IIR half-band
decimation filter runs at the oversampled rate and folds the signal back down to
the device rate with >80 dB stopband attenuation. Oversampling factor is chosen
at start-up; it cannot change at runtime without restarting the engine.

## Design

### Filter structure

Each 2× decimation stage uses the Regalia-Mitra polyphase decomposition:

```text
H(z) = ½ · [A₀(z²) + z⁻¹ · A₁(z²)]
```

A₀ and A₁ are cascades of first-order all-pass sections in the w = z² domain:

```text
G(w) = (a + w⁻¹) / (1 + a·w⁻¹)
```

Branch 0 processes even input samples, branch 1 processes odd input samples,
both running at the output (half) rate. Output = (branch0 + branch1) / 2.

4× and 8× are cascades of two or three 2× stages respectively.

### No-allocation guarantee

All filter state lives in fixed-size arrays inside `AudioCallback`. Coefficients
are computed on the non-audio thread (during `SoundEngine::new`) and stored as
plain `f32` fields. `Decimator::push` contains no allocation, no branching on
heap structures, and no system calls.

### Effect on `AudioEnvironment`

`SoundEngine::open()` returns `AudioEnvironment { sample_rate: device_rate * N }`.
Modules see the elevated rate and adapt naturally (oscillators phase-increment
correctly, filter coefficients are correct). No `Module` trait changes required.

### Control period

`control_period` is specified in output-rate samples and converted to
oversampled-rate samples internally (`control_period * N`), preserving the same
wall-clock control frequency regardless of oversampling setting.

## Implementation plan

The filter implementation is intentionally split from the processing-flow
wiring. T-0093 introduces a naive no-op decimator (every-Nth-sample) to settle
the API and `AudioCallback` inner loop; T-0095 then replaces the internals with
the real polyphase IIR filter without touching anything else.

## Tickets

| ID   | Title                                                    | Priority | Depends on |
|------|----------------------------------------------------------|----------|------------|
| 0092 | `OversamplingFactor` type and engine wiring              | high     | —          |
| 0093 | Naive `Decimator` type (no-op downsampling)              | high     | 0092       |
| 0094 | Wire `Decimator` into `AudioCallback`                    | high     | 0093       |
| 0095 | Replace naive `Decimator` with polyphase IIR half-band   | high     | 0094       |

## Definition of done

- `OversamplingFactor::{None, X2, X4, X8}` is exported from `patches-engine`.
- `SoundEngine::new` and `PatchEngine::new` accept `OversamplingFactor`.
- `SoundEngine::open()` returns `AudioEnvironment` with `sample_rate` scaled by
  the oversampling factor.
- `AudioCallback::process_chunk` runs `plan.tick()` N times per output frame,
  feeding each oversampled sample through a per-channel `Decimator`.
- `Decimator` has no heap allocation after construction; verified by inspection.
- Unit tests confirm passband ripple < 0.01 dB up to 0.45× Nyquist and
  stopband attenuation > 60 dB above 0.55× Nyquist for a single 2× stage.
- `patch_player` accepts `--oversampling <1|2|4|8>`.
- `cargo build`, `cargo test`, `cargo clippy` clean with no new warnings.
- No `unwrap()` or `expect()` in library code.
