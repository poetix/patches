---
id: "0094"
title: Wire `Decimator` into `AudioCallback`
priority: high
created: 2026-03-07
epic: "E018"
---

## Summary

Replace the stub oversampling path from T-0092 (which simply used the last
inner-tick sink output) with proper decimation. `AudioCallback` gains two
`Decimator` instances (left and right channels) constructed at engine start time.
`process_chunk` feeds each inner-tick sink output into the decimators and writes
the decimated result to the output buffer.

## Acceptance criteria

- [ ] `AudioCallback` has fields `decimator_l: Decimator` and `decimator_r:
  Decimator`, constructed in `AudioCallback::new` via
  `Decimator::new(oversampling)`.
- [ ] `process_chunk` inner loop (pseudocode):
  ```
  let mut out_l = 0.0_f64;
  let mut out_r = 0.0_f64;
  for _ in 0..self.oversampling_factor {
      self.current_plan.tick(&mut self.module_pool, &mut self.buffer_pool, wi);
      self.wi_counter += 1;
      let wi = self.wi_counter % 2;
      if let Some(l) = self.decimator_l.push(self.module_pool.read_sink_left()) {
          out_l = l;
      }
      if let Some(r) = self.decimator_r.push(self.module_pool.read_sink_right()) {
          out_r = r;
      }
  }
  // write out_l, out_r to data[out_i]
  ```
- [ ] `Decimator::push` for `Passthrough` (1× oversampling) always returns
  `Some`, so the code path is identical to the pre-oversampling behaviour for
  `OversamplingFactor::None`.
- [ ] Integration test in `patches-integration-tests` using `HeadlessEngine`:
  - Build a patch with an oscillator at a frequency above half the device rate
    (simulating an alias-generating signal at the oversampled rate) and an
    `AudioOut`.
  - Run with `OversamplingFactor::X2` and collect N output samples.
  - Assert that the output RMS is at least 40 dB below what it would be without
    decimation (i.e. the alias is suppressed). A sine at exactly the folding
    frequency with known amplitude is a convenient test signal.
- [ ] `HeadlessEngine` in `patches-integration-tests/src/lib.rs` is updated to
  accept an `OversamplingFactor` parameter and pass it through to `SoundEngine`.
- [ ] `cargo test` passes. `cargo clippy` clean.

## Notes

The `wi_counter` increment must happen inside the inner loop (once per
oversampled tick), not once per output frame, so that the double-buffer write
index alternates correctly at the oversampled rate.

For `OversamplingFactor::None`, `Decimator::push` always returns `Some(x)`
immediately, so the inner loop runs exactly once and the code is equivalent to
the original single-tick path. No special-casing needed.

The integration test does not require CPAL or real audio hardware. Drive
`HeadlessEngine::tick_n` (or equivalent) directly and inspect the collected
samples.
