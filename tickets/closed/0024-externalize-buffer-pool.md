---
id: "0024"
epic: "E005"
title: Externalize buffer pool from ExecutionPlan into SoundEngine
priority: high
created: 2026-03-01
---

## Summary

`ExecutionPlan` currently owns the flat cable buffer pool (`buffers: Vec<[f32; 2]>`).
Because each re-plan allocates a fresh pool with all indices zeroed, every hot-reload
resets all cable values for at least one tick. Moving the pool into `SoundEngine`
(pre-allocated at construction with a configurable fixed capacity) is the prerequisite
for stable buffer indices across re-plans (ticket 0025). This ticket handles the
structural move: pool ownership, the `tick()` signature change, and zeroing freed
slots on plan acceptance (`to_zero`). Stable index allocation is handled separately
in ticket 0025; this ticket retains the current (fresh-zeroed) allocation behaviour
so changes can be merged independently.

## Acceptance criteria

- [ ] `SoundEngine` pre-allocates `pool: Box<[[f32; 2]]>` with a capacity supplied
      at construction (e.g. `SoundEngine::new(..., pool_capacity: usize)`)
- [ ] `ExecutionPlan` no longer has a `buffers` field; it holds only slot index
      vectors (`input_buffers`, `output_buffers`) as before
- [ ] `ExecutionPlan::tick()` accepts `pool: &mut [[f32; 2]]` as an additional
      parameter and reads/writes through it instead of `self.buffers`
- [ ] `ExecutionPlan` gains `to_zero: Vec<usize>` — indices to zero on plan
      acceptance; populated by the builder (initially all allocated indices, since
      stable allocation is not yet in place)
- [ ] On plan acceptance, the audio thread zeroes `pool[i]` for each `i` in
      `to_zero` before the first `tick()` with the new plan
- [ ] All existing tests updated for the new `tick(pool, wi)` signature
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

**Capacity choice.** A reasonable default is 4096 slots (≈ 4096 concurrent output
ports). Builders of `SoundEngine` should be able to override this. The capacity
includes the permanent-zero slot at index 0.

**to_zero semantics.** In this ticket, `to_zero` contains every allocated buffer
index (since all are freshly zeroed anyway). Ticket 0025 will narrow this to only
the indices recycled from the freelist, eliminating the discontinuity for stable
cables.

**Zeroing only on the audio thread.** The audio thread is the sole writer of the
pool. Zeroing from the control thread would be a data race. The `to_zero` list is
the mechanism by which the control thread instructs the audio thread to perform
zeroing safely, once, before the first tick with the new plan.
