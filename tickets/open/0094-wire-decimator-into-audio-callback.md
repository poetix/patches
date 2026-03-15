---
id: "0094"
title: Wire `Decimator` into `AudioCallback`
priority: high
created: 2026-03-07
updated: 2026-03-08
epic: "E018"
---

## Summary

Replace the stub oversampling path from T-0092 (which simply used the last
inner-tick sink output) with the `Decimator` from T-0093. `AudioCallback` gains
two `Decimator` instances (left and right channels) constructed at engine start
time. `process_chunk` feeds each inner-tick sink output into the decimators and
writes the decimated result to the output buffer.

At this stage the decimator is the naive no-op version (every-Nth-sample). The
processing flow and wiring are the goal; audio quality is addressed in T-0095.

## Acceptance criteria

- [ ] `AudioCallback` has fields `decimator_l: Decimator` and `decimator_r:
  Decimator`, constructed in `AudioCallback::new` via
  `Decimator::new(oversampling)`.
- [ ] `process_chunk` inner loop (pseudocode):

  ```rust
  let mut out_l = 0.0_f32;
  let mut out_r = 0.0_f32;
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
- [ ] `HeadlessEngine` in `patches-integration-tests/src/lib.rs` is updated to
  accept an `OversamplingFactor` parameter and pass it through to `SoundEngine`.
- [ ] Integration smoke test in `patches-integration-tests`: build a simple
  patch (e.g. a constant-signal source into `AudioOut`), run with
  `OversamplingFactor::X2`, and verify the output samples are non-zero and
  finite. This confirms the inner loop produces output without asserting anything
  about alias rejection (that is T-0095's job).
- [ ] `cargo test` passes. `cargo clippy` clean.

## Notes

The `wi_counter` increment must happen inside the inner loop (once per
oversampled tick), not once per output frame, so that the double-buffer write
index alternates correctly at the oversampled rate.

For `OversamplingFactor::None`, `Decimator::push` always returns `Some(x)`
immediately, so the inner loop runs exactly once and the code is equivalent to
the original single-tick path. No special-casing needed.

Audio-quality verification (alias rejection ≥ 60 dB) is deliberately deferred
to T-0095, which replaces the naive decimator with the polyphase IIR filter.
