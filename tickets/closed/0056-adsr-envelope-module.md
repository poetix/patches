---
id: "0056"
title: ADSR envelope module
priority: high
epic: "E012"
created: 2026-03-03
---

## Summary

Add an `AdsrEnvelope` module to `patches-modules`. The envelope progresses through
Attack → Decay → Sustain → Release stages driven by trigger and gate inputs. Output is
always in `[0.0, 1.0]`. All stage times are converted to per-sample linear increments
in `initialise`; `process` contains only additions and comparisons.

## Acceptance criteria

- [ ] `AdsrEnvelope::new(attack_secs: f32, decay_secs: f32, sustain: f32, release_secs: f32)` compiles.
- [ ] Two input ports: `trigger/0`, `gate/1`.
- [ ] One output port: `out/0`.
- [ ] Rising edge on `trigger` transitions to Attack from any state and current level.
- [ ] Attack rises linearly to 1.0; Decay falls linearly to `sustain`; Sustain holds at `sustain` while gate ≥ 0.5; Release falls linearly from current level to 0.0.
- [ ] Output is always in `[0.0, 1.0]` (clamp if floating-point drift pushes it outside).
- [ ] `initialise` recomputes per-sample increments from stage durations and `env.sample_rate`.
- [ ] Unit tests: output at sample boundaries for known attack/decay/sustain/release values.
- [ ] `cargo clippy` and `cargo test -p patches-modules` clean.

## Notes

**State enum**

```rust
enum Stage { Idle, Attack, Decay, Sustain, Release }
```

**Per-sample increments (computed in `initialise`)**

```rust
attack_inc  = 1.0 / (attack_secs  * sample_rate);
decay_inc   = (1.0 - sustain) / (decay_secs   * sample_rate);
release_inc = sustain / (release_secs * sample_rate);  // or current_level / ...
```

Release uses the *current level* at the moment release begins divided by the release
time, so the slope is constant regardless of where release started. Recalculate on
entry to Release stage.

**Trigger edge detection**: same rising-edge logic as StepSequencer (threshold 0.5,
store previous trigger sample).

**Gate**: read raw each sample — no edge detection needed; Sustain holds while
`inputs[1] >= 0.5`.
