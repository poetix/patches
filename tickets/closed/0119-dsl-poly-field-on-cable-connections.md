---
id: "0119"
title: "DSL `poly: N` field on cable connections"
priority: medium
epic: "E022"
depends_on: ["0114"]
created: 2026-03-11
status: superseded
---

> **Superseded.** Cable arity is a property of the port, not the connection.
> `PortDescriptor::kind` (a `CableKind`) is already declared per-port in the
> `ModuleDescriptor`, which is a compile-time constant per module type. The
> planner has both endpoints' descriptors at plan-build time and should infer
> pool slot kind from the source port's `CableKind` directly. Adding a `poly: N`
> annotation to the DSL cable list would duplicate information already present in
> the module definitions and could produce contradictions. No DSL change is
> needed; the planner's pool-allocation logic (T-0116) should read `kind` from
> the source `PortDescriptor`.

## Summary

Extend the YAML DSL to allow cable connections to declare `poly: N`, causing
the planner to allocate a `CableValue::Poly` buffer pool slot for that
connection. Connections without the field default to `CableValue::Mono`.

## Acceptance criteria

- [ ] The cable connection type in the YAML schema gains an optional `poly`
      field:
      ```yaml
      cables:
        - from: sequencer.voct   to: oscillator.voct   poly: 16
        - from: oscillator.out   to: filter.in
      ```
      Only `poly: 16` is a valid value in this iteration (16 is the fixed
      maximum). Any other value is a parse error.
- [ ] The DSL deserialiser maps `poly: 16` to `CableKind::Poly` and its
      absence to `CableKind::Mono`.
- [ ] The patch builder / planner respects the declared kind when allocating
      pool slots: `poly` cables get a `CableValue::Poly` slot, mono cables get
      a `CableValue::Mono` slot.
- [ ] If the declared `CableKind` does not match the port descriptors of the
      source or destination (i.e. `connect()` would reject it), the DSL loader
      returns a descriptive error rather than panicking.
- [ ] At least one example YAML patch file demonstrates a `poly: 16` cable
      (can be a toy patch; does not need to produce musical output).
- [ ] Unit tests:
      - A YAML snippet with `poly: 16` deserialises to `CableKind::Poly`.
      - A YAML snippet without `poly` deserialises to `CableKind::Mono`.
      - A YAML snippet with an invalid value (e.g. `poly: 8`) returns a parse
        error.
- [ ] `cargo clippy` and `cargo test` clean across all crates.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

The fixed maximum of 16 is intentional (matches `CableValue::Poly([f32; 16])`).
Variable poly depth is out of scope for this epic.

Voice management (mapping MIDI notes to poly channels) is also out of scope;
the DSL simply allocates the buffer. A later epic will address how a polyphonic
keyboard module populates those channels.
