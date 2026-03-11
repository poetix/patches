# ADR 0014 — Control signals convey external events, not parameter updates

**Date:** 2026-03-06
**Status:** Superseded by [ADR 0016 — MIDI as the sole external control mechanism](0016-midi-only-control-architecture.md)

## Context

The `Module` trait includes a `receive_signal` method and a `ControlSignal` type.
The initial implementations of this method in several modules — `Glide`,
`ClockSequencer`, `SineOscillator`, and others — used it to accept
`ControlSignal::ParameterUpdate` events and apply them directly to internal
parameter state (e.g. updating a frequency or a glide time from a MIDI CC value).

This created two distinct mechanisms for changing module parameters:

1. Graph modification followed by re-planning (the `update_validated_parameters`
   / planner path).
2. Direct delivery of `ControlSignal::ParameterUpdate` via `receive_signal`.

Having both mechanisms causes several problems:

- **The planner's `NodeState` becomes stale.** Parameters applied through
  `receive_signal` bypass the planner entirely. If the patch is subsequently
  hot-reloaded, the planner diffs against its last-known `ParameterMap`, which
  does not reflect the values written by `receive_signal`. The module may receive
  an unwanted revert or a spurious update. ADR 0003 documents related state-
  freshness trade-offs and applies here too.

- **Two code paths for one concern.** Every module that wants live parameter
  control must duplicate its update logic: once in `update_validated_parameters`
  and again in `receive_signal`. Validation (range clamping, enum membership
  checks) performed in the former is silently bypassed in the latter.

- **Confusion about the purpose of `receive_signal`.** ADR 0008 describes a
  control architecture in which MIDI and OSC events are converted by dedicated
  *receiver modules* into audio-rate cable signals — pitch, gate, velocity, and
  so on — which then flow through the patch graph like any other signal. The
  `receive_signal` method exists to deliver raw events to those receiver modules.
  Using it to set parameters on general-purpose synthesis modules conflates two
  unrelated concerns.

## Decision

`ControlSignal` and `receive_signal` are reserved exclusively for delivering
external real-time events — MIDI note-on/off, pitch bend, CC, OSC messages, and
similar — to modules whose sole purpose is to translate those events into cable
signals. Such modules act as the boundary between the external world and the patch
graph.

**`receive_signal` must not be used to update parameters.** The only sanctioned
mechanism for changing a module's parameters is graph modification followed by
re-planning: the caller updates the `ParameterMap` in `ModuleGraph`, triggers a
new plan build, and the planner delivers the change via `update_validated_parameters`
during plan adoption. This path goes through validation, is visible to the planner's
diff logic, and keeps `NodeState` consistent.

As a direct consequence, `receive_signal` implementations have been removed from
all general-purpose module implementations (`Glide`, `ClockSequencer`,
`SineOscillator`, `SawtoothOscillator`, `SquareOscillator`, `StepSequencer`).
The default no-op provided by the trait is sufficient for all modules that are not
dedicated MIDI/OSC receiver modules.

## Consequences

**One mechanism for parameter updates.** Hot-reload, live-coding, and any future
GUI or automation layer all go through the same path: edit the graph, trigger
re-planning. The planner's `NodeState` remains the single source of truth.

**`receive_signal` has a clear, narrow scope.** Future MIDI receiver modules
implement it to translate events into signals. All other modules ignore it via the
default no-op. The distinction is enforced by convention rather than the type
system.

**Parameter changes in response to MIDI require a receiver module.** A MIDI CC
that previously called `receive_signal` to update, say, a filter cutoff must
instead be routed as a cable signal from a MIDI receiver module through whatever
scaling and mapping modules are needed to the filter's CV input port. This is more
explicit, more flexible (multiple modules can be driven by the same CC), and
consistent with the patch-graph architecture.

## Alternatives considered

### Keep `receive_signal` for parameters, document the stale-NodeState caveat

The stale-state problem (documented in ADR 0003) is manageable when the planner
rebuilds on every live-coding reload, but becomes a latent bug as the system grows.
Accepting the inconsistency and documenting it around an ad-hoc workaround was
rejected in favour of eliminating the dual-path entirely.

### Add a `notify_parameters` callback driven by the ring buffer

A dedicated path could deliver validated parameter updates at control rate through
the ring buffer, bypassing the full plan rebuild. This is a valid optimisation for
the future (a parameter-only update is cheaper than a full plan swap) but adds
significant complexity. It is deferred; the current approach of full re-planning on
parameter change is fast enough for interactive live-coding.
