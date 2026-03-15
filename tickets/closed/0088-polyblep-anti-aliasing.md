---
id: "0088"
title: PolyBLEP anti-aliasing for `Oscillator` sawtooth and square outputs
priority: medium
created: 2026-03-06
epic: "E016"
depends_on: ["0085"]
---

## Summary

The sawtooth and square outputs of `Oscillator` (T-0085) produce hard
discontinuities that alias heavily at audio frequencies. PolyBLEP (Polynomial
Band-Limited Step) is a lightweight per-sample correction that smooths
transitions without requiring a lookup table or oversampling. This ticket adds
PolyBLEP correction to both waveforms.

## Acceptance criteria

- [ ] A `polyblep(phase: f32, phase_increment: f32) -> f32` function is
      implemented in `patches-modules/src/common/` (new file `polyblep.rs` or
      inline in `oscillator.rs`).
- [ ] The sawtooth output applies PolyBLEP correction at the phase wrap
      discontinuity (phase ≈ 1.0 → 0.0):
      - add `polyblep(phase, phase_increment)` and subtract
        `polyblep(phase - 1.0 + phase_increment, phase_increment)` (standard
        two-sided correction around the reset).
- [ ] The square output applies PolyBLEP correction at both the rising
      transition (phase ≈ 0.0) and the falling transition (phase ≈ `duty`):
      - correction sign flips between the two transitions to match the
        `+1 → -1` and `-1 → +1` edges.
- [ ] PolyBLEP computation uses the current `phase_increment` from the
      `UnitPhaseAccumulator` (exposed as a `pub` field or accessor); no
      additional state is required.
- [ ] The sine and triangle outputs are not modified.
- [ ] Unit tests verify that the corrected waveforms no longer output exact
      `+1.0` or `-1.0` values at the transition sample (the PolyBLEP correction
      produces a value strictly between the two levels). The existing
      `square_output_values_are_only_plus_minus_one` test is updated or replaced
      accordingly.
- [ ] A spectral or THD test is not required; the behavioural transition test
      above is sufficient for this ticket.
- [ ] `cargo build`, `cargo test`, `cargo clippy` pass with no new warnings.

## Notes

Standard PolyBLEP formula for a normalised phase `t ∈ [0, 1)` and phase
increment `dt`:

```
fn polyblep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        2.0 * t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}
```

For sawtooth: `output = (2.0 * phase - 1.0) - polyblep(phase, dt)`.

For square: apply `+polyblep(phase, dt)` at the rising edge and
`-polyblep((phase - duty).rem_euclid(1.0), dt)` at the falling edge.

`phase_increment` is already available in `UnitPhaseAccumulator`; expose it as
a `pub` field or a `pub fn phase_increment(&self) -> f32` getter — whichever
is more consistent with the existing style.

PolyBLEP only corrects faithfully when `phase_increment < 0.5` (i.e. the
fundamental frequency is below Nyquist/2). No special handling is needed for
degenerate cases beyond this; signals above half Nyquist will alias regardless.
