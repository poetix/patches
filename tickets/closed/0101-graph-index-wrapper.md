---
id: "0101"
title: Encapsulate edge lookup caches in GraphIndex and ResolvedGraph
priority: low
created: 2026-03-09
epic: E019
depends-on: "0100"
---

## Summary

After T-0100 the edge lookup caches exist as locals in `build_patch`. This
ticket promotes them into named types so the decision-phase functions carry
coherent, self-describing arguments rather than a proliferation of raw
collections. It also prevents the raw `edges` slice from leaking into callsites
where only the indexed form is needed.

## Design

### `GraphIndex<'a>`

Holds the graph reference and the connectivity sets built from the edge list.
Available immediately after `graph.edge_list()` is called.

```rust
struct GraphIndex<'a> {
    graph: &'a ModuleGraph,
    connected_inputs:  HashSet<(NodeId, String, u32)>,
    connected_outputs: HashSet<(NodeId, String, u32)>,
}

impl<'a> GraphIndex<'a> {
    fn build(graph: &'a ModuleGraph) -> Self { ... }
    fn get_node(&self, id: &NodeId) -> Option<&'a Node> { ... }
    fn compute_connectivity(&self, desc: &ModuleDescriptor, id: &NodeId)
        -> PortConnectivity { ... }
}
```

`classify_nodes` takes `&GraphIndex<'a>` instead of `(graph, edges, ...)`.

### `ResolvedGraph<'a>`

Extends `GraphIndex` with the input buffer map, which requires `output_buf` and
is therefore only available after `allocate_buffers`.

```rust
struct ResolvedGraph<'a> {
    index: &'a GraphIndex<'a>,
    input_buffer_map: HashMap<(NodeId, String, u32), (usize, f32)>,
}

impl<'a> ResolvedGraph<'a> {
    fn build(
        index: &'a GraphIndex<'a>,
        output_buf: &HashMap<(NodeId, usize), usize>,
    ) -> Result<Self, BuildError> { ... }

    fn resolve_input_buffers(
        &self,
        desc: &ModuleDescriptor,
        node_id: &NodeId,
    ) -> Vec<(usize, f32)> { ... }
}
```

### Updated `build_patch` shape

```rust
let index = GraphIndex::build(graph);
// ... find_sink, compute_order ...
let buf_alloc = self.allocate_buffers(&index, &order, &prev_state.buffer_alloc)?;

// Decision phase
let decisions = classify_nodes(&index, &order, prev_state)?;

// Action phase
let resolved = ResolvedGraph::build(&index, &buf_alloc.output_buf)?;
// ... step A, B, C — slot assembly calls resolved.resolve_input_buffers(...)
```

`allocate_buffers` takes `&GraphIndex` instead of `&ModuleGraph` directly (it
only needs `get_node`, which `GraphIndex` delegates).

## Acceptance criteria

- [ ] `GraphIndex<'a>` struct defined and constructed via `GraphIndex::build`.
- [ ] `compute_connectivity` is a method on `GraphIndex`; free-function form
      removed.
- [ ] `classify_nodes` takes `&GraphIndex<'a>` instead of separate `graph` /
      `edges` parameters.
- [ ] `ResolvedGraph<'a>` struct defined and constructed from `&GraphIndex` +
      `output_buf`.
- [ ] `resolve_input_buffers` is a method on `ResolvedGraph`; free-function form
      removed.
- [ ] `build_patch` constructs `GraphIndex` then `ResolvedGraph` at the
      appropriate phase boundaries.
- [ ] `allocate_buffers` takes `&GraphIndex` (or `&ModuleGraph` via
      `GraphIndex::graph` — whichever is cleaner).
- [ ] Raw `edges: &EdgeList` parameter removed from all internal function
      signatures.
- [ ] All existing tests pass unchanged.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.

## Notes

`GraphIndex` and `ResolvedGraph` are `pub(crate)` — they are builder
implementation details, not part of the public API.

The naming is intentionally staged: `GraphIndex` expresses "I have indexed the
graph for fast queries", `ResolvedGraph` expresses "I have also resolved cables
to buffer slots". If the two types feel like too much ceremony for the codebase,
they can be collapsed into a single `PreparedGraph` constructed in two steps
(connectivity index first, then extended with buffer resolution). Record the
decision in an ADR if the trade-off is non-obvious.
