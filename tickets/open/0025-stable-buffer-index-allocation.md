---
id: "0025"
epic: "E005"
title: Stable buffer index allocation
priority: high
created: 2026-03-01
---

## Summary

With the buffer pool externalized into `SoundEngine` (ticket 0024), cable buffers
survive across re-plans. But the index each cable maps to still changes on every
re-plan because `build_patch` currently assigns indices in a fresh sequential pass.

This ticket introduces `BufferAllocState` — explicit state that `build_patch` accepts
as input and returns as output — so that cables whose `(NodeId, output_port_index)`
key is unchanged across a re-plan reuse the same pool index. The audio thread reads
and writes the same memory before and after the swap, eliminating any discontinuity
for stable connections. New or recycled indices are included in `ExecutionPlan::to_zero`
(introduced in ticket 0024) so the audio thread zeroes them before the first tick.

`build_patch` remains a pure function; `PatchEngine` threads `BufferAllocState`
forward across re-plans.

## Acceptance criteria

- [ ] `BufferAllocState` added (in `patches-engine`) with fields:
      - `output_buf: HashMap<(NodeId, usize), usize>` — stable index map
      - `freelist: Vec<usize>` — recycled indices available for reuse
      - `next_hwm: usize` — high-water mark; starts at 1 (`Default` impl)
- [ ] `build_patch` signature updated to accept `alloc: &BufferAllocState` and
      return `(ExecutionPlan, BufferAllocState)` (in addition to `BuildError`)
- [ ] Allocation logic per output port in the new plan:
      - Key exists in `alloc.output_buf` → reuse existing index (not in `to_zero`)
      - Key is new → pop from `alloc.freelist`; if empty, use `alloc.next_hwm`
        and increment; error (`BuildError::PoolExhausted`) if `>= pool_capacity`
- [ ] Deallocation logic: output ports present in `alloc.output_buf` but absent
      from the new graph have their indices pushed onto the new freelist and appended
      to `ExecutionPlan::to_zero`
- [ ] `Planner` updated to hold and pass `BufferAllocState` across `build()` calls
- [ ] `PatchEngine` updated accordingly
- [ ] Unit test: build plan A; re-plan to plan B with one module unchanged, one
      added, one removed; assert unchanged cable's buffer index is identical in
      both plans; assert removed cable's index appears in `plan_b.to_zero`
- [ ] Unit test: run the engine through many re-plans that add and remove modules;
      assert `next_hwm` does not grow unboundedly (freelist recycles indices)
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

**Depends on ticket 0024** (pool externalized, `to_zero` field exists).

**`NodeId` stability.** The stable-index scheme relies on `NodeId` being stable
for a given logical module across re-plans. This is guaranteed if callers reuse
the same module instances (as they do when reconstructing the graph from a DSL
source — module identity is tied to graph position or name, not the `NodeId`
counter). If a module is removed and re-added at the same logical position, it
will receive a new `NodeId` and a new buffer slot; the old slot will be zeroed.

**Freelist ordering.** Freelist order (LIFO via `Vec::pop`) is unspecified; any
recycled index is valid. LIFO is cache-friendlier (recently freed slots are more
likely to be in cache when reused).

**`BufferAllocState` is not `Send`.** It lives on the control thread only, never
crosses the real-time boundary.
