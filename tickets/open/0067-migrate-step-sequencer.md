---
id: "0067"
title: Migrate StepSequencer to Module v2
priority: medium
epic: "E013"
created: 2026-03-04
---

## Summary

Rewrite `patches-modules/src/step_sequencer.rs` so that `StepSequencer` implements the
`Module` v2 contract. The step pattern is not a typed `ParameterMap` entry (it is
variable-length and structurally distinct from the scalar/enum parameter kinds). Instead,
`prepare()` initialises with an empty pattern, and a `ControlSignal::ParameterUpdate`
variant with `name: "steps"` and `value: ParameterValue::Array(...)` sets the pattern
post-construction.

## Acceptance criteria

- [ ] `StepSequencer` has no `new(pattern)` constructor (or retains it as
      `pub(crate)` / private if needed internally, but is not the primary API).
- [ ] `StepSequencer::describe(shape)` returns:
      - `module_name`: `"StepSequencer"`
      - 4 inputs: `"clock"` / 0, `"start"` / 1, `"stop"` / 2, `"reset"` / 3
      - 3 outputs: `"pitch"` / 0, `"trigger"` / 1, `"gate"` / 2
      - 0 parameters (pattern is not a ParameterMap entry)
- [ ] `prepare()` stores `audio_environment` and `descriptor`; initialises `steps` to an
      empty `Vec`, `step_index` to `0`, `playing` to `false`, all edge-detection fields
      to `0.0`.
- [ ] `update_validated_parameters()` is a no-op; returns `Ok(())`.
- [ ] `receive_signal` handles `ControlSignal::ParameterUpdate { name: "steps", value: ParameterValue::Array(step_strs) }`:
      - Parses each `&'static str` using the existing `parse_step` logic.
      - On any parse error, silently ignores the signal (or stores what parsed successfully
        up to the error — implementer's choice, but must not panic).
      - Resets `step_index` to `0` and `playing` to `false` after a successful pattern load.
- [ ] `process` behaviour unchanged.
- [ ] `as_any` implemented.
- [ ] Tests include:
      - Registry creation succeeds with an empty pattern (`describe` / `instance_id` checks).
      - Setting a pattern via `receive_signal` then stepping through it works correctly
        (existing sequencing logic tests, adapted).
      - Invalid step strings in the signal payload are handled without panic.
- [ ] `cargo clippy -p patches-modules` and `cargo test -p patches-modules` clean.

## Notes

`ParameterValue::Array(Vec<&'static str>)` requires the step strings to be `'static`.
In the DSL use case they will always be string literals. Test code can use static slices:

```rust
let steps: Vec<&'static str> = vec!["C3", "Eb3", "F3", "G3"];
m.receive_signal(ControlSignal::ParameterUpdate {
    name: "steps",
    value: ParameterValue::Array(steps),
});
```

The existing `parse_step` helper function can remain private; it is called inside
`receive_signal`. `ParseStepError` may be kept or removed from the public API since
patterns are now set via signal, not constructor.

If keeping step-parsing tests that were previously tied to the `new()` constructor, they
should be rewritten to construct via registry, load a pattern via `receive_signal`, then
exercise `process`.
