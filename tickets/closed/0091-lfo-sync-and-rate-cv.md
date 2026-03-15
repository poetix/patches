---
id: "0091"
title: "`Lfo` sync trigger input and rate CV input"
priority: medium
created: 2026-03-06
epic: "E017"
depends_on: ["0090"]
---

## Summary

Add two inputs to the `Lfo` module: a `sync` trigger that resets the phase
accumulator on each rising edge, and a `rate_cv` signal that offsets the
effective rate in Hz. Both are optional — connectivity-gated — so the module
behaves identically to T-0090 when neither is connected.

## Acceptance criteria

- [ ] Module descriptor is updated to include:
      - `sync` (index 0): trigger input; a rising edge (previous sample ≤ 0,
        current sample > 0) resets `phase` to 0.0.
      - `rate_cv` (index 1): signal input; added directly to `rate` (in Hz)
        before computing the phase increment for that sample.
        Effective rate is clamped to `[0.001, 40.0]` after adding the CV to
        prevent negative or runaway increments.
- [ ] `set_connectivity` is updated to track both inputs.
- [ ] `process` applies sync detection and/or rate CV only when the respective
      input is connected per connectivity state.
- [ ] Sync detection uses a `prev_sync: f32` field (last sample's sync input
      value, initialised to 0.0). Phase reset is applied *before* the phase
      advance for that sample.
- [ ] When `rate_cv` is connected, the phase increment is recomputed each sample
      as `(rate + inputs[1]).clamp(0.001, 40.0) / sample_rate` rather than using
      the cached `phase_increment`. When disconnected, the cached
      `phase_increment` is used as before.
- [ ] Tests:
      - with `sync` connected, a rising edge mid-cycle resets the sine output
        to match phase 0 on the following sample
      - no spurious reset on a flat positive sync signal (level, not edge)
      - with `rate_cv` connected, doubling the base rate via CV produces a cycle
        at half the expected period
      - rate CV is clamped: a large negative CV does not produce negative or
        zero phase increment
      - with neither input connected, behaviour is identical to T-0090
        (regression check)
- [ ] `cargo build`, `cargo test`, `cargo clippy` pass with no new warnings.

## Notes

Clamping effective rate to `[0.001, 40.0]` (rather than the parameter's
`[0.01, 20.0]`) gives a little headroom for CV modulation to push slightly
beyond the parameter maximum, while still preventing wrap-direction confusion
from a zero or negative increment.

The `rate_cv` per-sample recomputation bypasses the cached `phase_increment`.
This is intentional — caching is only valid when rate is static — and the
extra multiply per sample is negligible at LFO rates.
