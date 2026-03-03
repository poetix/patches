# ADR 0003 — Planner state freshness trade-off

## Status

Superseded by ADR-0009 (ticket 0044)

The audio-thread-owned module pool introduced in ADR-0009 makes the
`ModuleInstanceRegistry` / `held_plan` / `into_registry` mechanism described
here unnecessary. Surviving modules remain in the pool across plan swaps, so
state preservation is automatic and requires no explicit cross-thread transfer.
`PatchEngine::held_plan`, `ExecutionPlan::into_registry`, and
`ModuleInstanceRegistry` have been removed.

## Context

When the patch graph is rebuilt at runtime (hot-reload / live-coding), stateful
module instances — such as oscillators carrying a phase value — should ideally
continue from where they were, rather than resetting to their initial state.

The `SoundEngine` drives an `ExecutionPlan` on the audio thread. The control
thread builds new plans and hands them over via a wait-free SPSC channel. Because
Rust's ownership model prevents simultaneous access from two threads, the plan
currently running in the audio thread cannot be inspected or mutated from the
control thread while it is in use.

## Decision

**State is preserved from the time the previous plan was built, not from the
engine's live audio state at swap time.**

Concretely:

1. `ExecutionPlan::into_registry()` consumes a plan and moves all its module
   instances into a `ModuleInstanceRegistry`, keyed by `InstanceId`.
2. `build_patch(graph, Some(&mut registry))` checks the registry for each module
   in the new graph: if a module with a matching `InstanceId` is found it replaces
   the fresh placeholder from the graph, carrying over the instance's internal state.
3. The `Planner::build(graph, prev_plan)` helper wraps these two steps. Callers
   retain the previous plan and pass it back at each re-plan.
4. `PatchEngine` (the optional high-level coordinator) keeps a *held plan* that
   is consumed for state extraction on the next call to `update`. When the engine's
   single-slot channel is full, the newly built plan is stashed as the held plan and
   retried on the next `update` call, preserving its module state for that retry.

Because `into_registry` consumes the plan, the module instances that carry state
are the ones from the *previous build*, not from the audio thread's running plan.
The audio thread may have advanced those modules' state further (e.g. oscillator
phase) during the time the old plan was running, but that additional progress is
not visible to the control thread.

## Consequences

**Acceptable for live-coding use cases.** Re-plans happen at human interaction
speed (every few seconds at most). The phase error introduced by not capturing
live audio state is at most `re_plan_period * sample_rate` samples — equivalent
to a phase advance that is inaudible in the context of a live edit. For most
stateful modules (oscillators, envelopes, delay write pointers) the discontinuity
from a cold reset is more disruptive than the discontinuity from a slightly stale
starting state.

**Not suitable for sample-accurate parameter updates.** If a future use case
requires sample-accurate state hand-off (e.g. seamless looping with a precise
playback position), a different mechanism will be needed — for example, a
parameter-update channel that writes directly into the running plan without
replacing it.

**Planner is fully testable without audio hardware.** Because state is passed via
`Option<ExecutionPlan>` (not via any channel), the planner can be exercised in
unit tests by calling `Planner::build` directly and inspecting the resulting plan.
No CPAL device or running engine is required.

**InstanceId must be stable across rebuilds.** For state to be preserved, modules
in the new graph must carry the same `InstanceId` as their counterparts in the
old plan. The current implementation assigns IDs via a global atomic counter, so
callers must either (a) reuse the same module instances across rebuilds by
explicitly extracting them from the old registry, or (b) use a higher-level DSL
layer that assigns stable IDs (e.g. by module name or graph position) rather than
relying on auto-increment.

## Alternatives considered

**Retrieve the running plan from the engine via a return channel.** Rejected
because it introduces a two-way lock-free channel with complex ownership semantics
and makes `SoundEngine` responsible for exposing plan internals. The engine's job
is to run plans, not to manage state hand-off.

**Snapshot module state into a side-channel structure.** Would require a
`Module::snapshot() / restore()` API or a generic serialisation mechanism. The
current approach of moving instances directly is simpler and preserves all state
without requiring modules to implement any extra trait.
