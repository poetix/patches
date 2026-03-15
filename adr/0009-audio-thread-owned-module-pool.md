# ADR 0009 — Audio-thread-owned module pool

**Date:** 2026-03-02
**Status:** Accepted

## Context

### State preservation across replans does not work in normal flow

ADR-0003 described a design for preserving module state (oscillator phase, envelope
position, etc.) across hot-reloads by passing a `prev_plan: Option<ExecutionPlan>` to
`Planner::build`. The intent was that module instances from the previous plan would be
extracted into a `ModuleInstanceRegistry` and reused in the new plan.

In practice, this does not work in the normal replan path through `PatchEngine`. After a
successful `swap_plan`, the audio thread exclusively owns the running plan;
`PatchEngine::held_plan` is `None`. The next call to `update` therefore passes `None` to
`Planner::build`, producing a plan with entirely fresh, stateless module instances. State
preservation via `prev_plan` only fires in the channel-full retry case — an edge condition,
not the common case.

### `held_plan` is architecturally confused

The `held_plan: Option<ExecutionPlan>` field in `PatchEngine` conflates two unrelated
concerns:

1. **State source for the next build.** A previous plan's module instances carry state that
   should be reused when rebuilding. But once a plan is pushed to the audio thread, the
   control thread loses ownership of it and cannot use it as a state source.

2. **Retry buffer for a full channel.** If `swap_plan` returns `Err`, the plan is stashed
   for retry. This is a valid need but unrelated to state preservation, and using the same
   field for both leads to an incomplete design where state preservation only occurs as a
   side-effect of the retry path.

### The ownership gap

The fundamental problem is that the module instances carrying live audio state are owned by
the audio thread's running `ExecutionPlan`. The control thread cannot access them without
either:

- A return channel (audio thread sends the old plan back when accepting a new one) — rejected
  in ADR-0003 on complexity grounds, and also circular: the old plan's state is needed
  *during* the build of the new plan, not after it is sent.
- Shared ownership (`Arc<Mutex<dyn Module>>`) — requires locking on the audio thread;
  incompatible with the no-blocking constraint.

### The buffer pool analogy

The buffer pool (introduced in ADR-0001 and externalised in ADR-0004) demonstrates the
correct pattern: the audio thread owns the pool; the control thread manages indices. Modules
that survive a replan keep their buffer indices unchanged, so their signals are continuous
across the plan swap without any cross-thread value transfer. The same pattern applies to
module instances.

## Decision

Introduce an **audio-thread-owned module pool**, symmetric with the buffer pool.

### Structure

```rust
// Audio thread (SoundEngine)
module_pool: Box<[Option<Box<dyn Module>>]>   // fixed capacity, pre-allocated

// Control thread (Planner, via ModuleAllocState)
pool_map: HashMap<InstanceId, usize>          // InstanceId → pool slot index
freelist: Vec<usize>                          // recycled slot indices (LIFO)
next_hwm: usize                               // high-water mark
```

### Plan structure

`ExecutionPlan` changes from owning module instances to referencing pool slots:

```rust
pub struct ModuleSlot {
    pub pool_index: usize,         // was: pub module: Box<dyn Module>
    pub input_buffers: Vec<usize>,
    pub input_scales: Vec<f32>,
    pub output_buffers: Vec<usize>,
    pub input_scratch: Vec<f32>,
    pub output_scratch: Vec<f32>,
}

pub struct ExecutionPlan {
    pub slots: Vec<ModuleSlot>,
    pub new_modules: Vec<(usize, Box<dyn Module>)>,  // (pool_index, instance)
    pub tombstones: Vec<usize>,                       // pool indices to remove
    pub to_zero: Vec<usize>,                          // buffer slots to zero
    pub audio_out_index: usize,
    pub signal_dispatch: Box<[(InstanceId, usize)]>,  // InstanceId → pool_index
}
```

### Plan build

`build_patch` accepts a `&ModuleAllocState` (analogous to `&BufferAllocState`) and returns
an updated `ModuleAllocState`. For each module in the new graph:

- **Surviving module** (InstanceId already in `pool_map`): reuse its existing pool index;
  no entry in `new_modules`. The module continues running in the pool with its current state.
- **New module** (InstanceId not in `pool_map`): acquire a slot from the freelist or HWM,
  initialise the instance with the current sample rate, add to `new_modules`.
- **Tombstoned module** (InstanceId in old `pool_map` but not in new graph): add its pool
  index to `tombstones`, return slot to the freelist.

The `registry` parameter is removed from `build_patch`. Module state preservation is now
automatic and requires no explicit mechanism.

### Plan acceptance (audio thread)

On `consumer.pop()`:

1. Install `new_modules`: `pool[idx] = Some(module)` for each entry.
2. Process `tombstones`: `pool[idx].take()` for each entry — `Box<dyn Module>` drops here.
3. Zero `to_zero` buffer slots.
4. Replace `current_plan`; begin ticking.

### Initialisation

New module instances are initialised with the sample rate on the control thread, inside
`swap_plan`, before the plan is pushed. Surviving modules are never re-initialised.
`ExecutionPlan::initialise()` is removed.

### Removal of `held_plan` and `ModuleInstanceRegistry`

`PatchEngine::held_plan` and `ExecutionPlan::into_registry` are removed.
`ModuleInstanceRegistry` is removed from `patches-core`. `PatchEngine::update` simply
returns `PatchEngineError::ChannelFull` when `swap_plan` fails; it does not stash any
plan. There is no current design for rapid consecutive replans; this is recorded as a
known gap.

## Consequences

**State preservation is automatic.** Surviving modules never leave the pool. Their live
audio state — oscillator phase, delay write pointer, envelope position — is continuous
across plan swaps with no additional mechanism.

**Tombstoned modules drop on the audio thread.** `pool[idx].take()` calls `free()` on the
audio thread. This is in the same hazard class as allocation, but tombstoning occurs at
human-speed replan intervals, not per-sample. The current design already drops the entire
`ExecutionPlan` (all modules) on the audio thread when a plan swap occurs; this is a
continuation of that accepted trade-off.

**Deferred mitigation for strict real-time.** If `free()` on the audio thread proves
problematic in practice, tombstoned boxes can instead be pushed onto a bounded return
channel (audio → control) and dropped on a cleanup thread. This is a contained, incremental
change to the plan acceptance code.

**Fixed module pool capacity.** An upper bound on concurrent module instances, configurable
at `SoundEngine` construction. Exceeding it returns `BuildError::ModulePoolExhausted` at
plan-build time — not a panic or silent failure at audio time. A default of 1024 slots
(16 KiB of `Option<Box<dyn Module>>` pointers) is sufficient for all expected patch sizes.

**Planner holds `ModuleAllocState` internally.** Callers no longer pass `prev_plan` to
`Planner::build`. The planner's state is self-contained and threads forward automatically,
mirroring `BufferAllocState`.

**`signal_dispatch` maps `InstanceId → pool_index`.** The binary-search dispatch used by
the audio thread at control-rate ticks now dispatches directly into the pool rather than
into slot indices.

**`ModuleInstanceRegistry` and `held_plan` are removed entirely.** ADR-0003 is superseded.
The two open tickets that depended on the old design (T-0031 state preservation, T-0034
held-plan channel-full path) are resolved by this epic.

## Alternatives considered

**Return channel (audio thread returns old plan to control thread).** The control thread
needs module instances *before* building the new plan in order to preserve their state.
A return channel provides them *after* the new plan is sent — too late. Rejected.

**`held_plan` populated after every successful swap.** If `PatchEngine` kept a copy of
every plan it built, it could use that copy as a state source for the next build. But
`rtrb::push` takes ownership by value — once pushed, the control thread has no copy.
Cloning `ExecutionPlan` requires `Module: Clone`, which was rejected in ADR-0002.

**Snapshot / restore (`Module::snapshot()` / `Module::restore()`).**  Would require every
module to implement serialisation of its state. The pool approach preserves state
without any module-level API change. Rejected.
