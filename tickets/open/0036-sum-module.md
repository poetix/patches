---
id: "0036"
epic: "E007"
title: Add Sum module, remove Mix
priority: medium
created: 2026-03-02
depends_on: "0035"
---

## Summary

Replace the existing `Mix` module (fixed 2-input average) with a `Sum` module that
accepts a configurable number of inputs and produces their unscaled sum. The number
of inputs is fixed at construction time; the module's descriptor is built
dynamically to list ports `in/0` … `in/(size-1)` and a single output `out/0`.

## Acceptance criteria

- [ ] `patches-modules` gains a `Sum` struct. `Sum::new(size: usize)` constructs a
      module with:
      - `size` input ports: `PortDescriptor { name: "in", index: 0 }` …
        `PortDescriptor { name: "in", index: size - 1 }`.
      - 1 output port: `PortDescriptor { name: "out", index: 0 }`.
- [ ] `Sum::process` sums all `inputs[0..size]` into `outputs[0]`. No division.
- [ ] `Mix` is removed from `patches-modules`. `patches-modules::lib.rs` exports
      `Sum` in its place. Any internal or test use of `Mix` is updated to `Sum`.
- [ ] Tests cover:
      - Descriptor shape (correct port counts, names, and indices).
      - `Sum::new(1)` passes its single input unchanged.
      - `Sum::new(3)` with inputs `[0.2, 0.3, 0.5]` produces `1.0`.
      - Distinct `InstanceId`s across multiple `Sum::new` calls.
- [ ] `cargo clippy` clean, `cargo test` green across the workspace.

## Notes

**Semantic difference from Mix:** `Mix` produced `(a + b) / 2.0` (normalised
blend). `Sum` produces the raw sum with no normalisation. Callers that previously
used `Mix` for amplitude blending will need to apply scaling via edge `scale`
values or an explicit gain module if they want to stay in [-1, 1].

**Why remove Mix?** `Sum(2)` subsumes `Mix`'s graph topology; the averaging
behaviour belongs to the caller's scaling strategy rather than the module.

**`size = 0`:** Treat as a valid no-op: descriptor has no inputs and output is 0.
The `process` body handles an empty slice naturally.
