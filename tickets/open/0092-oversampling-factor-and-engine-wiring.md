---
id: "0092"
title: "`OversamplingFactor` type and engine wiring"
priority: high
created: 2026-03-07
epic: "E018"
---

## Summary

Introduce `OversamplingFactor` and wire it through `SoundEngine`, `PatchEngine`,
and `patch_player`. At this stage no decimation filter exists yet — extra inner
ticks are executed but the output is just the last sample from the sink. The
primary goal is to get the API shape right and ensure modules are initialised at
the correct elevated sample rate.

## Acceptance criteria

- [ ] `patches-engine/src/oversampling.rs` defines and exports:
  ```rust
  pub enum OversamplingFactor { None, X2, X4, X8 }
  impl OversamplingFactor {
      pub fn factor(&self) -> usize  // 1, 2, 4, 8
  }
  ```
- [ ] `OversamplingFactor` is re-exported from `patches-engine/src/lib.rs`.
- [ ] `SoundEngine::new(buffer_pool_capacity, module_pool_capacity, control_period,
  oversampling: OversamplingFactor)` — new final parameter.
- [ ] `SoundEngine::open()` returns `AudioEnvironment { sample_rate: device_rate
  * factor as f32 }`.
- [ ] `SoundEngine` stores `oversampling_factor: usize` and passes it to
  `AudioCallback::new`.
- [ ] `AudioCallback` stores `oversampling_factor: usize`. `process_chunk` runs
  `plan.tick()` `oversampling_factor` times per output frame (no decimation yet;
  sink output from the last inner tick is used directly).
- [ ] `control_period` stored in `AudioCallback` is `control_period *
  oversampling_factor` so that the control rate in wall-clock time is preserved.
- [ ] `PatchEngine::new(registry, oversampling: OversamplingFactor)` and
  `PatchEngine::with_control_period(registry, control_period, oversampling)`.
- [ ] `patch_player` accepts an optional `--oversampling <1|2|4|8>` flag before
  the path argument, defaulting to `1` (no oversampling). Invalid values print a
  usage message and exit.
- [ ] All existing tests pass. `cargo clippy` clean.

## Notes

The `wi_counter` in `AudioCallback` should increment on every inner tick, not
just every outer frame, so that double-buffer indices rotate correctly at the
oversampled rate.

`OversamplingFactor::None` maps to `factor() == 1` and results in exactly the
same code path as before this ticket (one tick per output sample, sample rate
unchanged).

`HeadlessEngine` in `patches-integration-tests` constructs a `SoundEngine`
directly; update it to pass `OversamplingFactor::None` so it compiles.
