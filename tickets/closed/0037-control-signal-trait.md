---
id: "0037"
epic: "E008"
title: Add ControlSignal enum and Module::receive_signal
priority: high
created: 2026-03-02
---

## Summary

Introduce a `ControlSignal` typed enum in `patches-core` for non-audio-rate
parameter updates (e.g. changing an oscillator's base frequency, or delivering an
OSC message). Add a `receive_signal` method to the `Module` trait with a default
no-op so that existing modules need no changes.

## Acceptance criteria

- [ ] `ControlSignal` is defined in `patches-core::module` (or a new
      `patches-core::signal` sub-module) and re-exported from `patches-core`.
      Initial variants:
      ```rust
      pub enum ControlSignal {
          /// A single named float parameter (e.g. frequency, gain).
          Float { name: &'static str, value: f32 },
          // Further variants (OSC bundles, MIDI, etc.) added as needed.
      }
      ```
- [ ] `Module` trait gains:
      ```rust
      fn receive_signal(&mut self, signal: ControlSignal) {}
      ```
      Default implementation is a no-op. `Send` bound on `ControlSignal` is not
      required here (signals are not sent across threads by the trait itself; the
      engine handles that via a ring buffer in T-0038).
- [ ] `SineOscillator` in `patches-modules` overrides `receive_signal` to handle
      `ControlSignal::Float { name: "freq", value }` by updating `self.frequency`.
      Other variants are ignored.
- [ ] Tests for `SineOscillator::receive_signal`:
      - Sending `Float { name: "freq", value: 880.0 }` updates the frequency.
      - Sending an unknown `name` or unrecognised variant is silently ignored.
- [ ] `cargo clippy` clean, `cargo test` green across the workspace.

## Notes

**Why a typed enum?** Signals may eventually carry structured data (OSC bundles with
multiple values, MIDI note events, etc.). Starting with a typed enum means new
variants can be added without breaking the `receive_signal` contract. `match`
exhaustiveness in overriding implementations will surface unhandled variants at
compile time once new variants are added — modules should use a wildcard arm to
stay forward-compatible.

**Ownership:** `receive_signal` takes ownership of `ControlSignal`. The engine
(T-0038) pops signals from a ring buffer and passes them by value. Modules that
need to store a signal value can do so directly; there is no need to clone.

**`Send` on `ControlSignal`:** `ControlSignal` must implement `Send` so it can be
queued in the engine's ring buffer (T-0038). Ensure no variant holds non-Send types
(e.g. `Rc`). The `&'static str` in `Float` is `Send`; `f32` is `Send`.

**`name` field in `Float`:** Using `&'static str` for the parameter name keeps the
variant allocation-free. Module implementations match on string literals (`"freq"`,
`"gain"`, etc.).
