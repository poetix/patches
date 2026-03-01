---
id: "0023"
epic: "E004"
title: Remove write_phase from ExecutionPlan
priority: low
created: 2026-03-01
---

## Summary

`ExecutionPlan` carries a mutable `write_phase: bool` field that alternates
on every `tick()` call to select the active write slot in each cable's
two-element ring buffer. This makes the plan itself stateful beyond its module
contents, muddying the boundary between "plan structure" and "runtime state".

The field can be eliminated by having the audio engine step through the output
buffer two frames at a time — always processing one frame with `wi = 0`
followed by one frame with `wi = 1`. CPAL callback buffers are sized as a
multiple of two frames in practice (hardware devices expose power-of-two frame
counts), so this is always a clean split.

## Acceptance criteria

- [ ] `ExecutionPlan` no longer has a `write_phase` field
- [ ] `tick()` accepts an explicit `wi: usize` parameter (0 or 1); `ri = 1 - wi`
- [ ] `fill_buffer` in `patches-engine/src/engine.rs` iterates in steps of 2,
      calling `plan.tick(0)` then `plan.tick(1)` per step; panics (debug) or
      handles gracefully (release) if the frame count is odd
- [ ] `ExecutionPlan::initialise` is unaffected
- [ ] All tests updated; `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

**Why this is safe.** The two-slot ring buffer already guarantees that, within
a single `tick()`, every read comes from the slot not being written (ri ≠ wi).
Fixing wi = 0 for odd frames and wi = 1 for even frames preserves this property
identically to the current alternating bool.

**Frame-count assumption.** CPAL does not formally guarantee power-of-two
buffer sizes, but in practice all common backends (CoreAudio, ALSA, WASAPI)
use power-of-two frame counts. A `debug_assert!(frames % 2 == 0)` in
`fill_buffer` documents and enforces this assumption without release overhead.
If a future backend violates it, the assert will catch it early.

**`last_left` / `last_right` are unaffected.** These read from the `AudioOut`
module's stored fields (set inside `process()`), not from the cable buffers
directly, so they remain correct after either tick.

**test changes.** Tests that call `plan.tick()` directly will need to pass an
explicit `wi` (alternating 0 and 1, or always 0 if ordering doesn't matter for
the specific assertion).
