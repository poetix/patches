---
id: "E013"
title: Migrate patches-modules to Module v2 contract
created: 2026-03-04
tickets: ["0060", "0061", "0062", "0063", "0064", "0065", "0066", "0067", "0068", "0069"]
---

## Summary

The modules in `patches-modules` were written against an earlier version of the `Module`
trait (`initialise`, ad-hoc `new()` constructors, `ControlSignal::Float`). After E003
overhauled `patches-core` with a proper two-phase construction protocol (`describe` /
`prepare` / `update_validated_parameters`), a typed `ParameterMap`, a `Registry`, and
the `ControlSignal::ParameterUpdate` variant, the module implementations were not updated.

This epic brings all ten modules into full conformance with the current `patches-core`
API and wires them into a `default_registry()` function exported from `patches-modules`.

## Tickets

| ID   | Title                                               | Priority |
|------|-----------------------------------------------------|----------|
| 0060 | Migrate SineOscillator to Module v2                 | high     |
| 0061 | Migrate Sum to Module v2                            | high     |
| 0062 | Migrate Vca to Module v2                            | high     |
| 0063 | Migrate AudioOut to Module v2                       | high     |
| 0064 | Migrate SawtoothOscillator and SquareOscillator to Module v2 | high |
| 0065 | Migrate AdsrEnvelope to Module v2                   | high     |
| 0066 | Migrate ClockSequencer to Module v2                 | high     |
| 0067 | Migrate StepSequencer to Module v2                  | medium   |
| 0068 | Migrate Glide to Module v2                          | high     |
| 0069 | Add default_registry() and update all module tests  | high     |

## Definition of done

- Every module in `patches-modules` implements `describe()`, `prepare()`, and
  `update_validated_parameters()` and no longer uses `new()` constructors or `initialise()`.
- `ControlSignal::Float` references replaced with `ControlSignal::ParameterUpdate`.
- `patches-modules` exports `pub fn default_registry() -> Registry` that registers all
  ten module types.
- All tests instantiate modules via `registry.create(...)` (using a local registry or
  `default_registry()`), not via direct constructors.
- `cargo build`, `cargo test`, `cargo clippy` clean with no new warnings.
- No `unwrap()` or `expect()` in library code.

---

## Module contract reference

The v2 `Module` trait requires:

```rust
fn describe(shape: &ModuleShape) -> ModuleDescriptor;
fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor) -> Self;
fn update_validated_parameters(&mut self, params: &ParameterMap) -> Result<(), BuildError>;
fn descriptor(&self) -> &ModuleDescriptor;
fn instance_id(&self) -> InstanceId;
fn process(&mut self, inputs: &[f32], outputs: &mut [f32]);
fn as_any(&self) -> &dyn std::any::Any;
// defaults: receive_signal (no-op), as_sink (None), update_parameters (validates then delegates)
```

`prepare()` stores `audio_environment` and `descriptor`; all other fields are zero/default.
Sample rate must be extracted from `audio_environment` in `prepare()` (not `initialise()`).

### Parameter design per module

| Module                 | Parameters (name → kind)                                                       |
|------------------------|--------------------------------------------------------------------------------|
| SineOscillator         | `frequency`: Float { min: 0.01, max: 20_000.0, default: 440.0 }               |
| Sum                    | none; input count = `shape.channels`                                           |
| Vca                    | none                                                                           |
| AudioOut               | none                                                                           |
| SawtoothOscillator     | `base_voct`: Float { min: -4.0, max: 8.0, default: 0.0 }                      |
| SquareOscillator       | `base_voct`: Float { min: -4.0, max: 8.0, default: 0.0 }                      |
| AdsrEnvelope           | `attack`: Float { min: 0.001, max: 10.0, default: 0.01 }                      |
|                        | `decay`: Float { min: 0.001, max: 10.0, default: 0.1 }                        |
|                        | `sustain`: Float { min: 0.0, max: 1.0, default: 0.7 }                         |
|                        | `release`: Float { min: 0.001, max: 10.0, default: 0.3 }                      |
| ClockSequencer         | `bpm`: Float { min: 1.0, max: 300.0, default: 120.0 }                         |
|                        | `beats_per_bar`: Int { min: 1, max: 16, default: 4 }                          |
|                        | `quavers_per_beat`: Int { min: 1, max: 4, default: 2 }                        |
| StepSequencer          | none via ParameterMap; pattern is set post-construction (see T-0067)           |
| Glide                  | `glide_ms`: Float { min: 0.0, max: 10_000.0, default: 100.0 }                 |

### receive_signal migration

Modules that currently pattern-match `ControlSignal::Float { name, value }` must be
updated to match `ControlSignal::ParameterUpdate { name, value: ParameterValue::Float(v) }`.

### Test pattern (per module)

```rust
fn make_registry() -> Registry {
    let mut r = Registry { builders: HashMap::new() };
    r.register::<T>();
    r
}

fn make_module() -> Box<dyn Module> {
    let r = make_registry();
    r.create("ModuleName", &AudioEnvironment { sample_rate: 44100.0 },
             &ModuleShape { channels: N }, &ParameterMap::new()).unwrap()
}
```

Tests that verify parameter behaviour should pass explicit `ParameterMap` entries rather
than relying on constructor arguments.

---

## Notes

**`Sum` is shape-sensitive.** Its input count is `shape.channels`; callers must pass the
desired channel count in `ModuleShape`. `describe()` uses `shape.channels` to build the
input list. This is the intended use of `ModuleShape`.

**`StepSequencer` pattern.** The step pattern is not expressible as a simple typed
parameter (it is a variable-length sequence of note names). `prepare()` initialises the
sequencer with an empty pattern; T-0067 defines how patterns are provided post-construction
(via a dedicated `receive_signal` variant). Registry-based tests only need to verify that
creation succeeds; full sequencing tests may retain direct construction temporarily.

**No new Cargo dependencies.** All required types are already in `patches-core`.

**`patches-engine` is out of scope.** The demo synth example and engine glue are not
changed by this epic; that is left to a follow-on epic. After E013, `patches-engine`
may temporarily reference both old-style direct construction (for examples) and the new
registry API; those will be unified separately.
