---
id: "0002"
title: Module graph structure
priority: high
created: 2026-02-28
depends_on: ["0001"]
epic: "E001"
---

## Summary

Implement a data structure representing a patch as a directed graph of modules connected by patch cables. This is the editable, in-memory representation of a patch — not an execution structure. Nodes are module instances; edges are connections from an output port on one module to an input port on another.

## Acceptance criteria

- [ ] `ModuleGraph` struct in `patches-core`
- [ ] Nodes: modules stored as `Box<dyn Module>` with a stable `NodeId` (e.g. a newtype over `usize`)
- [ ] Edges: directed connections from `(NodeId, port_name)` to `(NodeId, port_name)`, validated against the connected modules' `ModuleDescriptor`s at insertion time
- [ ] `add_module(module: Box<dyn Module>) -> NodeId`
- [ ] `connect(from: NodeId, output: &str, to: NodeId, input: &str) -> Result<(), GraphError>` — returns an error if port names are invalid
- [ ] `remove_module(id: NodeId)` — also removes all edges involving that node
- [ ] `disconnect(from: NodeId, output: &str, to: NodeId, input: &str)`
- [ ] Cycles are explicitly permitted — the graph does not validate for acyclicity
- [ ] `cargo test -p patches-core` passes
- [ ] `cargo clippy` is clean

## Notes

**No execution here.** `ModuleGraph` is a pure data structure for describing the patch topology. Execution ordering (toposort) and `SampleBuffer` allocation happen in the patch builder (0005).

**Multiple connections to one input:** Decide whether an input port may have more than one incoming connection. For a first pass, treat it as an error — one driver per input.

**Multiple connections from one output:** An output may fan out to multiple inputs.

**Cycles:** Because the `SampleBuffer` model gives every cable a 1-sample delay, cycles are safe to represent and execute. The graph does not need to detect or reject them.
