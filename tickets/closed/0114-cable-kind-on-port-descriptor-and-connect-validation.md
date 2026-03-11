---
id: "0114"
title: "`CableKind` on `PortDescriptor`; kind enforcement in `connect()`"
priority: high
epic: "E022"
depends_on: ["0113"]
created: 2026-03-11
---

## Summary

Add `kind: CableKind` to `PortDescriptor` so that each port declares whether
it carries a mono or poly signal. Update `ModuleGraph::connect()` to look up
the source and destination port descriptors and return an error if their
`CableKind`s differ. Kind mismatches are therefore caught at graph-construction
time, making them impossible to reach the planner or the audio thread.

## Acceptance criteria

- [ ] `PortDescriptor` gains a `kind: CableKind` field:
      ```rust
      pub struct PortDescriptor {
          pub name: &'static str,
          pub index: usize,
          pub kind: CableKind,
      }
      ```
- [ ] All existing port declarations in `patches-modules` and `patches-core`
      set `kind: CableKind::Mono` (no behaviour change for current modules).
- [ ] `ModuleGraph::connect()` (or the equivalent connection API) returns a
      `GraphError::CableKindMismatch { from_port, to_port }` (or similar) when
      the source output port's `kind` differs from the destination input port's
      `kind`. The existing error type should be extended rather than replaced.
- [ ] `ModuleGraph::connect()` succeeds without change for mono-to-mono
      connections (the common case today).
- [ ] Unit tests:
      - Connecting two mono ports succeeds.
      - Connecting two poly ports succeeds.
      - Connecting a mono output to a poly input returns `Err(CableKindMismatch)`.
      - Connecting a poly output to a mono input returns `Err(CableKindMismatch)`.
- [ ] `cargo clippy` and `cargo test -p patches-core` clean.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

Port descriptors are accessed via `ModuleDescriptor`, which is returned by
`Module::describe()`. `ModuleGraph` stores node descriptors at insertion time
and can look them up during `connect()`.

The planner does not need to re-validate cable kinds: the graph invariant that
all connections are kind-compatible is established and maintained by
`ModuleGraph::connect()`.
