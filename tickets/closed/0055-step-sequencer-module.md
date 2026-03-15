---
id: "0055"
title: StepSequencer module
priority: high
epic: "E012"
created: 2026-03-03
---

## Summary

Add a `StepSequencer` module to `patches-modules`. The pattern is supplied at
construction time as a slice of step strings and parsed into a pre-computed vec of step
data before audio begins. At audio time the sequencer advances one step per rising edge
on the `clock` input, with `start`, `stop`, and `reset` inputs for playback control.

## Acceptance criteria

- [ ] `StepSequencer::new(pattern: &[&str]) -> Self` parses the pattern at construction; returns an error type (or panics in tests only) for unrecognised step strings.
- [ ] Four input ports: `clock/0`, `start/1`, `stop/2`, `reset/3`.
- [ ] Three output ports: `pitch/0`, `trigger/1`, `gate/2`.
- [ ] Pitch notes output V/OCT relative to C2 (`C2=0.0`, `C3=1.0`, etc.).
- [ ] `"-"` steps set gate=0, trigger=0; pitch holds previous value.
- [ ] `"_"` steps set gate=1, trigger=0; pitch holds the current tied note's value.
- [ ] Named pitch steps set gate=1, trigger=1 on the entry sample then trigger=0 thereafter.
- [ ] Rising-edge detection on all four inputs (threshold 0.5; stores previous sample values).
- [ ] Unit tests: correct pitch, trigger, and gate outputs for a short known sequence.
- [ ] `cargo clippy` and `cargo test -p patches-modules` clean.

## Notes

**Pattern parsing (construction time, allocation allowed)**

Intern each step string as one of:

```rust
enum Step {
    Note { voct: f32 },
    Rest,
    Tie,
}
```

Parse note names: letter (`C`…`B`), optional accidental (`#` / `b`), octave digit.
Semitone index: `C=0, C#=1, D=2, D#=3, E=4, F=5, F#=6, G=7, G#=8, A=9, A#=10, B=11`.

```
voct = (octave as f32 - 2.0) + semitone_index as f32 / 12.0
```

Store the parsed vec in `steps: Vec<Step>` on the struct. `process` indexes this vec
by step index — no string parsing at audio time.

**Audio-time logic**

On each `process` call:
1. Detect rising edges on inputs.
2. If `reset` fired: `step_index = 0`.
3. If `stop` fired: `playing = false`; set gate=0, trigger=0.
4. If `start` fired: `playing = true`.
5. If `clock` fired AND `playing`: advance `step_index = (step_index + 1) % steps.len()`, update outputs from new step.
6. Write `pitch`, `trigger`, `gate` outputs. `trigger` is 1.0 only on the clock-advance sample; resets to 0.0 immediately next sample automatically since `trigger_pending` is a bool cleared after output.

Sequencer starts in the running state (simplifies the demo; can be made configurable
later).
