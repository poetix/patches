---
id: "0095"
title: "Polyphase IIR half-band `Decimator`"
priority: high
created: 2026-03-08
epic: "E018"
depends_on: ["0094"]
---

## Summary

Implement the `Decimator` type in `patches-engine/src/decimator.rs` as a
polyphase IIR half-band filter, providing >60 dB stopband attenuation.
Public API: `Decimator::new(factor: OversamplingFactor)` and `Decimator::push`.
The processing flow established in T-0094 requires no modification.

## Acceptance criteria

- [ ] `patches-engine/src/decimator.rs` implements the Regalia-Mitra polyphase
  decomposition:

  ```
  H(z) = ½ · [A₀(z²) + z⁻¹ · A₁(z²)]
  ```

  Where A₀ and A₁ are cascades of first-order all-pass sections in the w = z²
  domain:

  ```
  G(w) = (a + w⁻¹) / (1 + a·w⁻¹)
  ```

- [ ] Internal types (not part of the public API):
  - `AllPassSection { a: f32, x1: f32, y1: f32 }` — `fn tick(&mut self, x: f32) -> f32`
    implements `y = a*(x - y1) + x1; x1 = x; y1 = y; y`.
  - `HalfBandStage` — two branches of `M` `AllPassSection` values each,
    plus `branch0_last: f32` and `phase: u8`. `fn push(&mut self, x: f32) ->
    Option<f32>` returns `Some((branch0_last + branch1_out) / 2.0)` on odd
    calls and `None` on even calls.

- [ ] `Decimator` internally uses `HalfBandStage` cascade(s): one stage for
  X2, two for X4, three for X8. `Passthrough` variant is unchanged.

- [ ] Coefficients: a 6th-order design (M = 2 all-pass sections per branch)
  targeting ≥ 80 dB stopband attenuation. Embed as `const` values. Use a
  verified reference (e.g. Zölzer "DAFX" Table 11.2, Valimäki & Haghparast)
  rather than computing from scratch.

- [ ] All filter state lives in fixed-size arrays; no `Vec`, `Box`, or heap
  allocation after construction. Verify by inspection.

- [ ] Unit tests:
  - Feed a 1 kHz sine at 96 kHz (simulating 2× oversampling of 48 kHz
    hardware) through `Decimator::new(X2)`. Output amplitude within 0.01 dB of
    input amplitude.
  - Feed a 30 kHz sine (above the 24 kHz Nyquist of the 48 kHz output rate)
    through `Decimator::new(X2)`. Output RMS at least 60 dB below full scale.
  - `Decimator::new(X4)` and `X8` — same passband/stopband checks at the
    appropriate frequencies.

- [ ] Integration test from T-0094 (non-zero finite output) continues to pass.

- [ ] `cargo test` passes. `cargo clippy` clean. No `unwrap()`/`expect()` in
  library code.

## Notes

All cascaded stages use the same half-band coefficients: they all perform 2×
decimation with the same normalised cutoff, so the same filter is correct for
each stage.

`control_period` is already stored in `AudioCallback` as
`control_period * oversampling_factor` (from T-0092), so there is nothing to
change there.
