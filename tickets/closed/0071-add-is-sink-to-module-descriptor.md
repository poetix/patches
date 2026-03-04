---
id: "0071"
title: Add is_sink to ModuleDescriptor
priority: high
epic: "E014"
created: 2026-03-04
---

## Summary

Add an `is_sink: bool` field to `ModuleDescriptor` so the planner can identify the audio
output node from the graph's descriptors alone, without requiring a live module instance
or the `as_sink()` trait method.

## Acceptance criteria

- [ ] `ModuleDescriptor` has a pub `is_sink: bool` field.
- [ ] `AudioOut::describe()` returns a descriptor with `is_sink: true`.
- [ ] All other modules' `describe()` implementations return `is_sink: false`.
- [ ] Any existing code that constructs `ModuleDescriptor` directly (tests, builders)
      is updated to include the new field.
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

This is a compile-time constant — zero cost. The field replaces the need to call
`module.as_sink()` during planning, which required a live module instance. The `as_sink()`
method on `Module` is not removed by this ticket (it is still used by `ModulePool` for
sink caching on the audio thread); removal is a separate concern if desired later.

See ADR 0012 § "ModuleDescriptor gains is_sink".
