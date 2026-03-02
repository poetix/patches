---
id: "0032"
epic: "E006"
title: Stable buffer indices end-to-end
priority: medium
created: 2026-03-02
---

## Summary

End-to-end check that a cable surviving a re-plan reads from the same pool slot
before and after, producing no discontinuity in the output signal. Complements the
unit-level `BufferAllocState` tests with a full-stack scenario exercised through
`HeadlessEngine`.

## Acceptance criteria

- [ ] Integration test: build a two-node graph (source → sink); tick N samples;
      re-plan to the same graph; confirm the cable's pool slot index is unchanged
      across the re-plan
- [ ] Integration test: confirm the output signal is continuous across the re-plan
      (no sudden jump or zeroing in the samples immediately after the swap)
- [ ] Integration test: add a new cable in the re-plan and confirm it starts from
      zero
- [ ] Integration test: remove a cable in the re-plan and confirm its former slot
      is zeroed on plan acceptance (present in `to_zero`)
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

Pool slot indices are accessible via `BufferAllocState::output_buf`. The test can
compare the state returned by successive `build_patch` calls before passing each
plan to `HeadlessEngine`.
