---
id: "0006"
title: Sound engine
priority: high
created: 2026-02-28
depends_on: ["0005"]
epic: "E001"
---

## Summary

Implement a sound engine in `patches-engine` that takes an `ExecutionPlan` and runs it continuously, writing the output of the `AudioOut` module to the hardware audio output. The engine runs until explicitly stopped. This is the component that makes the patch audible.

## Acceptance criteria

- [ ] `SoundEngine` struct in `patches-engine`
- [ ] `SoundEngine::new(plan: ExecutionPlan) -> Result<SoundEngine, EngineError>`
- [ ] `SoundEngine::start(&mut self)` — opens the audio output device and begins processing
- [ ] `SoundEngine::stop(&mut self)` — stops processing and closes the device
- [ ] Per tick: calls `process` on each module in execution order, advances all `SampleBuffer` write indices, then reads `left`/`right` from the `AudioOut` module and writes them to the hardware output buffer
- [ ] The audio callback does not allocate, block, or panic
- [ ] Sample rate is obtained from the opened device and passed to each module's `process` call
- [ ] `cargo clippy` is clean

## Notes

**Backend:** Use [CPAL](https://github.com/RustAudio/cpal) for cross-platform audio I/O. Add it as a dependency of `patches-engine` only.

**Audio callback constraints:** CPAL's output callback must be real-time safe. The `ExecutionPlan` and all `SampleBuffer`s are pre-allocated; the callback should only read/write pre-existing memory. If the engine needs to receive a new `ExecutionPlan` (hot-reload), use a lock-free channel (e.g. `triple-buffer` or an atomic swap) to hand it in between ticks — never inside the callback itself.

**Output format:** Request an `f32` output stream (CPAL's most portable format); convert from `f32` at the point of writing to the hardware buffer.

**Stopping:** `stop` should join or signal the audio thread cleanly. The engine should be restartable with a new `ExecutionPlan` after stopping.
