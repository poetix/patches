---
id: "0007"
title: Sine tone smoke-test example
priority: medium
created: 2026-02-28
depends_on: ["0006"]
epic: "E001"
---

## Summary

Add a `sine_tone` example binary to `patches-engine` that exercises the full
stack end-to-end: builds a sine oscillator → AudioOut graph, runs the
`SoundEngine` for three seconds, then stops cleanly. This acts as a hardware
validation that the entire E001 epic produces audible output.

## Acceptance criteria

- [ ] `patches-engine/examples/sine_tone.rs` compiles with `cargo build --example sine_tone`
- [ ] Running `cargo run --example sine_tone` plays a 440 Hz sine wave for ~3 seconds then exits cleanly
- [ ] No `unwrap`/`expect` in the example — errors are propagated or printed and the process exits with a non-zero code
- [ ] `cargo clippy` remains clean

## Notes

The example lives in `patches-engine` because that crate already depends on
`patches-core`, `patches-modules`, and `cpal`. No new dependencies are needed.
