---
id: "E014"
title: Planner v2 — graph-diffing and data-driven planning
created: 2026-03-04
adr: "0012"
tickets: ["0070", "0071", "0072", "0073", "0074", "0075", "0076"]
---

## Summary

The planning layer (`build_patch`, `Planner`, `PatchEngine`, `SoundEngine`) still uses
the v1 `ModuleGraph` API: it accesses live module instances, consumes the graph, and
rebuilds every module on every re-plan. ADR 0012 redesigns planning as graph diffing —
the planner owns module identity, borrows a topology-only graph, diffs against its
previous state, and only instantiates new or type-changed modules. Parameter-only
changes are carried as diffs in the `ExecutionPlan` and applied on the audio thread
without rebuilding anything.

This epic also straightens out the startup sequence: `SoundEngine` splits into
open (get sample rate) and start (begin audio thread), so the initial plan can be
built with the real `AudioEnvironment` before audio begins.

## Tickets

| ID   | Title                                                  | Priority | Depends on |
|------|--------------------------------------------------------|----------|------------|
| 0070 | Make `update_validated_parameters` infallible          | high     | —          |
| 0071 | Add `is_sink` to `ModuleDescriptor`                    | high     | —          |
| 0072 | Two-phase `SoundEngine` startup (open / start)         | high     | —          |
| 0073 | Introduce `PlannerState` and graph-diffing `build_patch` | high   | 0071       |
| 0074 | Parameter diffs in `ExecutionPlan`                     | high     | 0070, 0073 |
| 0075 | Rewrite `PatchEngine` for v2 planner                   | high     | 0072, 0074 |
| 0076 | Planner v2 integration tests                           | medium   | 0075       |

## Definition of done

- `build_patch` borrows `&ModuleGraph` and takes `&Registry`, `&AudioEnvironment`,
  `&PlannerState`; returns `(ExecutionPlan, PlannerState)`.
- Surviving modules are never re-instantiated — only new and type-changed nodes
  trigger `Registry::create()`.
- Parameter-only changes produce `parameter_updates` in the `ExecutionPlan`; the
  audio callback applies them via `update_validated_parameters` on plan adoption.
- `update_validated_parameters` is infallible across all modules and the `Module` trait.
- `ModuleDescriptor` has `is_sink: bool`; `AudioOut` returns `true`, all others `false`.
- `SoundEngine` exposes `open()` → `start()` two-phase startup; sample rate is
  available after `open()` before audio begins.
- `PatchEngine::start(graph)` opens the device, builds the initial plan with the
  real sample rate, then starts the audio thread.
- `cargo build`, `cargo test`, `cargo clippy` clean with no new warnings.
- No `unwrap()` or `expect()` in library code.
