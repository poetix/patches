---
id: "0017"
title: Add AudioEnvironment and initialise method to Module trait
priority: high
created: 2026-02-28
---

## Summary

`sample_rate` is currently passed on every `process()` call, which is redundant
since it is constant for the lifetime of an audio stream. Introduce an
`AudioEnvironment` struct (in `patches-core`) and an `initialise` method on the
`Module` trait that is called once when a plan is activated. Modules that need the
sample rate (e.g. `SineOscillator`) store it in `initialise` and use the stored
value in `process`. Remove `sample_rate` from `process` entirely.

## Acceptance criteria

- [ ] `AudioEnvironment { pub sample_rate: f32 }` added to `patches-core`
- [ ] `Module::initialise(&mut self, _env: &AudioEnvironment) {}` default no-op added
- [ ] `sample_rate: f32` removed from `Module::process` signature
- [ ] `ExecutionPlan::tick()` takes no sample_rate parameter
- [ ] `ExecutionPlan::initialise(&mut self, env: &AudioEnvironment)` added
- [ ] `SoundEngine` stores sample_rate after `start()`, calls `initialise` on initial
      plan and on each plan passed to `swap_plan`
- [ ] `SineOscillator` implements `initialise` to store sample_rate; uses it in `process`
- [ ] `AudioOut` and `Mix` updated to new `process` signature (no logic change)
- [ ] All tests updated and passing
- [ ] `cargo clippy` clean

## Notes

Part of epic E003. `AudioEnvironment` lives in `patches-core` (no backend knowledge).
`swap_plan` called before `start()` skips `initialise` (sample rate unknown) — document this.
