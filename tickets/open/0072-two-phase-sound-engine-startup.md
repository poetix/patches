---
id: "0072"
title: Two-phase SoundEngine startup (open / start)
priority: high
epic: "E014"
created: 2026-03-04
---

## Summary

Split `SoundEngine::start()` into two phases: `open()` opens the audio device and
queries its configuration (yielding the sample rate as an `AudioEnvironment`), and
`start(plan)` spawns the audio thread and begins playback. This lets the caller build
the initial plan with the real sample rate before audio begins, eliminating the
post-construction `Module::initialise()` workaround.

## Acceptance criteria

- [ ] `SoundEngine` exposes an `open()` method (or equivalent) that opens the audio
      device, queries the sample rate, and returns/stores an `AudioEnvironment` — but
      does **not** start the audio callback.
- [ ] `SoundEngine` exposes a `start(plan: ExecutionPlan)` method that takes a fully-
      constructed plan and begins the audio thread.
- [ ] `SoundEngine::open()` can be called before `start()` and the sample rate is
      available to the caller between the two calls.
- [ ] `SoundEngine::swap_plan()` no longer calls `Module::initialise()` on new modules.
      Modules arrive fully constructed (via `Module::build()`) and are installed directly.
- [ ] The `Module::initialise()` call path in `SoundEngine` is removed (modules are
      fully prepared at construction time via `prepare()` + `update_validated_parameters()`).
- [ ] Existing examples (`sine_tone`, `demo_synth`, etc.) continue to work with the
      new startup sequence.
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

The current `SoundEngine::new()` takes an initial `ExecutionPlan` before the device is
opened. The new API flips this: the engine is created, the device is opened (yielding
sample rate), the caller builds the plan, then `start(plan)` begins audio.

The cleanup thread should be started in `open()` or `start()` — whichever is more
natural. The key constraint is that the sample rate must be available before the caller
needs to build the first plan.

See ADR 0012 § "Startup sequence changes".
