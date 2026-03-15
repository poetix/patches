---
id: "E003"
title: Module lifecycle improvements
created: 2026-02-28
tickets: ["0017", "0018", "0019", "0021"]
---

## Summary

A set of related improvements to module lifecycle, identity, and connectivity that
establish cleaner runtime behaviour and lay the infrastructure for live-coding
hot-reload:

1. **AudioEnvironment / initialise** — decouple environment parameters (sample rate)
   from per-sample processing. `sample_rate` was previously passed on every
   `process()` call; it is now supplied once at plan activation via `initialise()`
   and stored by each module.

2. **InstanceId and module registry** — every module instance gains a stable,
   immutable identity so that re-planning (changing the patch graph at runtime) can
   locate and reuse existing instances, preserving their internal state (e.g.
   oscillator phase). A stateless `Planner` drives re-planning; a higher-level
   `PatchEngine` coordinates planning and audio.

3. **Input scaling factor** — each connection carries a scalar in `[-1, 1]` applied
   to the signal at read-time in `tick()`. Validated at graph-build time; applied as
   a single multiply per input per sample at audio time with no branching.

## Acceptance criteria

- [x] All four tickets closed
- [x] `cargo build`, `cargo test`, `cargo clippy` all clean
- [x] `cargo run --example sine_tone` still plays audio correctly
- [x] `Module::process` no longer accepts `sample_rate`
- [x] All module instances carry an `InstanceId`
- [x] `Planner::build(graph, prev_plan)` reuses matching module instances by `InstanceId`
- [x] `ModuleGraph::connect()` accepts a `scale: f32` parameter

## Tickets

| ID   | Title                                               | Priority |
|------|-----------------------------------------------------|----------|
| 0017 | AudioEnvironment and `initialise`                   | medium   |
| 0018 | Module `InstanceId`                                 | medium   |
| 0019 | ModuleInstanceRegistry, Planner, and PatchEngine    | medium   |
| 0021 | Input connection scaling factor                     | low      |

(Ticket number 0020 was not used; the sequence jumps from 0019 to 0021.)

## Architecture introduced

### AudioEnvironment

`AudioEnvironment { sample_rate: f32 }` lives in `patches-core`. `Module::initialise`
is called once when a plan is activated; `Module::process` no longer receives
`sample_rate`. `SoundEngine` creates the environment from the CPAL stream config and
calls `initialise` on every new plan before it reaches the audio thread.

### InstanceId and state preservation

Each module stores an `InstanceId(u64)` assigned at construction from a global
`AtomicU64`. `ExecutionPlan::into_registry()` consumes a plan and moves all module
instances into a `ModuleInstanceRegistry` keyed by `InstanceId`.

`Planner::build(graph, prev_plan)` extracts a registry from `prev_plan` (if any),
then calls `build_patch(graph, Some(&mut registry))`. For each module consumed from
the graph, `build_patch` checks whether the registry holds an instance with the same
`InstanceId`; if so, the old instance (with its accumulated state) is used in place
of the fresh one from the graph.

### PatchEngine and state freshness

`PatchEngine` coordinates `Planner` and `SoundEngine`. It holds a "held plan" (the
most recently built plan) which is passed to the Planner for state extraction at each
re-plan. The newly built plan becomes the new held plan; the old held plan is sent to
the audio engine. The state preserved in the new plan therefore reflects module state
at *build time*, not the engine's live audio state at *swap time*. For live-coding
use cases (re-plans every few seconds) the difference is negligible. This trade-off
is documented in `adr/0003-planner-state-freshness.md`.

When the engine's rtrb channel is full, `PatchEngine::update` returns
`PatchEngineError::ChannelFull` and stashes the new plan back into `held_plan`,
preserving the state for the next retry.

### Input scaling

`Edge` carries `scale: f32`, validated at `connect()` time
(`ScaleOutOfRange` error if outside `[-1, 1]` or non-finite). At build time the
scale is resolved from each edge and stored as `input_scales: Vec<f32>` on
`ModuleSlot` (one entry per input port; `1.0` for unconnected inputs). In `tick()`:

```rust
slot.input_scratch[j] = buffers[buf_idx][ri] * slot.input_scales[j];
```

No map lookup or branching at audio time.

## Notes

**No new crates or external dependencies.** All changes are within existing
`patches-core`, `patches-modules`, and `patches-engine`.

**`Planner` is stateless** and can be unit-tested without audio hardware by calling
`planner.build(graph, prev)` directly.

**Backward-compatible call sites.** All existing `connect()` call sites pass `1.0`
as the scale, preserving identical audio output. The toposort in `build_patch` was
updated for the new 5-tuple edge list format returned by `edge_list()`.
