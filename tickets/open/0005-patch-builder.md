---
id: "0005"
title: Patch builder (toposort and execution plan)
priority: high
created: 2026-02-28
depends_on: ["0002", "0003", "0004"]
epic: "E001"
---

## Summary

Implement a patch builder that consumes a `ModuleGraph` and produces an `ExecutionPlan`: a fully resolved, execution-ready structure containing an ordered list of modules, pre-allocated `SampleBuffer`s for every connection, and pre-resolved input/output buffer mappings for each module. The engine runs the execution plan directly without consulting the graph again.

This component lives in a new `patches-engine` crate (see Notes) because it must know about both `patches-core` types and concrete module types from `patches-modules` (specifically `AudioOut`) to validate and read from the patch output.

## Acceptance criteria

- [ ] New `patches-engine` crate created in the workspace, depending on `patches-core` and `patches-modules`
- [ ] `PatchBuilder` (or a free function `build_patch`) in `patches-engine` that takes a `ModuleGraph` and returns `Result<ExecutionPlan, BuildError>`
- [ ] Validates that exactly one `AudioOut` node is present (identified by type); returns `BuildError` otherwise
- [ ] Performs a topological sort of the graph to determine module execution order — because cycles are permitted, use a cycle-tolerant ordering (e.g. Kahn's algorithm, falling back to an arbitrary order for nodes involved in cycles)
- [ ] Allocates one `SampleBuffer` per directed edge in the graph
- [ ] Produces per-module input/output buffer assignments: for each module in execution order, a list of references (or indices) into the shared `SampleBuffer` pool — one per input port and one per output port
- [ ] Unconnected input ports are assigned a permanently-zero buffer; unconnected output ports are assigned a scratch buffer
- [ ] `ExecutionPlan` is a self-contained structure: the engine needs no other information to run the patch for N samples
- [ ] `cargo test -p patches-engine` passes, including at least:
      - a test building a minimal plan (sine → audio out) and verifying execution order and buffer assignments
- [ ] `cargo clippy` is clean

## Notes

**Crate structure rationale:** `patches-core` defines module traits and graph types but must not depend on `patches-modules`. `patches-modules` implements concrete modules and depends on `patches-core`. `patches-engine` depends on both, allowing it to identify `AudioOut` by type and read left/right samples from it after each tick.

**Cycle handling in toposort:** A standard DFS toposort will fail on cyclic graphs. Use Kahn's algorithm: process nodes with zero in-degree first; any nodes remaining after the queue empties form cycles and can be appended in arbitrary order. The 1-sample `SampleBuffer` delay makes this correct regardless of the order chosen for cyclic nodes.

**`ExecutionPlan` ownership:** The plan takes ownership of the modules out of the `ModuleGraph`. The graph is consumed by the builder. If the patch needs to be rebuilt (e.g. on hot-reload), a new `ModuleGraph` is constructed and a new `ExecutionPlan` is built from it.

**No allocation during execution:** All `SampleBuffer`s are allocated here, at plan-build time. The engine's per-sample tick must not allocate.
