---
id: "0093"
title: "Naive `Decimator` type (no-op downsampling)"
priority: high
created: 2026-03-08
epic: "E018"
---

## Summary

Introduce the `Decimator` type in `patches-engine/src/decimator.rs` with the
same public API that the eventual IIR filter will use, but implemented as a
naive downsampler: it simply discards the first N-1 oversampled samples of each
group and passes the last one through. No filtering is performed. This gets the
processing flow and the `AudioCallback` wiring settled before the complex filter
work in T-0095.

## Acceptance criteria

- [ ] `patches-engine/src/decimator.rs` defines:
  ```rust
  pub(crate) struct Decimator { /* ... */ }

  impl Decimator {
      pub(crate) fn new(factor: OversamplingFactor) -> Self
      pub(crate) fn push(&mut self, x: f32) -> Option<f32>
  }
  ```
- [ ] `Decimator::push` returns `None` for the first `factor - 1` calls of each
  group and `Some(x)` on the Nth call (i.e. every-nth-sample downsampling, no
  filtering).
- [ ] For `OversamplingFactor::None` (`factor == 1`), `push` always returns
  `Some(x)` — identical to the pre-oversampling code path.
- [ ] Unit tests:
  - `Decimator::new(OversamplingFactor::None)` — every call returns `Some`.
  - `Decimator::new(OversamplingFactor::X2)` — alternates `None`, `Some`.
  - `Decimator::new(OversamplingFactor::X4)` — returns `Some` every 4th call.
  - `Decimator::new(OversamplingFactor::X8)` — returns `Some` every 8th call.
- [ ] `cargo test` passes. `cargo clippy` clean.

## Notes

The naive implementation is intentionally trivial; correctness of audio quality
is not a goal here. The purpose is to lock in the `push` API and confirm the
inner-loop wiring in T-0094 works end-to-end before the filter design in T-0095
makes the internals more complex.

The `Decimator` type can be `pub(crate)` since nothing outside `patches-engine`
needs to construct one directly.
