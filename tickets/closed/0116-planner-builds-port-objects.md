---
id: "0116"
title: "Planner builds port objects; `ExecutionPlan::port_updates`"
priority: high
epic: "E022"
depends_on: ["0114", "0115"]
created: 2026-03-11
---

## Summary

Extend the planner to compute `InputPort` and `OutputPort` values for each
module during `build_slots`, store them in `NodeState` for change detection,
and emit `port_updates` in `ExecutionPlan` so that `set_ports` is called on the
audio thread during plan-accept (step 3 of the accept sequence).

## Acceptance criteria

- [ ] During `build_slots`, for each node the planner computes a `Vec<InputPort>`
      and `Vec<OutputPort>`:
      - For each input port declared by the module, look up the port's
        `CableKind` from its `PortDescriptor`. Construct the matching concrete
        type (`MonoInput` or `PolyInput`) with `cable_idx` from the buffer pool
        allocation, `scale` from the edge, and `connected: true` if an edge
        targets this port or `connected: false` if not. Wrap in the
        corresponding `InputPort` enum variant.
      - For each output port, similarly construct `MonoOutput` or `PolyOutput`
        with `connected: true` iff any edge originates from this port. Wrap in
        the `OutputPort` enum variant.
- [ ] `NodeState` stores `input_ports: Vec<InputPort>` and
      `output_ports: Vec<OutputPort>` for change detection across successive
      builds.
- [ ] For new modules (present in `new_modules`), `set_ports` is called on the
      boxed module before it is inserted into the pool.
- [ ] `ExecutionPlan` gains:
      ```rust
      pub port_updates: Vec<(usize, Vec<InputPort>, Vec<OutputPort>)>
      ```
      where the `usize` is the pool slot index. Only surviving modules whose
      port assignments changed since the last plan emit an entry.
- [ ] `ModulePool` (or `AudioCallback::receive_plan`) applies `port_updates`
      during plan-accept step 3 by calling `module.set_ports(&inputs, &outputs)`
      for each entry, using `if let Some(m)` — no `unwrap()`.
- [ ] `ExecutionPlan::connectivity_updates` is removed (superseded by
      `port_updates`). Any remaining references in `HeadlessEngine`,
      `AudioCallback`, and tests are updated.
- [ ] `cargo clippy` and `cargo test` clean across all crates.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

The plan-accept sequence after this ticket:

1. Accept new plan from ring buffer.
2. Apply parameter updates (`set_parameter` calls).
3. Apply port updates (`set_ports` calls).
4. Begin ticking.

`HeadlessEngine::adopt_plan` in `patches-integration-tests/src/lib.rs` must
also be updated to apply `port_updates` in step 3, replacing the current
`connectivity_updates` application.

The planner determines `InputPort` variant from the cable kind recorded in the
buffer pool allocation, which is determined by the destination port's
`CableKind` (validated to match the source's `CableKind` by `connect()` in
T-0114).
