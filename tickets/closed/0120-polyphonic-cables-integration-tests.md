---
id: "0120"
title: "Integration tests for polyphonic cables"
priority: medium
epic: "E022"
depends_on: ["0116", "0118", "0119"]
created: 2026-03-11
---

## Summary

Add integration tests in `patches-integration-tests` that verify the end-to-end
polyphonic cable behaviour: port delivery at plan-accept, poly cable data
propagation, kind-mismatch rejection at graph construction, and correct
`is_connected()` state before and after patch edits.

## Acceptance criteria

- [ ] A `PolyProbe` test module is defined locally in the integration test file.
      It declares one poly input port and one poly output port, stores them as
      `PolyInput` / `PolyOutput` fields, and records the values it receives in
      `process()` for inspection by tests.
- [ ] **Initial port delivery**: `HeadlessEngine` is given a plan with one
      `PolyProbe` wired up. After plan-accept, the probe's `PolyInput` and
      `PolyOutput` fields have `connected: true` (i.e. `set_ports` was called
      with the correct concrete types).
- [ ] **Poly cable propagation**: A source module writes a known `[f32; 16]`
      pattern to a poly output; `PolyProbe` reads it via `read_poly`. After
      one tick, the probe's recorded values match the written pattern.
- [ ] **Kind-mismatch at connect()**: Constructing a `ModuleGraph` that connects
      a mono output port to a poly input port returns
      `Err(GraphError::CableKindMismatch)` (or equivalent). The graph is never
      submitted to the engine.
- [ ] **`connected` after cable removal**: A plan with a connected poly cable
      is replaced by a plan with that cable removed. After plan-accept,
      `PolyProbe::voct_in.connected` is `false`.
- [ ] **No spurious `set_ports` calls**: If the patch is reloaded with no
      change to the cable topology, `set_ports` is not called on the surviving
      module (verified by a call counter on `PolyProbe`).
- [ ] All tests run without audio hardware (using `HeadlessEngine`).
- [ ] `cargo test -p patches-integration-tests` passes with no failures or
      `#[ignore]`-skipped poly tests.
- [ ] `cargo clippy -p patches-integration-tests` clean.

## Notes

`HeadlessEngine` is defined in `patches-integration-tests/src/lib.rs`. It will
need to apply `port_updates` in step 3 of `adopt_plan` (added in T-0116); if
T-0116 already updated it this ticket only needs to verify the behaviour.

The no-spurious-update test guards against the planner emitting `port_updates`
entries for modules whose port assignments did not change — the same
change-detection requirement as `connectivity_updates` in E015.
