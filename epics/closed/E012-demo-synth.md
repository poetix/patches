---
id: "E012"
title: Demo synth
created: 2026-03-03
tickets: ["0054", "0055", "0056", "0057", "0058", "0059"]
---

## Summary

Build a set of practical music modules — clock sequencer, step sequencer, ADSR envelope,
sawtooth and square oscillators, and a VCA — then wire them into a runnable example that
plays a repeating 16-step melodic sequence at 120 BPM. The result is the first
"recognisably musical" demo of the Patches system and proves that the existing module
graph, buffer pool, and audio engine are sufficient for sequenced synthesis without any
architectural changes.

## Tickets

| ID   | Title                                   | Priority |
|------|-----------------------------------------|----------|
| 0054 | ClockSequencer module                   | high     |
| 0055 | StepSequencer module                    | high     |
| 0056 | ADSR envelope module                    | high     |
| 0057 | Sawtooth and square oscillators         | medium   |
| 0058 | VCA module                              | medium   |
| 0059 | Demo synth example                      | high     |

## Definition of done

- All six tickets closed.
- `cargo build`, `cargo test`, `cargo clippy` all clean with no new warnings.
- `cargo run --example demo_synth` compiles and produces audible, pitched output for at
  least two full bar cycles before exiting cleanly.
- No new `unwrap()` or `expect()` in library code.
- No allocations introduced in any `process()` implementation.

---

## Module designs

### T-0054 — ClockSequencer

Generates synchronised subdivisions of a musical tempo so that other modules can be
driven at bar, beat, quaver, and semiquaver rates without needing to reason about BPM
themselves.

**Port layout**

| Direction | Name         | Index | Description                              |
|-----------|--------------|-------|------------------------------------------|
| output    | `bar`        | 0     | Fires at the start of each bar           |
| output    | `beat`       | 1     | Fires on every beat                      |
| output    | `quaver`     | 2     | Fires on every quaver (beat / quavers_per_beat) |
| output    | `semiquaver` | 3     | Fires on every semiquaver (quaver / 2)   |

All four outputs are normally `0.0`. On the sample where a subdivision boundary is
crossed, the output is `1.0` for exactly that one sample.

**Constructor**

```rust
ClockSequencer::new(bpm: f32, beats_per_bar: u32, quavers_per_beat: u32) -> Self
```

- `bpm` — tempo; "beat" means the denominator unit of the time signature.
- `beats_per_bar` — numerator of the time signature (e.g. 4 for 4/4, 2 for 6/8).
- `quavers_per_beat` — how many quavers divide each beat: **2** for simple time (2/4,
  3/4, 4/4), **3** for compound time (6/8, 9/8, 12/8).

The semiquaver is always half a quaver, so the full hierarchy is:

```
bar           = beats_per_bar × beat_period
beat          = 60.0 / bpm  seconds
quaver        = beat / quavers_per_beat
semiquaver    = quaver / 2
```

Examples:

| Signature | bpm | beats_per_bar | quavers_per_beat | semiquavers/bar |
|-----------|-----|---------------|------------------|-----------------|
| 4/4 ♩=120 | 120 | 4             | 2                | 32              |
| 3/4 ♩=90  |  90 | 3             | 2                | 24              |
| 6/8 ♩.=60 |  60 | 2             | 3                | 24              |
| 12/8 ♩.=80|  80 | 4             | 3                | 48              |

**Control signals** (via `receive_signal`)

| Name               | Type  | Effect                                            |
|--------------------|-------|---------------------------------------------------|
| `"bpm"`            | Float | Updates tempo immediately                         |
| `"beats_per_bar"`  | Float | Updates bar length (cast to u32)                  |
| `"quavers_per_beat"` | Float | Updates subdivision structure (cast to u32)     |

**Internal design**

A single beat-phase accumulator (`beat_phase: f32 ∈ [0.0, 1.0)`) advances by
`bpm / (60.0 * sample_rate)` each sample. All four outputs are derived from this one
accumulator, so they stay perfectly phase-locked:

- **Beat**: fires when `beat_phase` wraps past `1.0` (i.e. `beat_phase >= 1.0` after
  incrementing, then `beat_phase -= 1.0`). A `beat_count: u32` is incremented on each
  beat.
- **Bar**: fires simultaneously with the beat when `beat_count % beats_per_bar == 0`.
- **Quaver**: fires when `floor(beat_phase_new * quavers_per_beat)` exceeds
  `floor(beat_phase_old * quavers_per_beat)`, or when the beat fires (since the beat is
  also a quaver boundary).
- **Semiquaver**: same crossing test using `quavers_per_beat * 2` as the divisor.

The crossing test compares integer bucket indices before and after the increment; no
secondary counters are needed and the outputs remain sample-accurate regardless of
`quavers_per_beat`.

---

### T-0055 — StepSequencer

A variable-length sequencer with V/OCT pitch, trigger, and gate outputs. Driven by an
external clock pulse and start/stop/reset trigger inputs.

**Port layout**

| Direction | Name      | Index | Description                                              |
|-----------|-----------|-------|----------------------------------------------------------|
| input     | `clock`   | 0     | Rising edge advances to the next step                    |
| input     | `start`   | 1     | Rising edge begins playback from the current step        |
| input     | `stop`    | 2     | Rising edge halts playback; outputs go to rest values    |
| input     | `reset`   | 3     | Rising edge returns to step 0 (play state unchanged)     |
| output    | `pitch`   | 0     | V/OCT pitch of the current step                          |
| output    | `trigger` | 1     | `1.0` for the one sample when a new note starts          |
| output    | `gate`    | 2     | `1.0` while a note is sustaining, `0.0` during rests     |

**Constructor**

```rust
StepSequencer::new(pattern: &[&str]) -> Self
```

`pattern` is a slice of step descriptors (see notation below). The sequencer loops
indefinitely. Starts in the **stopped** state; the `start` input must fire (or the
sequencer can be constructed in the running state for simple demos — the ticket may
choose).

**Step notation**

| Symbol   | Meaning                                                              |
|----------|----------------------------------------------------------------------|
| `"C3"`   | Named pitch; gate=1, trigger=1 for this sample on step entry         |
| `"-"`    | Rest; gate=0, trigger=0, pitch holds previous value                  |
| `"_"`    | Tie/continue; gate=1, trigger=0, pitch holds current note's value    |

**Pitch encoding — V/OCT relative to C2**

```
voct = (octave - 2) + semitone_index / 12.0
```

| Note | Semitone index |
|------|---------------|
| C    | 0  |
| C#/Db| 1 |
| D    | 2  |
| D#/Eb| 3 |
| E    | 4  |
| F    | 5  |
| F#/Gb| 6 |
| G    | 7  |
| G#/Ab| 8 |
| A    | 9  |
| A#/Bb| 10|
| B    | 11 |

Examples: `C2 → 0.0`, `A2 → 0.75`, `C3 → 1.0`, `G3 → 1.583…`, `C4 → 2.0`.

Pattern parsing happens at construction time; the `process` path operates only on
pre-computed `f32` pitch values and enum step types — no string work at audio time.

**Edge detection**

All four trigger inputs use rising-edge detection: fire on the sample where the value
crosses above 0.5 from at or below 0.5. Each input stores its previous sample value.

---

### T-0056 — ADSR envelope

A standard four-stage envelope generator. Output is in `[0.0, 1.0]` at all times.

**Port layout**

| Direction | Name      | Index | Description                                               |
|-----------|-----------|-------|-----------------------------------------------------------|
| input     | `trigger` | 0     | Rising edge restarts the envelope from the Attack stage   |
| input     | `gate`    | 1     | Holds the envelope in Sustain while `≥ 0.5`               |
| output    | `out`     | 0     | Envelope amplitude `[0.0, 1.0]`                           |

**Constructor**

```rust
AdsrEnvelope::new(attack_secs: f32, decay_secs: f32, sustain: f32, release_secs: f32) -> Self
```

- `sustain` is a level in `[0.0, 1.0]`, not a time.

**Stage logic (per sample)**

```
Idle    → level holds at 0.0
Attack  → level rises linearly from current level to 1.0 over attack_secs;
           transitions to Decay when level reaches 1.0
Decay   → level falls linearly from 1.0 to sustain over decay_secs;
           transitions to Sustain
Sustain → level holds at sustain while gate ≥ 0.5;
           transitions to Release when gate < 0.5
Release → level falls linearly from current level to 0.0 over release_secs;
           transitions to Idle when level reaches 0.0
```

A rising edge on `trigger` transitions to Attack from whatever state and level the
envelope is currently in (natural retrigger with no pop suppression for now).

---

### T-0057 — Sawtooth and square oscillators

Two waveform generators sharing the same V/OCT pitch interface. Both live in
`patches-modules/src/waveforms.rs` and are exported from `patches-modules`.

**Port layout (both)**

| Direction | Name   | Index | Description                                        |
|-----------|--------|-------|----------------------------------------------------|
| input     | `voct` | 0     | V/OCT offset added to the constructor base pitch   |
| output    | `out`  | 0     | Audio signal in `[-1.0, 1.0]`                      |

**Constructors**

```rust
SawtoothOscillator::new(base_voct: f32) -> Self
SquareOscillator::new(base_voct: f32) -> Self
```

`base_voct` is the pitch when the `voct` input is `0.0`. Total pitch =
`base_voct + voct_input`. Frequency:

```
C2_FREQ: f32 = 65.406_194;
freq = C2_FREQ * 2_f32.powf(base_voct + voct_input)
```

The `voct` input is sampled every audio-rate call so V/OCT modulation works at full
sample resolution. In the demo the step sequencer updates pitch once per semiquaver
clock; the oscillator simply tracks it.

**Waveforms**

Phase `φ ∈ [0.0, 1.0)`, incremented by `freq / sample_rate` each sample:

- **Sawtooth**: `output = 2.0 * φ - 1.0` (rises linearly from −1 to +1 per cycle)
- **Square**: `output = if φ < 0.5 { 1.0 } else { -1.0 }`

---

### T-0058 — VCA

A voltage-controlled amplifier: multiplies an audio signal by a control voltage.
Stateless; no `initialise` override needed.

**Port layout**

| Direction | Name  | Index | Description                               |
|-----------|-------|-------|-------------------------------------------|
| input     | `in`  | 0     | Audio signal (any range)                  |
| input     | `cv`  | 1     | Control voltage, typically `[0.0, 1.0]`   |
| output    | `out` | 0     | `in * cv`                                 |

**Constructor**

```rust
Vca::new() -> Self
```

No clamping. A `cv` above `1.0` amplifies; a negative `cv` inverts.

---

### T-0059 — Demo synth example

File: `patches-engine/examples/demo_synth.rs`

Wires the new modules into a complete synthesiser that plays a 16-step melodic sequence
for several bars then exits.

**Patch graph**

```
ClockSequencer
  └─ semiquaver ──────────────────────────────→ StepSequencer.clock

StepSequencer
  ├─ pitch   ──────────────────────────────────→ SawtoothOscillator.voct
  ├─ pitch   ──────────────────────────────────→ SquareOscillator.voct
  ├─ trigger ──────────────────────────────────→ AdsrEnvelope.trigger
  └─ gate    ──────────────────────────────────→ AdsrEnvelope.gate

SawtoothOscillator.out ──→ Sum(2).in/0
SquareOscillator.out   ──→ Sum(2).in/1

Sum.out          ──→ Vca.in
AdsrEnvelope.out ──→ Vca.cv

Vca.out ──→ AudioOut.left
Vca.out ──→ AudioOut.right
```

The `start` input of the StepSequencer is not connected in the demo; the sequencer is
constructed in the running state so that playback begins as soon as the first clock tick
arrives.

**Sequence (16 steps)**

```rust
["C3", "Eb3", "F3", "G3", "_",   "Bb3", "_",  "C4",
 "-",  "G3",  "F3", "Eb3", "_",  "C3",  "_",  "-" ]
```

A minor-pentatonic phrase that loops every bar.

**Settings**

| Parameter       | Value             |
|-----------------|-------------------|
| BPM             | 120               |
| beats_per_bar   | 4                 |
| quavers_per_beat| 2 (simple 4/4)    |
| Clock source    | `semiquaver` output|
| Attack          | 5 ms              |
| Decay           | 50 ms             |
| Sustain level   | 0.6               |
| Release         | 100 ms            |
| Mix scale       | 0.5 per oscillator (Sum inputs) |
| Run time        | ~8 seconds (4 bars at 120 BPM) |

The example follows the `run() -> Result<…>` + `main()` error-handling pattern from the
existing examples.

---

## Notes

**No new crates.** All modules are added to `patches-modules`. No new `Cargo.toml`
dependencies are introduced; all required maths is in `std`.

**No structural engine changes.** The existing `build_patch` / `SoundEngine` / `AudioOut`
API is sufficient. This epic validates that the engine architecture handles real musical
workloads without modification.

**Trigger signals on the audio bus.** Triggers (0.0/1.0 one-sample pulses) and gate
signals (sustained 0.0/1.0) flow through the same `f32` audio buffers as pitched audio.
This avoids a separate control-rate bus at this stage. All receiving modules detect
triggers via rising-edge crossing of the 0.5 threshold.

**V/OCT convention.** All pitch-related modules use V/OCT relative to C2
(0.0 = C2 = 65.406 Hz, 1.0 = C3). Conversion: `freq = 65.406194 × 2^voct`. A direct
connection from `StepSequencer.pitch` to `SawtoothOscillator.voct` with scale `1.0` is
therefore correct and sufficient.

**Clock phase-lock.** Because all four ClockSequencer outputs derive from one
beat-phase accumulator, bar/beat/quaver/semiquaver fire on the same sample as the
corresponding beat boundary — they are never offset by a sample relative to each other.

**Future extension points.** The `start` and `stop` inputs of the StepSequencer are
implemented but unused in the demo. Pattern contents are fixed at construction time; a
subsequent epic could expose `receive_signal` variants for live pattern mutation.
