---
id: "0011"
title: Use static port names and return ModuleDescriptor by reference
priority: medium
created: 2026-02-28
depends_on: []
epic: "E002"
---

## Summary

Every call to `descriptor()` allocates a new `Vec<PortDescriptor>` containing heap-allocated `String`s. Port names are compile-time constants for every module in the system. Change `PortDescriptor::name` to `&'static str`, store each module's descriptor as a constant or field, and return `&ModuleDescriptor` from the trait method instead of an owned value. This eliminates all allocations from descriptor access and makes the descriptor API zero-cost.

## Acceptance criteria

- [ ] `PortDescriptor::name` is `&'static str` (was `String`)
- [ ] `ModuleDescriptor` uses `&'static [PortDescriptor]` (or `Vec<PortDescriptor>`) for its `inputs` and `outputs` fields — prefer slices if all descriptors can be `const`/`static`, otherwise `Vec` is acceptable
- [ ] `Module::descriptor()` returns `&ModuleDescriptor` (was `ModuleDescriptor` by value)
- [ ] All module implementations (`SineOscillator`, `AudioOut`, `Crossfade`) updated to store or reference a static descriptor
- [ ] `ModuleGraph::connect` and `build_patch` updated to work with borrowed descriptors
- [ ] All graph tests and their `StubModule` updated
- [ ] `cargo test` passes
- [ ] `cargo clippy` is clean

## Notes

**Approach:** The simplest path is to have each module store its `ModuleDescriptor` as a field initialised at construction time (with `&'static str` names), and return `&self.descriptor` from the trait method. This avoids needing `lazy_static` or complex `const` construction.

**Impact radius:** This changes the `Module` trait signature, so every `impl Module` block must be updated. The graph's `connect` method calls `descriptor()` and iterates over ports — it will work with references without further changes. The builder snapshots descriptors into a `HashMap<NodeId, ModuleDescriptor>` — this will need to clone or change to store references.

**Alternative considered:** Using `Cow<'static, str>` for the name. This adds complexity for no benefit since all port names in practice are string literals.
