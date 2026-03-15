---
id: "0047"
epic: "E011"
title: Add sink output caching to ModulePool
priority: high
created: 2026-03-03
---

## Summary

`ModulePool` was a thin newtype around `Box<[Option<Box<dyn Module>>]>` with no
awareness of which slot holds the `Sink` module. Callers had to call `get()` to
retrieve a module reference, call `as_sink()` on it, and read its output — placing
vtable dispatch in the hot audio path. This ticket removes `get`, tracks the sink slot
index directly on `ModulePool`, and caches the sink's last output as plain `f32` fields
updated inside `process()`.

## Acceptance criteria

- [x] `get` method removed from `ModulePool`.
- [x] `ModulePool` gains `sink_slot: Option<usize>`, `last_sink_left: f32`, `last_sink_right: f32` fields.
- [x] `install()` detects sinks at install time via `module.as_sink().is_some()` and records the slot; clears registration if a non-sink replaces the registered sink slot.
- [x] `tombstone()` clears `sink_slot` and zeros the cache when the sink slot is tombstoned.
- [x] `process()` updates the cache after processing the sink slot — a single vtable call per tick, not per read.
- [x] `has_sink() -> bool` added (distinguishes "no sink" from "sink produced 0.0").
- [x] `read_sink_left() -> f32` and `read_sink_right() -> f32` are plain field reads — no vtable dispatch.
- [x] `get()` removed; `ModulePool` exposes no method that returns a module reference — the pool boundary is opaque to callers.
- [x] Tests updated to cover `has_sink`, `read_sink_left/right`, `tombstone_clears_sink`, `non_sink_install_does_not_register_sink`, `sink_install_registers_sink`, `read_sink_reflects_last_processed_value`.
- [x] `cargo clippy` clean, all tests pass.

## Notes

Caching a `&dyn Sink` in the struct is not possible (self-referential). Caching a
raw pointer would require unsafe. The chosen approach — cache scalar output values
and update them in `process()` — is zero-unsafe and makes the read path a pair of
field accesses with no branching in the common case.
