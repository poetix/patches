---
id: "0100"
title: Build edge lookup caches to eliminate O(N·E) scans in builder
priority: medium
created: 2026-03-09
epic: E019
depends-on: "0099"
---

## Summary

Several builder functions scan the full edge list once per node (or per input
port), producing O(N·E) or O(E·ΣP_in) complexity at build time. This is
acceptable for small patches but degrades on hot-reload of large graphs.
Two pre-built lookup structures, each constructed in a single O(E) pass, reduce
all per-node and per-port lookups to O(1).

## Current behaviour

- `compute_connectivity(desc, id, edges)` iterates all E edges per node. It is
  called in `classify_nodes` (once per surviving node) and again in the action
  phase for Install nodes and Update nodes with changed connectivity — worst
  case 2N calls × O(E) = **O(N·E)** total.
- `resolve_input_buffers(desc, id, edges, output_buf, graph)` does
  `edges.iter().find()` inside `desc.inputs.iter().map()`, giving
  O(E × P_in) per node and **O(E · ΣP_in)** overall. Inside the `find`
  closure, `resolve_edge_to_buffer` additionally calls `outputs.iter().position`
  on the source node's descriptor.

## Design

### Structure 1 — connectivity index (built from `edges` alone)

```rust
let mut connected_inputs:  HashSet<(NodeId, String, u32)> = HashSet::new();
let mut connected_outputs: HashSet<(NodeId, String, u32)> = HashSet::new();
for (from, out_name, out_idx, to, in_name, in_idx, _) in &edges {
    connected_inputs .insert((to.clone(),   in_name.clone(),  *in_idx));
    connected_outputs.insert((from.clone(), out_name.clone(), *out_idx));
}
```

`compute_connectivity` is rewritten to look up each port in the relevant set in
O(1), reducing its cost to O(P_in + P_out) per node.

### Structure 2 — input buffer map (built from `edges` + `output_buf`)

```rust
// Built after allocate_buffers.
let mut input_buffer_map: HashMap<(NodeId, String, u32), (usize, f32)> =
    HashMap::new();
for (from, out_name, out_idx, to, in_name, in_idx, scale) in &edges {
    // one position() lookup per edge (not per port):
    let from_desc = &graph.get_node(from)?.module_descriptor;
    let out_pos = from_desc.outputs.iter()
        .position(|p| p.name == out_name.as_str() && p.index == *out_idx)?;
    let buf = output_buf[&(from.clone(), out_pos)];
    input_buffer_map.insert((to.clone(), in_name.clone(), *in_idx), (buf, *scale));
}
```

`resolve_input_buffers` is rewritten to look up each input port in O(1).
`resolve_edge_to_buffer` is no longer needed and is removed.

Both structures are locals in `build_patch`; no public API changes.

## Acceptance criteria

- [ ] A connectivity index (`connected_inputs` / `connected_outputs` sets) is
      built once in a single pass over the edge list, before `classify_nodes`.
- [ ] `compute_connectivity` uses the index instead of scanning `edges`; the
      `edges` parameter is removed from its signature.
- [ ] An input buffer map is built once after `allocate_buffers`, before step C.
- [ ] `resolve_input_buffers` uses the map for O(1) per-port lookup; the `edges`
      and `graph` parameters are removed from its signature.
- [ ] `resolve_edge_to_buffer` is removed (its work is folded into the single
      map-construction pass).
- [ ] All existing tests pass unchanged.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.

## Notes

The `position()` call in map construction is per edge, not per (node, port)
pair — so it executes at most E times rather than E × ΣP_in times.

Both structures allocate `String` keys because `EdgeList` uses `String` for
port names. The allocation happens once per edge during the O(E) build pass
rather than once per (node × port) lookup, so the total allocation count falls
from O(E · ΣP_in) to O(E).

T-0101 (GraphIndex wrapper) can follow this ticket once the cache structures
exist as locals.
