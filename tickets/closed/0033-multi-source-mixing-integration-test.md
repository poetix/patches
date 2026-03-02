---
id: "0033"
epic: "E006"
title: Multi-source mixing integration test
priority: medium
created: 2026-03-02
---

## Summary

Integration test for a graph containing a `Mix` module with multiple input sources,
verifying correct stereo output and correct slot reuse for the mixer's output buffer
across a re-plan.

## Acceptance criteria

- [ ] Integration test: build a graph with two source modules feeding a `Mix` module;
      tick N samples and verify the mixed output matches the expected sum of the
      sources (within floating-point tolerance)
- [ ] Integration test: re-plan to the same graph and confirm the `Mix` module's
      output buffer slot is unchanged (stable index)
- [ ] Integration test: re-plan dropping one source and verify the remaining source's
      contribution is correct and the dropped source's buffer slot appears in `to_zero`
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

This is the first integration test to exercise a module with multiple inputs.
It doubles as a check that `input_scales` (from T-0021) are applied correctly
end-to-end.
