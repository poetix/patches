# ADR 0012 — Planner v2: graph-diffing and data-driven planning

**Date:** 2026-03-04
**Status:** Proposed

## Context

### The planner still uses the v1 ModuleGraph API

ADR 0011 redesigned `ModuleGraph` as a topology-only structure: nodes hold
`(ModuleDescriptor, ParameterMap)` rather than `Box<dyn Module>`. E013 migrated
all module implementations to the v2 contract (`describe` / `prepare` /
`update_validated_parameters`).

However, the planning layer in `patches-engine` — `build_patch()`, `Planner`,
`PatchEngine`, and `SoundEngine` — still uses the v1 graph API. `build_patch()`
calls `graph.get_module(id)` to access live module instances, reads
`m.as_sink()`, `m.instance_id()`, and `m.descriptor()`, and calls
`graph.into_modules()` to extract `Box<dyn Module>` values. These methods do not
exist on the v2 `ModuleGraph`.

### Module identity is in the wrong place

In v1, `InstanceId` is assigned at module construction time. The graph carries
live modules with their IDs, and the planner matches IDs across successive builds
to determine which modules survive. This means:

- Every re-plan instantiates every module, even survivors. Survivor instances
  are immediately dropped — the stateful instance in the audio-thread pool is
  the one that continues. This is wasteful.
- Module identity is a side-effect of construction, not a deliberate assignment
  by the planner. The planner discovers identity; it does not control it.

### Parameter changes require full graph rebuilds

There is no mechanism to communicate "only the frequency parameter on node X
changed" to the audio engine. Every edit — even a single parameter tweak —
requires a full plan rebuild with new module instantiation for all nodes. The
planner cannot distinguish a parameter change from a topology change.

### Module construction requires AudioEnvironment

The v2 `Module::build()` takes `&AudioEnvironment` (which includes the sample
rate). The sample rate is only known after `SoundEngine::start()` opens the audio
device. The current startup sequence builds the initial plan in
`PatchEngine::new()` before `start()` is called, relying on a post-construction
`Module::initialise()` hook to inject the sample rate later. The v2 trait
replaces `initialise()` with `prepare()`, which is called during `build()` — so
module construction and environment injection are unified, and the old two-phase
startup no longer works.

## Decision

### The planner owns module identity

The planner maintains a `PlannerState` that maps `NodeId` to module identity and
last-known parameters:

```rust
struct NodeState {
    module_name: &'static str,
    instance_id: InstanceId,
    parameter_map: ParameterMap,
}

struct PlannerState {
    nodes: HashMap<NodeId, NodeState>,
    buffer_alloc: BufferAllocState,
    module_alloc: ModuleAllocState,
}
```

`InstanceId` is assigned by the planner when a node first appears, not by module
construction. The planner controls identity; modules receive their ID at
construction time from the planner.

### Planning is graph diffing

On each build, the planner compares the previous `PlannerState` with the incoming
`ModuleGraph` by `NodeId`:

| Previous state | New graph | Action |
|----------------|-----------|--------|
| absent | present | **New** — assign `InstanceId`, instantiate via `Registry` |
| present | absent | **Removed** — tombstone module pool slot |
| present, same `module_name` | present | **Surviving** — reuse `InstanceId`, diff parameters |
| present, different `module_name` | present | **Type change** — tombstone old, instantiate new |

Only new and type-changed nodes trigger module instantiation. Surviving modules
are never re-instantiated — the audio-thread pool holds the live instance.

### The graph is borrowed, not consumed

```rust
pub fn build_patch(
    graph: &ModuleGraph,
    registry: &Registry,
    env: &AudioEnvironment,
    prev_state: &PlannerState,
    pool_capacity: usize,
    module_pool_capacity: usize,
) -> Result<(ExecutionPlan, PlannerState), BuildError>
```

The graph is a serialisable config. It is not consumed by planning — the caller
retains it for inspection, serialisation, or comparison.

### Parameter diffs are carried in the ExecutionPlan

```rust
pub struct ExecutionPlan {
    // ... existing fields ...
    pub parameter_updates: Vec<(usize, ParameterMap)>,
}
```

Each entry is `(pool_index, diff_map)` where `diff_map` contains only the
parameter keys whose values changed between the previous and current graph. The
planner validates diffs off the audio thread via `validate_parameters`. The audio
callback applies them on plan adoption:

```rust
// In AudioCallback::receive_plan(), after installing new modules:
for (idx, params) in &new_plan.parameter_updates {
    self.module_pool.update_parameters(*idx, params);
}
```

`ModulePool::update_parameters` calls `module.update_validated_parameters(params)`
on the module at that slot. This is the same code path used during module
construction — no separate mechanism for parameter changes.

### `update_validated_parameters` becomes infallible

The signature changes from:

```rust
fn update_validated_parameters(&mut self, params: &ParameterMap) -> Result<(), BuildError>;
```

to:

```rust
fn update_validated_parameters(&mut self, params: &ParameterMap);
```

If parameters have been validated against the descriptor, the module accepts or
ignores them. There is no meaningful error to return — validation is the
caller's responsibility, and it happens off the audio thread before the diff
reaches the plan. This removes boilerplate `Ok(())` returns from every module
implementation and eliminates the awkwardness of calling a `Result`-returning
method in the audio callback.

`Module::update_parameters` (the validating wrapper) continues to return
`Result<(), BuildError>` for callers that need validation, but it delegates to
the now-infallible `update_validated_parameters`.

`Module::build` (the default implementation) likewise continues to return
`Result<Self, BuildError>` since validation in `update_parameters` can still
fail at construction time.

### `ModuleDescriptor` gains `is_sink`

```rust
pub struct ModuleDescriptor {
    // ... existing fields ...
    pub is_sink: bool,
}
```

The planner uses this to identify the audio output node without requiring a live
module instance. `AudioOut::describe()` returns `is_sink: true`; all other
modules return `false`. This is a compile-time constant — zero cost.

### Startup sequence changes

Since `Module::build()` requires `AudioEnvironment` (including sample rate), and
the sample rate is only known after the audio device is opened, module
instantiation is deferred until the device has been opened.

`SoundEngine` splits device initialisation into two phases:

1. **Open** — open the audio device, query its configuration, obtain the sample
   rate. This creates the `AudioEnvironment` but does not start the audio
   thread.
2. **Start** — spawn the audio thread and begin playback.

This lets `PatchEngine` build the initial plan with the real sample rate before
the audio thread begins:

1. `PatchEngine::new(registry)` — stores registry and creates planner.
2. `PatchEngine::start(initial_graph)` — opens the audio device, obtains the
   sample rate, builds the initial plan via the planner (with the real
   `AudioEnvironment`), then starts the audio thread. Sound begins immediately.
3. `PatchEngine::update(graph)` — builds a new plan and sends it to the running
   engine via `swap_plan()`.

`SoundEngine::swap_plan()` no longer calls `module.initialise()`. Modules arrive
fully constructed via `Registry::create()` → `Module::build()`, ready to install
directly.

### Plan deallocation

When the audio callback adopts a new plan, the old plan (including any
`ParameterMap` diffs it carried) is dropped. This deallocation happens on the
audio thread at plan-swap time — once per reload, not per sample.

If profiling shows that drop overhead at plan-swap time is a concern, stale plans
can be punted to the cleanup thread (the same `rtrb` channel used for tombstoned
modules). The infrastructure already exists; the change is to push the old plan
onto the cleanup channel rather than dropping it in place. This is not done
initially — the simpler approach is adopted first and the optimisation is
available if needed.

## Consequences

### Benefits

- **No wasted instantiation.** Surviving modules are never re-instantiated. Only
  new and type-changed nodes trigger `Registry::create()`.

- **Minimal parameter updates.** Only changed parameter values are communicated
  to the audio thread, via the same `update_validated_parameters` path used
  during construction. No separate signal mechanism needed.

- **Graph is not consumed.** The caller retains the graph after planning —
  enabling serialisation, comparison, and inspection without rebuilding.

- **Identity is explicit.** The planner assigns `InstanceId`s deliberately rather
  than discovering them as a side-effect of module construction.

- **Simpler module implementations.** `update_validated_parameters` is infallible,
  removing `Result` boilerplate from every module.

- **Startup is cleaner.** No two-phase construction/initialisation split. The
  device is opened first (yielding the sample rate), then the initial plan is
  built with the real `AudioEnvironment`, then the audio thread starts. Modules
  are fully constructed once.

### Costs

- **`SoundEngine` gains a two-phase startup.** Device opening and audio thread
  start are split so the sample rate is available for plan building. This adds
  a small amount of API surface to `SoundEngine`.

- **Planner parameter surface grows.** `build()` now requires `&Registry` and
  `&AudioEnvironment` in addition to the graph. This is inherent — the planner
  is now responsible for module instantiation.

- **`update_validated_parameters` signature change.** Ripples through all module
  implementations and the `Module` trait. The change is mechanical (remove
  `-> Result<(), BuildError>` and `Ok(())` returns) but touches every module.

- **`ModuleDescriptor` gains a field.** All `describe()` implementations must
  set `is_sink`. Mechanical — all non-AudioOut modules set `false`.

## Alternatives considered

### Communicate parameter changes via ControlSignal

Parameter diffs could be delivered as `ControlSignal::ParameterUpdate` messages
through the existing signal ring buffer. Rejected: `ControlSignal` is the
channel for external control sources (MIDI, OSC). Parameter updates from graph
edits are a different concern — they should go through the module's own parameter
update mechanism (`update_validated_parameters`), and they should be atomic with
plan adoption rather than subject to ring buffer timing.

### Return the graph from `build_patch` instead of borrowing

`build_patch` could consume and return the graph (like the v1 API). Rejected:
borrowing is simpler, and consuming a topology-only config has no benefit — there
are no resources to transfer.

### Keep `update_validated_parameters` fallible

The `Result` return could be preserved for forward compatibility. Rejected: if
parameters have been validated against the descriptor, there is no error to
report. Preserving the `Result` forces every module to write `Ok(())` and every
audio-thread call site to ignore or debug-assert the result. The infallible
signature is honest about the contract.
