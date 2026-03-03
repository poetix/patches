---
id: "0050"
epic: "E011"
title: Extract AudioCallback into its own module file
priority: low
created: 2026-03-03
---

## Summary

`engine.rs` contained both `AudioCallback` (the audio-thread struct) and `SoundEngine`
(the control-thread API). After the method extractions in T-0048, the two are clearly
separate concerns. This ticket moves `AudioCallback`, its `impl`, and the `build_stream`
helper into a new `callback.rs` module, leaving `engine.rs` responsible only for
`EngineError`, `PendingState`, and `SoundEngine`.

## Acceptance criteria

- [x] `patches-engine/src/callback.rs` created, containing `AudioCallback`, `impl AudioCallback`, and `build_stream`.
- [x] `AudioCallback` and `build_stream` marked `pub(crate)`; all fields remain private.
- [x] `mod callback;` declared in `lib.rs` (private to the crate; not re-exported).
- [x] `engine.rs` imports `AudioCallback` and `build_stream` from `crate::callback`.
- [x] `DeviceTrait` import moved to `callback.rs`; `engine.rs` retains only the imports it actually uses.
- [x] `cargo clippy` clean, all tests pass.
