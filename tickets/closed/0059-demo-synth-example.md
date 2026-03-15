---
id: "0059"
title: Demo synth example
priority: high
epic: "E012"
created: 2026-03-03
---

## Summary

Add `patches-engine/examples/demo_synth.rs` that wires a `ClockSequencer`,
`StepSequencer`, `AdsrEnvelope`, `SawtoothOscillator`, `SquareOscillator`, `Vca`,
`Sum`, and `AudioOut` into a complete synthesiser that plays a 16-step minor-pentatonic
phrase for approximately 4 bars at 120 BPM then exits cleanly. This is the first
end-to-end demo of sequenced, enveloped synthesis in the Patches system.

## Acceptance criteria

- [ ] `cargo run --example demo_synth` compiles and runs without panics.
- [ ] Audible, pitched, enveloped output is produced for at least two bar cycles.
- [ ] Graph wiring matches the topology in E012 (clock → sequencer → oscillators + envelope → VCA → out).
- [ ] No `unwrap()` or `expect()` in example body; uses `run() -> Result<…>` + `main()` pattern.
- [ ] Exits cleanly (not with `process::exit`) after the configured run time.
- [ ] `cargo clippy` clean (no warnings in example).

## Notes

**Graph topology**

```
ClockSequencer (120 BPM, 4/4)
  semiquaver → StepSequencer.clock

StepSequencer (16-step pattern)
  pitch   → SawtoothOscillator.voct
  pitch   → SquareOscillator.voct
  trigger → AdsrEnvelope.trigger
  gate    → AdsrEnvelope.gate

SawtoothOscillator.out → Sum(2).in/0   (scale 0.5)
SquareOscillator.out   → Sum(2).in/1   (scale 0.5)

Sum.out          → Vca.in
AdsrEnvelope.out → Vca.cv

Vca.out → AudioOut.left
Vca.out → AudioOut.right
```

Scale `0.5` on each oscillator input to Sum keeps the mixed signal within `[-1.0, 1.0]`.

**Pattern**

```rust
const PATTERN: &[&str] = &[
    "C3", "Eb3", "F3", "G3", "_",   "Bb3", "_",  "C4",
    "-",  "G3",  "F3", "Eb3", "_",  "C3",  "_",  "-",
];
```

**Parameters**

```rust
const BPM: f32 = 120.0;
const BEATS_PER_BAR: u32 = 4;
const QUAVERS_PER_BEAT: u32 = 2;

const ATTACK_SECS: f32  = 0.005;
const DECAY_SECS: f32   = 0.050;
const SUSTAIN: f32      = 0.6;
const RELEASE_SECS: f32 = 0.100;

const RUN_SECS: u64 = 8;   // ~4 bars at 120 BPM
```

**Node IDs**

Use descriptive string IDs: `"clock"`, `"seq"`, `"saw"`, `"sq"`, `"mix"`, `"env"`,
`"vca"`, `"out"`.

Depends on tickets 0054–0058 all being closed before this ticket is started.
