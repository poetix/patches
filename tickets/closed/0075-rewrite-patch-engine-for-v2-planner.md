---
id: "0075"
title: Rewrite PatchEngine for v2 planner
priority: high
epic: "E014"
depends: ["0072", "0074"]
created: 2026-03-04
---

## Summary

Rewrite `PatchEngine` to use the v2 planner (graph-diffing `build_patch` with
`PlannerState`) and the two-phase `SoundEngine` startup. The new lifecycle is:

1. `PatchEngine::new(registry)` — stores the registry and creates the planner.
2. `PatchEngine::start(initial_graph)` — opens the audio device, obtains the sample
   rate, builds the initial plan with the real `AudioEnvironment`, then starts the
   audio thread.
3. `PatchEngine::update(graph)` — diffs against the previous `PlannerState`, builds a
   new plan, and sends it to the running engine.

## Acceptance criteria

- [ ] `PatchEngine::new()` takes a `Registry` (not a `ModuleGraph`). No plan is built
      at construction time.
- [ ] `PatchEngine::start(graph: &ModuleGraph)` opens the audio device, queries the
      sample rate, builds the initial plan via the v2 planner (using `&Registry`,
      `&AudioEnvironment`, `PlannerState::empty()`), then starts the audio thread.
- [ ] `PatchEngine::update(graph: &ModuleGraph)` builds a new plan by diffing against
      the stored `PlannerState`, then sends it via `SoundEngine::swap_plan()`.
- [ ] The graph is borrowed in both `start()` and `update()` — not consumed.
- [ ] `PatchEngine` holds the `PlannerState` (updated after each successful build)
      and the `Registry`.
- [ ] `Module::initialise()` is no longer called anywhere in the engine. Modules are
      fully constructed by `Registry::create()` → `Module::build()`.
- [ ] Existing examples (`sine_tone`, `demo_synth`, etc.) are updated to the new
      `PatchEngine` API.
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

This ticket ties together T-0072 (two-phase `SoundEngine`), T-0073 (graph-diffing
planner), and T-0074 (parameter diffs). It is the integration point where the new
planning pipeline replaces the old one end-to-end.

The `Planner` struct may be simplified or inlined into `PatchEngine` at this point,
since `PlannerState` carries the persistent state and `build_patch` is a free function.

See ADR 0012 § "Startup sequence changes".
