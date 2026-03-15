---
id: "0090"
title: "`Lfo` core module — six outputs, rate, phase offset, polarity mode"
priority: high
created: 2026-03-06
epic: "E017"
---

## Summary

Introduce `patches-modules/src/lfo.rs` with a new `Lfo` module covering the
core LFO feature set: six waveform outputs, a rate parameter, a phase offset
parameter, and a polarity mode. Sync and rate CV inputs are added in T-0091.

## Acceptance criteria

- [ ] New file `patches-modules/src/lfo.rs`; `Lfo` struct is registered in the
      default registry in `lib.rs`.
- [ ] Module descriptor:
      - name: `"Lfo"`
      - inputs: *(none in this ticket; added in T-0091)*
      - outputs: `sine` (0), `triangle` (1), `saw_up` (2), `saw_down` (3),
        `square` (4), `random` (5)
      - parameters:
        - `rate`: Float, min 0.01, max 20.0, default 1.0 (value is Hz directly)
        - `phase_offset`: Float, min 0.0, max 1.0, default 0.0
        - `mode`: Enum, variants `["bipolar", "unipolar_positive",
          "unipolar_negative"]`, default `"bipolar"`
- [ ] Phase accumulator is a simple `phase: f32` + `phase_increment: f32`
      maintained in the struct (no `UnitPhaseAccumulator` — the LFO has no
      V/OCT or FM and does not benefit from that abstraction).
      `phase_increment = rate / sample_rate`, recomputed on each
      `update_validated_parameters` call and stored.
- [ ] `process` advances `phase += phase_increment; phase = phase.fract();`
      then computes each connected output from `read_phase = (phase + phase_offset).fract()`.
- [ ] Waveform formulae (before polarity mapping):
      - **sine**: `lookup_sine(read_phase)` — reuse existing lookup
      - **triangle**: `1.0 - 4.0 * (read_phase - 0.5).abs()`
      - **saw_up**: `2.0 * read_phase - 1.0`
      - **saw_down**: `1.0 - 2.0 * read_phase`
      - **square**: `if read_phase < 0.5 { 1.0 } else { -1.0 }` (fixed 50%
        duty; no pulse-width input on LFO)
      - **random**: holds a value drawn from the module's private PRNG,
        refreshed whenever `phase` wraps (i.e. `new_phase < old_phase` after
        advance). See PRNG note below.
- [ ] Polarity mapping applied to the raw value `v ∈ [-1.0, 1.0]` before
      writing to the output buffer:
      - `bipolar`: output = `v`
      - `unipolar_positive`: output = `0.5 + 0.5 * v` → range `[0.0, 1.0]`
      - `unipolar_negative`: output = `-(0.5 + 0.5 * v)` → range `[-1.0, 0.0]`
- [ ] `set_connectivity` records which outputs are connected; only connected
      outputs are computed.
- [ ] `sample_rate` is stored during `prepare` for use in `phase_increment`
      computation.
- [ ] Tests:
      - descriptor has 0 inputs and 6 outputs with correct names
      - instance IDs are distinct between two `Lfo` instances
      - sine output completes a consistent full cycle across two periods
      - `phase_offset = 0.25` shifts sine output by a quarter cycle
      - `mode = unipolar_positive` maps saw_up to `[0.0, 1.0]`
      - random output holds each value for exactly one period (one new value
        per wrap) and stays within `[0.0, 1.0]` in unipolar_positive mode
      - disconnected outputs are not written (output buffer slot unchanged)
- [ ] `cargo build`, `cargo test`, `cargo clippy` pass with no new warnings.

## Notes

**PRNG.** Do not add the `rand` crate. Use a simple xorshift64 PRNG seeded from
the `InstanceId` counter value. A minimal implementation in the struct:

```rust
// state must never be 0; seed from instance id (which is always > 0)
fn xorshift64(state: &mut u64) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    // map to [-1.0, 1.0]
    (*state as i64 as f32) / (i64::MAX as f32)
}
```

Store `prng_state: u64` in the struct, initialised in `prepare` to the raw
`u64` value of the `InstanceId` (guaranteed non-zero by the atomic counter).

**No `UnitPhaseAccumulator`.** The LFO's phase logic is simple enough to own
directly. Pulling in `UnitPhaseAccumulator` would require a dummy
`reference_frequency` and would expose FM machinery that has no meaning here.
