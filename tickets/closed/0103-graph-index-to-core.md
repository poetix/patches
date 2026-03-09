---
id: "0103"
title: Move GraphIndex and ResolvedGraph to patches-core
priority: medium
created: 2026-03-09
epic: E020
depends-on: "0102"
---

## Summary

`GraphIndex` and `ResolvedGraph` are pure `ModuleGraph` query helpers — they
index the edge list for O(1) connectivity lookups and resolve cable buffer
slots. Neither type references `ExecutionPlan`, `ModulePool`, or any
audio-backend type. They belong in `patches-core` alongside the graph types
they wrap.

The `build_input_buffer_map` free function (currently in `builder.rs`) is an
implementation detail of `ResolvedGraph::build` and moves with it.

## Acceptance criteria

- [ ] `GraphIndex<'a>` and `ResolvedGraph<'a>` are defined in `patches-core`
      (in the planner module introduced by T-0102, or a sibling module).
- [ ] `GraphIndex::build`, `GraphIndex::get_node`, and
      `GraphIndex::compute_connectivity` are defined in core.
- [ ] `ResolvedGraph::build` and `ResolvedGraph::resolve_input_buffers` are
      defined in core.
- [ ] `build_input_buffer_map` moves to core as a private helper of
      `ResolvedGraph::build`.
- [ ] `patches-engine` imports these types from `patches-core`; its own
      definitions are deleted.
- [ ] The `#[cfg(test)]` helpers `graph_index_for_test` and
      `resolved_graph_for_test` in `builder.rs` move to the relevant test
      modules in `patches-core`.
- [ ] All existing tests pass unchanged.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.
