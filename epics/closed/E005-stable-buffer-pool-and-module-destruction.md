---
id: "E005"
title: Stable buffer pool and module destruction
created: 2026-03-01
closed: 2026-03-02
tickets: ["0024", "0025", "0026", "0028"]
---

## Summary

Re-planning (hot-reload) previously zeroed all cable buffers because `ExecutionPlan`
owned them and each build created a fresh allocation. This caused a 1-sample
discontinuity per graph hop on every re-plan. For audio-rate cables the discontinuity
is inaudible, but CV cables (slow-moving control signals modulating filter cutoffs,
VCA gain, etc.) may produce an audible click.

Copying old buffer values during `build_patch` is not safe: the audio thread
continuously writes to those buffers, making any read a data race. The fix
requires the buffer pool to outlive any single plan — living in `SoundEngine`
rather than inside `ExecutionPlan` — combined with a stable index-allocation
scheme so that unchanged cables always map to the same pool slot across re-plans.

T-0026 (module tombstoning and a `Module::destroy` hook) was investigated and
closed as won't-implement. Analysis showed that removed modules always drop on
the control thread via `Drop`, which is sufficient. See
`adr/0007-no-module-destroy-hook.md` for the full reasoning.

## Acceptance criteria

- [x] All four tickets resolved (T-0024, T-0025, T-0028 implemented; T-0026 won't-implement)
- [x] `cargo build`, `cargo test`, `cargo clippy` all clean
- [x] Re-planning with an unchanged cable produces no zeroing of that cable's buffer
- [x] Re-planning with a new cable starts that cable from zero

## Tickets

| ID   | Title                                         | Priority | Outcome         |
|------|-----------------------------------------------|----------|-----------------|
| 0024 | Externalize buffer pool from ExecutionPlan    | high     | implemented     |
| 0025 | Stable buffer index allocation                | high     | implemented     |
| 0026 | Module::destroy and tombstoning               | medium   | won't-implement |
| 0028 | Caller-assigned string NodeIds                | high     | implemented     |

## Architecture introduced

### Buffer pool in SoundEngine

`SoundEngine` pre-allocates a fixed-capacity `pool: Box<[[f32; 2]]>` at
construction. `ExecutionPlan` no longer owns `buffers`; it holds only indices into
the shared pool. `tick()` accepts `pool: &mut [[f32; 2]]` as a parameter.

Index 0 remains the permanent-zero slot (never written to).

### Stable index allocation via BufferAllocState

```rust
pub struct BufferAllocState {
    /// Stable (NodeId, output_port_index) → pool index mapping.
    pub output_buf: HashMap<(NodeId, usize), usize>,
    /// Recycled indices available for reuse.
    pub freelist: Vec<usize>,
    /// Next unallocated index (high-water mark). Starts at 1.
    pub next_hwm: usize,
}
```

`build_patch` is extended to accept `&BufferAllocState` and return
`(ExecutionPlan, BufferAllocState)`. Per output port in the new plan:

- Unchanged cable `(NodeId, port_idx)` present in `old.output_buf` → **reuse**
  the existing index (no discontinuity, no zeroing needed).
- New cable not in `old.output_buf` → **acquire** from freelist (if non-empty)
  or increment `next_hwm` (error if `>= pool_capacity`).

Per output port in the *old* plan absent from the new graph → **release** its
index back to the freelist, and include it in `ExecutionPlan::to_zero` so the
audio thread zeroes it on acceptance.

`PatchEngine` threads `BufferAllocState` forward across re-plans. The initial
state has an empty `output_buf`, empty `freelist`, and `next_hwm = 1`.

### Buffer zeroing on plan acceptance

`ExecutionPlan` gains `to_zero: Vec<usize>` — the indices freed in the most recent
re-plan. The audio thread zeroes `pool[i]` for each `i` in `to_zero` immediately
on accepting the new plan, before the first `tick()`. This ensures recycled slots
never carry stale data into a new connection.

Zeroing happens at *release time* (when a connection is removed) rather than at
*acquisition time* (when a slot is recycled), so slots are clean even if they sit
in the freelist across multiple re-plans before being reused.

### Module destruction (won't-implement)

`Module::destroy` and a cleanup thread were not added. Removed modules are always
extracted from the held plan (control thread) and drop there via `Drop`, which is
safe and sufficient. See `adr/0007-no-module-destroy-hook.md`.

## Notes

Trade-offs are documented in `adr/0004-stable-buffer-pool-and-module-lifecycle.md`
and `adr/0007-no-module-destroy-hook.md`.

**No new external crates.** All changes are within existing crates.

**`Planner` remains pure.** `BufferAllocState` is an explicit input/output of
`build_patch`; no global mutable state is introduced.

**Fixed pool capacity.** The pool does not grow dynamically (dynamic growth would
require reallocation, moving memory the audio thread holds references into). A
generous capacity is chosen at engine construction; the freelist ensures the index
space is not exhausted over the engine's lifetime even with many re-plans.
