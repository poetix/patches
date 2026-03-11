---
id: "0118"
title: "Migrate all `patches-modules` modules to new `Module` trait"
priority: medium
epic: "E022"
depends_on: ["0116", "0117"]
created: 2026-03-11
---

## Summary

Update every module implementation in `patches-modules` to compile and pass
tests under the new `Module` trait introduced in T-0115. Modules that only
ever use mono signals can be wrapped in `MonoShim<M>` (T-0117) with minimal
changes; poly-aware modules implement `set_ports` and `process` directly.

## Acceptance criteria

- [ ] Every module in `patches-modules` compiles against the new `Module` trait.
- [ ] All port declarations in every module's `ModuleDescriptor` include
      `kind: CableKind::Mono` (existing modules are mono-only; no behaviour
      change).
- [ ] Each migrated module either:
      - Wraps itself in `MonoShim<M>` by implementing `MonoModule` (minimal
        change path), or
      - Implements `set_ports` and `process` directly, storing `InputPort` /
        `OutputPort` fields by name on the struct.
- [ ] No module retains a reference to the old `set_connectivity` method.
- [ ] `cargo test -p patches-modules` passes with no failures.
- [ ] `cargo clippy -p patches-modules` reports no warnings.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

The preferred approach for each module is whichever is simpler. For modules
with straightforward mono DSP logic, `MonoShim` is likely less churn. For
modules that already track connectivity internally or have conditional
per-port behaviour, a direct implementation of `set_ports` + named port fields
may be cleaner.

The integration tests in `patches-integration-tests` must also pass after this
ticket. If any integration test relied on `set_connectivity` behaviour, it
should be updated to use `InputPort::is_connected()` or the `port_updates`
mechanism instead.
