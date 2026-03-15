---
id: "0097"
title: Extract resolve_input_buffers and partition_inputs from build_slots
priority: medium
created: 2026-03-08
epic: E019
---

## Summary

`build_slots` contains a deeply nested iterator chain (lines 624–676) that
resolves each input port to a `(buffer_index, scale)` pair by scanning the edge
list, then partitions those pairs into unscaled and scaled lists. These two
concerns are independently useful and independently testable but are currently
inlined with no named boundary.

## Acceptance criteria

- [ ] `resolve_input_buffers(desc, node_id, edges, output_buf, graph)` extracted
      as a private function returning `Result<Vec<(usize, f32)>, BuildError>` —
      one entry per input port, defaulting to `(0, 1.0)` for unconnected ports.
- [ ] `partition_inputs(resolved: Vec<(usize, f32)>)` extracted as a private
      pure function returning
      `(Vec<(usize, usize)>, Vec<(usize, usize, f32)>)` (unscaled, scaled).
- [ ] `build_slots` calls the two new functions in place of the inlined logic;
      behaviour is unchanged.
- [ ] Unit tests for `resolve_input_buffers`:
  - [ ] Unconnected port maps to buffer 0, scale 1.0.
  - [ ] Connected port resolves to the correct buffer index and scale.
  - [ ] Multiple input ports resolved independently.
  - [ ] Missing source node or buffer returns `BuildError::InternalError`.
- [ ] Unit tests for `partition_inputs`:
  - [ ] Scale-1.0 entries go to unscaled list with correct `(port, buf)`.
  - [ ] Non-1.0 entries go to scaled list with correct `(port, buf, scale)`.
  - [ ] Mixed input produces correct split.
  - [ ] Empty input produces two empty lists.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.
