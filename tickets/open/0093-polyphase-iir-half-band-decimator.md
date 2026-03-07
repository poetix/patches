---
id: "0093"
title: Polyphase IIR half-band `Decimator`
priority: high
created: 2026-03-07
epic: "E018"
---

## Summary

Implement the `Decimator` type in `patches-engine/src/decimator.rs`. It wraps
one, two, or three cascaded polyphase IIR half-band stages for 2×/4×/8×
decimation, and is a zero-cost pass-through for 1×. All state lives in
fixed-size arrays; no heap allocation after construction.

## Acceptance criteria

- [ ] `patches-engine/src/decimator.rs` contains:
  - `AllPassSection { a: f64, x1: f64, y1: f64 }` — one first-order all-pass
    in the w = z² domain. `fn tick(&mut self, x: f64) -> f64` implements:
    `y = a*(x - y1) + x1; x1 = x; y1 = y; y`.
  - `HalfBandStage` — two branches of `M` `AllPassSection` values each (M
    chosen to give ≥ 60 dB stopband attenuation; see Notes), plus a
    `branch0_last: f64` and a `phase: u8`. `fn push(&mut self, x: f64) ->
    Option<f64>` returns `Some((branch0_last + branch1_out) / 2.0)` on odd
    calls (phase == 1) and `None` on even calls.
  - `Decimator` — a single public type with a `fn new(factor: OversamplingFactor)
    -> Self` constructor and `fn push(&mut self, x: f64) -> Option<f64>`.
    Internally an enum with variants `Passthrough`, `X2`, `X4`, `X8` holding
    the appropriate number of `HalfBandStage` values.
- [ ] `Decimator::push` for `Passthrough` always returns `Some(x)` (every call).
- [ ] `Decimator::push` for `X2` returns `Some` every 2 calls, `X4` every 4,
  `X8` every 8.
- [ ] Unit test: feed a 1 kHz sine at 96 kHz (simulating 2× oversampling of 48
  kHz hardware) through a `Decimator::new(X2)`. Verify the output amplitude
  is within 0.01 dB of the input amplitude.
- [ ] Unit test: feed a 30 kHz sine (above the 24 kHz Nyquist of the 48 kHz
  output rate) through a `Decimator::new(X2)`. Verify the output RMS is at
  least 60 dB below full scale.
- [ ] Unit test: `Decimator::new(X4)` and `X8` — same passband/stopband checks
  at the appropriate frequencies.
- [ ] `Decimator` is re-exported from `patches-engine/src/lib.rs` (or kept
  `pub(crate)` if only used internally — decide at implementation time).
- [ ] `cargo test` passes. `cargo clippy` clean.

## Notes

### Coefficient design

The half-band filter has its transition band centred on π/2 (one quarter of the
oversampled rate, i.e. the Nyquist of the output rate). The two polyphase
branches share a fixed set of all-pass coefficients chosen so that the stopband
attenuation is ≥ 60 dB (targeting ≥ 80 dB where cost permits).

A practical starting point is an elliptic or quasi-elliptic half-band design.
Coefficients can be derived by:

1. Starting from an Nth-order elliptic lowpass prototype with passband edge
   0.45π, stopband edge 0.55π, passband ripple 0.01 dB, stopband attenuation
   80 dB.
2. Extracting the polyphase all-pass coefficients via the Regalia-Mitra
   decomposition (alternating poles between A₀ and A₁).

Well-known tabulated coefficient sets also exist in the literature
(e.g. Zölzer "DAFX" Table 11.2, or Valimäki & Haghparast); use a verified
reference rather than computing from first principles during implementation if
this is simpler.

A 4th-order design (M = 1, one all-pass section per branch) gives roughly
40–50 dB attenuation. A 6th-order design (M = 2 per branch) reaches ≥ 80 dB
and is the recommended target. Embed the coefficients as `const` values.

### Stage cascading for 4× and 8×

All cascaded stages use the same halfband coefficients because they all perform
2× decimation with the same normalised cutoff (the transition band always straddles
the output Nyquist). The first stage does the heaviest anti-aliasing work; later
stages only need to suppress whatever aliasing the previous stage's stopband lets
through, but using the same coefficients is conservative and correct.

### No-alloc requirement

`HalfBandStage` must not implement `Drop` in a way that frees heap memory, and
must not contain `Vec`, `Box`, or any other heap type. The const-generic array
`[AllPassSection; M]` satisfies this. Verify by inspection.
