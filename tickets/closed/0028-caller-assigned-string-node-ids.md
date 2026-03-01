---
id: "0028"
title: Caller-assigned string NodeIds
priority: high
created: 2026-03-01
epic: E005
---

## Summary

`ModuleGraph` currently allocates `NodeId`s internally via an auto-incrementing
counter, with `NodeId` wrapping a `usize`. This makes it impossible for the DSL
layer to control node identity. When graphs are built from DSL files, module
instance names in the DSL should serve as stable identifiers so the planner can
track which modules persist across edits to the patch. Change `NodeId` to wrap a
`String` and move id assignment out of `ModuleGraph` — callers pass the id in to
`add_module`.

## Acceptance criteria

- [ ] `NodeId` wraps `String` (not `usize`)
- [ ] `NodeId` is no longer `Copy` (it will be `Clone`)
- [ ] `ModuleGraph::add_module` takes a `NodeId` (or `impl Into<NodeId>`) and a module; no longer returns a `NodeId`
- [ ] `ModuleGraph::add_module` returns an error (new `GraphError` variant) if the id is already present in the graph
- [ ] `ModuleGraph` no longer has a `next_id` field
- [ ] All downstream code (`Planner`, `ExecutionPlan`, `PatchEngine`, `BufferAllocState`, tests, examples) updated to compile and pass
- [ ] `cargo test`, `cargo clippy` clean

## Notes

This is preparation for DSL-driven graph construction, where the DSL name of a
module instance (e.g. `"lfo1"`, `"vca"`) becomes its `NodeId`. Stable,
caller-controlled ids are what allow the planner to recognise that a module in a
new graph corresponds to a module in the previous graph, enabling state
preservation across hot-reloads.

`NodeId` losing `Copy` will ripple through code that currently copies it freely.
The main mitigation is that `NodeId` is `Clone` and relatively cheap to clone
(short strings). Consider providing a `From<&str>` impl for ergonomic
construction.

`BufferAllocState` keys on `(NodeId, usize)` — this will need to use the new
string-based `NodeId`. The stability guarantee is preserved: same DSL name →
same `NodeId` → same buffer slot across re-plans.
