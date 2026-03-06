---
id: "0087"
title: Update example patches to use `Oscillator`
priority: medium
created: 2026-03-06
epic: "E016"
depends_on: ["0086"]
---

## Summary

After `SineOscillator`, `SawtoothOscillator`, and `SquareOscillator` are
removed in T-0085 and T-0086, the example YAML patches and any Rust code that
references those module names must be updated to use `Oscillator`.

## Acceptance criteria

- [ ] `demo_synth.yaml`:
      - `lfo` and `lfo2` nodes: `module: SineOscillator` → `module: Oscillator`;
        cables connecting to their `out` output updated to use `sine` output name.
      - `saw` node: `module: SawtoothOscillator` → `module: Oscillator`; cables
        connecting to `saw.out` updated to `saw.sawtooth`.
      - `sq` node: `module: SquareOscillator` → `module: Oscillator`; cables
        connecting to `sq.out` updated to `sq.square`.
      - `sub` node: `module: SineOscillator` → `module: Oscillator`; cables
        updated to `sub.sine`.
      - The `base_voct` parameter on `saw` and `sq` nodes is replaced with the
        equivalent `frequency` offset if needed, or removed if the V/OCT input
        achieves the same result. Document the chosen approach in a comment.
- [ ] `mutual_fm.yaml`:
      - `sine1` and `sine2` nodes: `module: SineOscillator` → `module: Oscillator`;
        cables connecting to their `out` output updated to `sine`.
      - `fm_type: logarithmic` parameter retained (it is present on `Oscillator`).
- [ ] Any Rust source files (examples, integration tests) that reference the old
      module names by string or type are updated.
- [ ] Both YAML files load and play correctly when run with `patches-player`.
- [ ] Signal-flow comments at the top of each YAML file are updated to reflect
      the new module names and output port names.
- [ ] `cargo build`, `cargo test` pass with no new warnings.

## Notes

`SawtoothOscillator` uses a `base_voct` parameter (V/OCT offset as a float)
while `Oscillator` uses `frequency` (Hz offset from C0 reference). The `saw`
and `sq` nodes in `demo_synth.yaml` use V/OCT inputs from `glide`, so the
`base_voct` parameter can be dropped entirely from those nodes (default
`frequency: 0.0` means C0 + voct input controls pitch). Verify the resulting
pitch is correct in context.

The `sq` node currently has `base_voct: 1.583` to transpose it up an octave.
With `Oscillator`, this transposition should be achieved by setting
`frequency: <appropriate Hz offset>` or by adjusting the voct input scale.
Document the chosen approach.
