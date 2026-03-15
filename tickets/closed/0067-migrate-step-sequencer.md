---
id: "0067"
title: Migrate StepSequencer to Module v2
priority: medium
epic: "E013"
created: 2026-03-04
---

## Summary

Rewrite `patches-modules/src/step_sequencer.rs` so that `StepSequencer` implements the
`Module` v2 contract, with the step pattern supplied as a `ParameterMap` entry. Because
no `ParameterKind` variant currently covers variable-length string arrays, this ticket
first extends `patches-core` to add one, then uses it in the sequencer.

## Prerequisites — changes to `patches-core`

### 1. Add `ParameterKind::Array` — `patches-core/src/module_descriptor.rs`

```rust
pub enum ParameterKind {
    Float { min: f32, max: f32, default: f32 },
    Int   { min: i64, max: i64, default: i64 },
    Bool  { default: bool },
    Enum  { variants: &'static [&'static str], default: &'static str },
    Array { default: &'static [&'static str] },   // ← add
}
```

- `default_value()` returns `ParameterValue::Array(default.iter().map(|s| s.to_string()).collect())`.
- `kind_name()` returns `"array"`.

The `default` field uses `&'static [&'static str]` so that the *descriptor* itself never
allocates (consistent with ADR 0011's zero-cost descriptor requirement). The
`ParameterValue` it produces *does* allocate, but only at the non-realtime boundary when
defaults are filled in by `Module::build`.

### 2. Change `ParameterValue::Array` to own its strings — `patches-core/src/parameter_map.rs`

```rust
// before
Array(Vec<&'static str>),

// after
Array(Vec<String>),
```

`Enum(&'static str)` can stay static because enum variants are always a closed set
declared in the descriptor. Array contents are data-driven (patterns come from a DSL
file or test code) so they cannot be required to be `'static`. This is a deliberate,
narrow exception to the ADR 0011 "all `ParameterValue` variants are static" convention;
worth noting in a comment at the declaration site.

### 3. Add the `Array` match arm to `validate_parameters` — `patches-core/src/module.rs`

In the `match (&param_desc.parameter_type, value)` block:

```rust
(ParameterKind::Array { .. }, ParameterValue::Array(_)) => {}
```

No bounds checking is needed for arrays; any `Vec<String>` is valid for an `Array`
parameter. The sequencer itself validates content in `update_validated_parameters`.

These three changes are self-contained within `patches-core` and have no impact on any
existing module (no current module declares an `Array` parameter or supplies
`ParameterValue::Array` in a `ParameterMap`).

---

## StepSequencer changes — `patches-modules/src/step_sequencer.rs`

- [ ] `StepSequencer` has no `new(pattern)` constructor.
- [ ] `StepSequencer::describe(shape)` returns:
      - `module_name`: `"StepSequencer"`
      - 4 inputs: `"clock"` / 0, `"start"` / 1, `"stop"` / 2, `"reset"` / 3
      - 3 outputs: `"pitch"` / 0, `"trigger"` / 1, `"gate"` / 2
      - 1 parameter: `"steps"`, `ParameterKind::Array { default: &[] }`
- [ ] `prepare()` stores `audio_environment` and `descriptor`; initialises `steps` to
      an empty `Vec`, `step_index` to `0`, `playing` to `false`, and all edge-detection
      fields to `0.0`.
- [ ] `update_validated_parameters()` reads `"steps"` from `params`:
      - Parses each `String` using the existing `parse_step` logic.
      - Returns `BuildError::Custom { module: "StepSequencer", message: … }` on any
        parse failure.
      - On success, stores the parsed steps and resets `step_index` to `0`.
      - An empty array (`&[]` default) produces an empty sequence; `process` must not
        panic when `steps` is empty (outputs hold at rest values).
- [ ] `process` behaviour otherwise unchanged.
- [ ] `as_any` implemented.
- [ ] `ParseStepError` may be kept or made private; it is no longer part of the public
      construction API.

## Acceptance criteria

- [ ] All three `patches-core` changes compile with no new warnings.
- [ ] `validate_parameters` unit tests in `patches-core` pass; add a test that a
      `ParameterValue::Array(vec!["C3".into()])` passes validation for an `Array`
      parameter, and that a `ParameterValue::Float(1.0)` against an `Array` descriptor
      returns `InvalidParameterType`.
- [ ] `StepSequencer` can be created via a local `Registry` with an empty parameter map
      (default empty pattern); `process` produces rest-value outputs without panicking.
- [ ] `StepSequencer` can be created via `Registry` with `"steps"` set to a valid
      pattern and sequences correctly through it.
- [ ] Invalid step strings in the pattern return a `BuildError` from `Registry::create`.
- [ ] All existing sequencing logic tests pass, rewritten to use the registry pattern:
      ```rust
      fn make_sequencer(steps: &[&str]) -> Box<dyn Module> {
          let mut params = ParameterMap::new();
          params.insert(
              "steps".into(),
              ParameterValue::Array(steps.iter().map(|s| s.to_string()).collect()),
          );
          let mut r = Registry { builders: HashMap::new() };
          r.register::<StepSequencer>();
          r.create("StepSequencer", &AudioEnvironment { sample_rate: 44100.0 },
                   &ModuleShape { channels: 0 }, &params).unwrap()
      }
      ```
- [ ] `cargo clippy` and `cargo test` clean across the whole workspace.

## Notes

The `patches-core` changes in this ticket should be committed before or alongside the
`patches-modules` changes so that the workspace never fails to build mid-ticket.

The `Enum` variant keeps `&'static str` because its values are always a closed set
declared at compile time in the descriptor. `Array` diverges because step patterns are
data — their content is not known until the DSL is parsed.
