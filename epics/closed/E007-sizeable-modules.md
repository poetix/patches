---
id: "E007"
title: Sizeable modules (indexed ports)
created: 2026-03-02
tickets: ["0035", "0036"]
---

## Summary

Audio modules today have a fixed, statically-described port layout with no way to
express multiple ports sharing the same semantic name (e.g. `in/0`, `in/1`,
`in/2`). This epic adds an explicit port index to the port descriptor and a
`PortRef` value type for call-sites, then delivers the first module that exploits
it: `Sum(size)`, a variable-width summing bus that replaces the existing two-input
`Mix`.

Indexed ports unlock a class of natively *poly* or *scalable* modules:

- **Fan-out / fan-in buses** — route one signal to N destinations or sum N signals
  to one.
- **Variable-channel mixers** — one module parameterised by channel count rather
  than a static bank of two-input mixers.
- **Poly oscillators / filters** — a single graph node with N voice ports,
  enabling polyphonic patches without graph duplication.

## Acceptance criteria

- [ ] All tickets closed.
- [ ] `cargo clippy` clean, `cargo test` green across the workspace.
- [ ] No existing module's external behaviour changes (only call-site syntax at
      `connect()` changes; processing semantics are identical).

## Tickets

| ID   | Title                                                        | Priority |
|------|--------------------------------------------------------------|----------|
| 0035 | Add `PortRef` type and `PortDescriptor.index`; update graph and builder | high |
| 0036 | Add `Sum` module; remove `Mix`                               | high     |

## Notes

Tickets must be worked in order: 0035 (infrastructure) before 0036 (first consumer).

The 1-sample cable delay and buffer-pool layout are unaffected by indexed ports;
this epic is purely a naming/addressing change at the descriptor and graph layers.
