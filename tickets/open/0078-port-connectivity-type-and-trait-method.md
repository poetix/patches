---
id: "0078"
title: PortConnectivity type and Module::set_connectivity
priority: medium
epic: "E015"
created: 2026-03-05
---

## Summary

Add the `PortConnectivity` struct to `patches-core` and a `set_connectivity`
method to the `Module` trait. This is the foundation for the rest of E015; no
planner changes are included here.

## Acceptance criteria

- [ ] `PortConnectivity` exists in `patches-core` with fields
      `inputs: Box<[bool]>` and `outputs: Box<[bool]>`.
- [ ] `PortConnectivity::new(n_inputs: usize, n_outputs: usize) -> Self` constructs
      an all-false instance of the correct size.
- [ ] `Module::set_connectivity` has the signature
      `fn set_connectivity(&mut self, connectivity: PortConnectivity) {}` with a
      default no-op body.
- [ ] The trait method is documented: implementations must not allocate, block, or
      perform I/O, as the method may be called on the audio thread.
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

`PortConnectivity` is indexed to match `ModuleDescriptor::inputs` and
`ModuleDescriptor::outputs`: `inputs[i]` corresponds to the i-th entry in
the descriptor's input port list, likewise for outputs.

No existing module needs to override `set_connectivity` as part of this ticket.
Modules that wish to use it (e.g. for coefficient caching or stereo mirroring) can
do so independently once the infrastructure exists.

See ADR 0013.
