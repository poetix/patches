---
id: "0012"
title: Remove redundant PortDirection from PortDescriptor
priority: low
created: 2026-02-28
depends_on: ["0011"]
epic: "E002"
---

## Summary

`ModuleDescriptor` already separates ports into `inputs` and `outputs` vecs/slices. The `direction` field on each `PortDescriptor` therefore always mirrors which collection the port is in — it carries no new information and is a source of potential inconsistency (nothing prevents placing an `Output`-direction port in the `inputs` collection). Remove `PortDirection` and the `direction` field from `PortDescriptor`.

## Acceptance criteria

- [ ] `PortDescriptor` has only a `name` field (no `direction`)
- [ ] `PortDirection` enum is removed from `patches-core`
- [ ] `patches-core` public re-exports updated (remove `PortDirection`)
- [ ] `patches-modules` re-exports updated
- [ ] All module implementations, graph code, builder code, and tests updated
- [ ] `cargo test` passes
- [ ] `cargo clippy` is clean

## Notes

**Depends on 0011** because 0011 changes `PortDescriptor::name` from `String` to `&'static str` — doing both in sequence avoids touching every `PortDescriptor` construction site twice.

If `PortDirection` is ever needed as a standalone concept (e.g. for a future DSL parser that describes ports before assigning them to input/output), it can be reintroduced at that point. For now it is dead weight.
