---
id: "0069"
title: Add default_registry() and update all module tests
priority: high
epic: "E013"
created: 2026-03-04
---

## Summary

After all ten modules (T-0060 through T-0068) have been migrated, add a
`pub fn default_registry() -> Registry` convenience function to
`patches-modules/src/lib.rs` that registers every module type. This is the capstone
ticket for E013. Verify the full crate builds and all tests pass.

## Acceptance criteria

- [ ] `patches-modules/src/lib.rs` exports:
      ```rust
      pub fn default_registry() -> Registry { … }
      ```
      The function creates a new `Registry` (via `Registry { builders: HashMap::new() }`
      or a `Registry::new()` if one is added to `patches-core`) and calls `register::<T>()`
      for all ten module types:
      - `SineOscillator`
      - `Sum`
      - `Vca`
      - `AudioOut`
      - `SawtoothOscillator`
      - `SquareOscillator`
      - `AdsrEnvelope`
      - `ClockSequencer`
      - `StepSequencer`
      - `Glide`
- [ ] A test in `lib.rs` (or a dedicated integration test) verifies that
      `default_registry().create(name, …)` succeeds for every registered module name
      with a default `ParameterMap` and `ModuleShape { channels: 2 }`.
- [ ] `patches-modules` re-exports `Registry`, `ParameterMap`, `ParameterValue`,
      `AudioEnvironment`, and `ModuleShape` (or documents that callers must import them
      from `patches-core` directly) — choose whichever is consistent with the existing
      re-export policy.
- [ ] `cargo build`, `cargo test`, and `cargo clippy` clean across the whole workspace
      with no new warnings.
- [ ] E013 epic moved to `epics/closed/`.
- [ ] All T-0060 through T-0068 tickets confirmed closed.

## Notes

`default_registry()` is a convenience for tests and the DSL layer; it is not required
for production use (callers may build their own registry with a subset of modules).

If `patches-core` does not yet expose `Registry::new()`, construct it directly:

```rust
pub fn default_registry() -> patches_core::Registry {
    use std::collections::HashMap;
    let mut r = patches_core::Registry { builders: HashMap::new() };
    r.register::<SineOscillator>();
    // …
    r
}
```

Alternatively, propose adding `Registry::new()` to `patches-core` as a minor quality-of-life
change in this ticket if the team agrees.
