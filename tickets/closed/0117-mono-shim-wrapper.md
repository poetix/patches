---
id: "0117"
title: "`MonoShim<M>` backward-compatible wrapper"
priority: medium
epic: "E022"
depends_on: ["0115"]
created: 2026-03-11
status: will-not-do
closed: 2026-03-12
---

## Decision

Skipped. All modules in `patches-modules` were migrated directly to the new `Module` trait
in T-0118 (implementing `set_ports` + `process` with pool access), making the shim unnecessary.

## Summary

Provide a `MonoShim<M>` wrapper type in `patches-core` (or `patches-engine`)
that adapts a legacy mono-only module to the new `Module` trait without
requiring any changes to the module's DSP logic. This minimises the blast
radius of the trait change during the migration window.

## Acceptance criteria

- [ ] A `MonoModule` trait (or equivalent) defines the old-style processing
      signature:
      ```rust
      pub trait MonoModule {
          fn describe() -> ModuleDescriptor where Self: Sized;
          fn process_mono(&mut self, inputs: &[f32], outputs: &mut [f32]);
          // plus initialise, set_parameter as needed
      }
      ```
- [ ] `MonoShim<M: MonoModule>` implements `Module`:
      - `describe()` delegates to `M::describe()`.
      - `set_ports()` stores the received `InputPort` and `OutputPort` slices
        internally (e.g. `Vec<InputPort>` / `Vec<OutputPort>` fields).
      - `process()` pre-reads all connected mono inputs into a `Vec<f32>` scratch
        buffer, calls `M::process_mono(&scratch_inputs, &mut scratch_outputs)`,
        then writes each non-`Disconnected` output port back to the pool as
        `CableValue::Mono`. Unconnected outputs are skipped.
- [ ] The scratch buffers in `MonoShim` are pre-allocated during
      `Module::initialise` (or at `set_ports` time) so that `process()` does
      not allocate on the audio thread.
- [ ] `MonoShim<M>` compiles and passes a unit test that wraps a trivial
      `MonoModule` implementation (e.g. a passthrough), calls `set_ports` with
      a synthetic `InputPort::Mono` and `OutputPort::Mono`, and verifies that
      `process()` propagates the value correctly.
- [ ] `cargo clippy` and `cargo test -p patches-core` (or the crate that houses
      `MonoShim`) clean.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

`MonoShim` is a migration aid, not a permanent public API. It may be deprecated
once all modules in `patches-modules` have been migrated to implement `Module`
directly (see T-0118).

Poly inputs on a mono-shimmed module are handled by the shim reading only
channel 0 (the mono convention). This is noted in the shim's documentation as
a deliberate limitation.
