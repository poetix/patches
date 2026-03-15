---
id: "0013"
title: Introduce Sink trait to decouple engine from AudioOut
priority: medium
created: 2026-02-28
depends_on: ["0011"]
epic: "E002"
---

## Summary

`build_patch` identifies the `AudioOut` node by downcasting via `as_any().downcast_ref::<AudioOut>()`, and `ExecutionPlan::last_left/last_right` downcast on every call. This creates a hard dependency from `patches-engine` to the concrete `AudioOut` type in `patches-modules`, making it impossible to substitute a different sink (e.g. a file-writer sink, a test sink, or a multi-output sink) without modifying the builder.

Introduce a `Sink` marker trait in `patches-core` that `AudioOut` (and any future sink) implements. The builder finds the sink via trait-object downcasting to `dyn Sink` rather than to a concrete type, removing `patches-engine`'s knowledge of `AudioOut`.

## Acceptance criteria

- [ ] `Sink` trait defined in `patches-core`, extending `Module`:
  ```rust
  pub trait Sink: Module {
      fn last_left(&self) -> f32;
      fn last_right(&self) -> f32;
  }
  ```
- [ ] `Module` trait gains `fn as_sink(&self) -> Option<&dyn Sink>` (default returns `None`)
- [ ] `AudioOut` implements `Sink` and overrides `as_sink` to return `Some(self)`
- [ ] `build_patch` uses `as_sink()` instead of `as_any().downcast_ref::<AudioOut>()`
- [ ] `ExecutionPlan::last_left/last_right` use `as_sink()` instead of downcasting to `AudioOut`
- [ ] `patches-engine/Cargo.toml` no longer depends on `patches-modules` (or the dependency is reduced to dev-dependencies for tests only)
- [ ] All existing tests pass
- [ ] `cargo clippy` is clean

## Notes

**Depends on 0011** because the `Module` trait signature is being changed there; adding `as_sink` at the same time avoids a second round of trait-method additions.

**Why not just use `as_any`?** `as_any` requires the caller to know the concrete type. A `Sink` trait lets the builder ask "are you a sink?" without knowing *which* sink it is. This is the standard Rust pattern for open-ended extension of trait objects.

**Removing the `patches-modules` dependency from `patches-engine`:** Currently `patches-engine` depends on `patches-modules` solely to import `AudioOut` for downcasting. After this change, `patches-engine` only needs `patches-core` (for `Sink`, `Module`, `ModuleGraph`, etc.). The `patches-modules` dependency can move to `[dev-dependencies]` (needed for builder tests that construct real modules).

**`as_any` can be removed later** if no other code uses it, but that is out of scope for this ticket.
