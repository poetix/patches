---
id: "E020"
title: Move decision-phase logic to patches-core
created: 2026-03-09
tickets: ["0102", "0103", "0104", "0105"]
---

## Summary

The decision phase of `PatchBuilder::build_patch` — graph indexing, node
classification, buffer allocation, and the planner state it threads across
builds — has no dependency on audio backends, `ModulePool`, `ExecutionPlan`,
`cpal`, or `rtrb`. It is pure graph reasoning and stable-index bookkeeping.
It belongs in `patches-core`.

The action phase (minting `InstanceId`s, calling the registry, assembling
`ModuleSlot`s, producing `ExecutionPlan`) remains in `patches-engine` because
`ExecutionPlan` is shaped to the engine's specific consumption pattern and
references `ModulePool`.

## Boundary

**Moves to `patches-core`:**

- Planner state types: `NodeState`, `PlannerState`, `BufferAllocState`,
  `ModuleAllocState`, `ModuleAllocDiff`
- Graph query helpers: `GraphIndex`, `ResolvedGraph`
- Decision types and logic: `NodeDecision`, `classify_nodes`, `allocate_buffers`
- Top-level decision entry point: `PlanDecisions`, `make_decisions`
- Decision-phase error type: `PlanError`

**Stays in `patches-engine`:**

- `ExecutionPlan`, `ModuleSlot` — shaped for engine consumption
- `PatchBuilder` action phase — minting ids, registry calls, slot assembly
- Action-phase error variants: `ModulePoolExhausted`, `ModuleCreationError`

**Removed from `NodeState`:**

`pool_index: usize` is an engine-pool detail that the decision phase never
reads. It is dropped from the core struct; the action phase already recovers
it from `module_diff.slot_map`.

## Tickets

| ID   | Title                                                         | Priority | Depends on |
|------|---------------------------------------------------------------|----------|------------|
| 0102 | Move planner state types to `patches-core`                   | medium   | —          |
| 0103 | Move `GraphIndex` and `ResolvedGraph` to `patches-core`      | medium   | 0102       |
| 0104 | Move `NodeDecision`, `classify_nodes`, and `allocate_buffers` to `patches-core` | medium | 0103 |
| 0105 | Move `PlanDecisions` and `make_decisions` to `patches-core`; slim engine builder | medium | 0104 |

## Definition of done

- All decision-phase types and functions live in `patches-core`.
- `patches-core` has no dependency on `patches-engine` (unchanged).
- `patches-engine/src/builder.rs` contains only the action phase and
  `ExecutionPlan`-related types.
- `cargo build`, `cargo test`, `cargo clippy` clean across all crates.
- No `unwrap()` or `expect()` in library code.
