---
id: "0086"
title: Remove `SawtoothOscillator` and `SquareOscillator`; migrate tests to `Oscillator`
priority: medium
created: 2026-03-06
epic: "E016"
depends_on: ["0085"]
---

## Summary

`patches-modules/src/waveforms.rs` contains `SawtoothOscillator` and
`SquareOscillator`. Now that `Oscillator` (T-0085) exposes `sawtooth` and
`square` outputs with equivalent functionality, these standalone modules are
redundant and should be removed. Their tests are rewritten to exercise the
corresponding outputs of `Oscillator`.

## Acceptance criteria

- [ ] `SawtoothOscillator` and `SquareOscillator` structs and their `impl Module`
      blocks are deleted from `waveforms.rs`.
- [ ] If `waveforms.rs` becomes empty after deletion, the file is removed and its
      `mod waveforms` declaration is removed from `lib.rs`.
- [ ] `SawtoothOscillator` and `SquareOscillator` are removed from the default
      module registry in `lib.rs`.
- [ ] All tests that previously covered `SawtoothOscillator` and `SquareOscillator`
      are ported to test the `sawtooth` (output index 2) and `square` (output index 3)
      ports of `Oscillator`. Test logic and coverage are preserved:
      - instance IDs are distinct between two `Oscillator` instances
      - descriptor ports include `sawtooth` and `square` outputs
      - a full sawtooth cycle is consistent across two periods
      - a full square cycle is consistent across two periods
      - square output values are only `+1.0` or `-1.0`
- [ ] `cargo build`, `cargo test`, `cargo clippy` pass with no new warnings.
- [ ] No references to `SawtoothOscillator` or `SquareOscillator` remain anywhere
      in the codebase (verified with `grep`).

## Notes

The `advance_phase` free function and the `C2_FREQ` constant in `waveforms.rs`
are private to that file; they can simply be deleted along with the module structs
rather than migrated.

If any other file in the workspace (e.g. integration tests, examples) references
`SawtoothOscillator` or `SquareOscillator` by name, those references must be
updated as part of this ticket.
