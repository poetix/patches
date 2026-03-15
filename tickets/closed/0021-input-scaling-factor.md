---
id: "0021"
title: Add per-connection input scaling factor
priority: medium
created: 2026-02-28
---

## Summary

Each patch cable (connection between an output port and an input port) should carry
a scaling factor in `[-1, 1]` applied to the signal at read-time inside `tick()`.
This allows attenuating or inverting a signal path without needing a dedicated
gain/attenuator module. The scale is stored on the graph edge, resolved to a
slot-level `Vec<f32>` at build time, and applied with a single multiply per sample.

## Acceptance criteria

- [ ] `Edge` in `ModuleGraph` gains a `scale: f32` field
- [ ] `GraphError::ScaleOutOfRange(f32)` variant added
- [ ] `connect(from, out, to, in, scale: f32)` validates `scale.is_finite() &&
      (-1.0..=1.0).contains(&scale)`; returns `Err(ScaleOutOfRange)` for invalid values
- [ ] `edge_list()` returns the scale as a fifth element in each tuple
- [ ] `ModuleSlot` gains `input_scales: Vec<f32>`
- [ ] `build_patch` populates `input_scales` (`1.0` for unconnected, edge scale for connected)
- [ ] `ExecutionPlan::tick()` multiplies each input by its scale before passing to `process`
- [ ] All existing `connect` call sites updated to pass `1.0`
- [ ] Test: sine → AudioOut with scale `0.5`; `last_left()` ≈ half the unscaled amplitude
- [ ] Test: `connect` with `scale` outside `[-1, 1]` returns `ScaleOutOfRange`
- [ ] `cargo clippy` clean, all tests passing

## Notes

Part of epic E003. Validation at graph-build time; no audio-thread overhead.
`disconnect()` signature unchanged — key remains `(from, output, to, input)`.
