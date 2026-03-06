---
id: "0089"
title: Add `phase_mod` input to `Oscillator`
priority: medium
created: 2026-03-06
epic: "E016"
depends_on: ["0085"]
---

## Summary

`Oscillator` currently drives all waveform outputs from an unmodulated phase
accumulator (plus the existing `voct` / `fm` inputs for frequency modulation).
A `phase_mod` input allows external signals to shift the instantaneous phase
each sample, enabling PM synthesis and hard-sync-like effects.

## Acceptance criteria

- [ ] `Oscillator` descriptor gains a fourth input: `phase_mod` (index 3),
      following `voct` (0), `fm` (1), `pulse_width` (2).
- [ ] When `phase_mod` is connected (per `set_connectivity`), every waveform
      computation samples from `(phase + inputs[3]).fract()` rather than
      `phase` directly. Phase accumulator state is unaffected — the modulation
      is applied at read time, not written back.
- [ ] When `phase_mod` is disconnected, waveform outputs are computed from
      `phase` as before (no behavioural change).
- [ ] Signal range convention is `[-1.0, 1.0]`; the full range shifts the phase
      by one full cycle. Callers use cable scaling to control depth.
- [ ] `set_connectivity` is updated to track `phase_mod` connectivity alongside
      the existing three inputs.
- [ ] Tests:
      - descriptor has 4 inputs with the correct names in order
      - with `phase_mod = 0.5` connected, sine output at phase 0.0 matches
        `lookup_sine(0.5)` (i.e. the half-cycle offset is applied)
      - disconnecting `phase_mod` restores normal output
- [ ] `cargo build`, `cargo test`, `cargo clippy` pass with no new warnings.

## Notes

Applying modulation at read time (rather than adding to the accumulator)
means the carrier frequency is not disturbed and phase wraps cleanly at all
modulation depths. This is the conventional PM implementation.

PolyBLEP correction (T-0088) must be aware of the modulated phase when computing
discontinuity proximity. If T-0088 lands first, it should be updated to pass
the modulated phase to `polyblep` rather than the raw accumulator phase.
The dependency ordering (both depend on T-0085) means either ticket may land
first; the second to land is responsible for the integration.
