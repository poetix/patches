---
id: "0098"
title: Extract compute_connectivity from build_slots
priority: medium
created: 2026-03-08
epic: E019
---

## Summary

`build_slots` inlines a scan of the edge list (lines 697–718) to determine
which input and output ports of a node have live connections, producing a
`PortConnectivity`. This is a self-contained pure function that can be extracted
and tested directly.

## Acceptance criteria

- [ ] `compute_connectivity(desc, node_id, edges) -> PortConnectivity` extracted
      as a private pure function.
- [ ] `build_slots` calls the new function in place of the inlined logic;
      behaviour is unchanged.
- [ ] Unit tests:
  - [ ] Node with no edges: all inputs and outputs false.
  - [ ] Single input connected: only that input true, all outputs false.
  - [ ] Single output connected: only that output true, all inputs false.
  - [ ] Multiple ports connected: correct subset marked true.
  - [ ] Edges for other nodes do not affect this node's connectivity.
  - [ ] Port matched by both name and index (no false positives from
        same-named ports with different indices).
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.
