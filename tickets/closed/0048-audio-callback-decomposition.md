---
id: "0048"
epic: "E011"
title: Decompose AudioCallback.fill_buffer into named methods
priority: medium
created: 2026-03-03
---

## Summary

`AudioCallback::fill_buffer` contained three distinct concerns inline: adopting a
new plan, ticking the execution plan for a chunk of samples, and dispatching control
signals. This ticket extracts each into a named method and applies two small
micro-optimisations: replacing a per-callback division with a pre-computed right-shift,
and storing the shift width as a struct field so it is computed once at construction.

## Acceptance criteria

- [x] `receive_plan(&mut self)` extracted: adopts a new plan from the ring buffer if one is available; tombstones, installs new modules, zeros cable buffers, replaces `current_plan`.
- [x] `process_chunk<T>(&mut self, data, out_i, chunk)` extracted: the inner `for _ in 0..chunk` loop that ticks the plan and writes samples to the output slice.
- [x] `dispatch_signals(&mut self)` extracted: drains the signal ring buffer, delivering each `(InstanceId, ControlSignal)` pair to the current plan.
- [x] `channel_shift: u32` field added to `AudioCallback`, initialised as `channels.trailing_zeros()` in `new()`.
- [x] `data.len() / self.channels` replaced with `data.len() >> self.channel_shift` in `fill_buffer`.
- [x] `fill_buffer` reduced to the top-level control flow only, delegating all inner work to the three extracted methods.
- [x] `cargo clippy` clean, all tests pass.

## Notes

`channel_shift` is only correct when `channels` is a power of two, which is always
the case for standard audio configurations (mono = 1, stereo = 2, surround = 4/6/8).
The guard `if self.channels > 0` before the shift remains in place.
