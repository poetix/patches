---
id: "0027"
title: Remove as_any_mut from Module trait
priority: low
created: 2026-03-01
---

## Summary

`Module::as_any_mut` was added to support downcasting to a mutable concrete type,
but the only call sites are two test assertions in `planner.rs` that only read a
field after downcasting. Switching those to `iter()` + `as_any()` + `downcast_ref`
eliminates all callers, allowing `as_any_mut` to be removed from the trait and all
implementors.

## Acceptance criteria

- [ ] `as_any_mut` removed from the `Module` trait in `patches-core`
- [ ] `as_any_mut` removed from all implementors (`Oscillator`, `Mix`, `AudioOut`, stub in `graph.rs`, stubs in `planner.rs`)
- [ ] Both test assertions in `planner.rs` use `iter()` + `as_any()` + `downcast_ref`
- [ ] `cargo clippy` and `cargo test` pass

## Notes

`as_any` (shared reference) must stay — it is used in `builder.rs` for legitimate
downcast-to-concrete logic in the production path.
