---
id: "0060"
title: Migrate SineOscillator to Module v2
priority: high
epic: "E013"
created: 2026-03-04
---

## Summary

Rewrite `patches-modules/src/oscillator.rs` so that `SineOscillator` implements the
current `patches-core` `Module` v2 contract: `describe()`, `prepare()`, and
`update_validated_parameters()`. Remove the `new()` constructor and the old `initialise()`
method. Update tests to use a local `Registry`.

## Acceptance criteria

- [ ] `SineOscillator` no longer has a `new()` constructor or `initialise()` method.
- [ ] `SineOscillator::describe(&ModuleShape { channels: 0 })` returns a `ModuleDescriptor`
      with:
      - `module_name`: `"SineOscillator"`
      - 0 inputs
      - 1 output named `"out"` / index 0
      - 1 parameter: `"frequency"`, `Float { min: 0.01, max: 20_000.0, default: 440.0 }`
- [ ] `prepare()` stores `audio_environment` and `descriptor`; derives
      `sample_rate_reciprocal` from `audio_environment.sample_rate`; initialises `frequency`
      to `0.0` and `phase` to `0.0` (parameters applied later by `update_validated_parameters`).
- [ ] `update_validated_parameters()` reads `"frequency"` from `params` and recomputes
      `phase_increment = TAU * frequency * sample_rate_reciprocal`.
- [ ] `receive_signal` matches `ControlSignal::ParameterUpdate { name: "frequency", value: ParameterValue::Float(v) }` and updates frequency (no longer matches `ControlSignal::Float`).
- [ ] `process` behaviour is unchanged: `outputs[0] = phase.sin()`, phase wraps in `[0, 2π)`.
- [ ] `as_any` implemented.
- [ ] All existing tests pass, rewritten to use a local `Registry`:
      - `descriptor_has_no_inputs_and_one_output`
      - `instance_ids_are_distinct`
      - `receive_signal_freq_updates_frequency`
      - `receive_signal_unknown_name_is_ignored`
      - `output_completes_full_cycle_in_period_samples`
- [ ] `cargo clippy -p patches-modules` and `cargo test -p patches-modules` clean.

## Notes

`SineOscillator` has no audio-rate inputs; frequency is controlled entirely by the
`"frequency"` parameter and `receive_signal`. The v2 contract does not change the audio
behaviour — only how the instance is constructed and parameters applied.

Helper in test module:

```rust
fn make_module(params: &ParameterMap) -> Box<dyn Module> {
    let mut r = Registry { builders: HashMap::new() };
    r.register::<SineOscillator>();
    r.create(
        "SineOscillator",
        &AudioEnvironment { sample_rate: 44100.0 },
        &ModuleShape { channels: 0 },
        params,
    ).unwrap()
}
```
