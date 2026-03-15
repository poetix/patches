# ADR 0004 — Stable buffer pool, freelist allocation, and module destruction

## Status

Accepted (epic E005, tickets 0024–0026)

## Context

### The re-plan discontinuity problem

`ExecutionPlan` owns the flat cable buffer pool (`buffers: Vec<[f32; 2]>`). Each
re-plan allocates a fresh, zero-initialised pool and assigns new buffer indices
to all output ports. On plan acceptance, the audio thread begins reading from an
all-zero pool, producing a discontinuity that propagates through the graph — one
zeroed tick per cable hop. For audio-rate signals this is inaudible (~23 μs at
44100 Hz). For slow-moving CV cables (e.g. a static detune offset modulating a
filter cutoff) even a single-sample blip to zero may produce an audible click.

### Why copying old buffer values during re-plan does not work

The obvious fix — copy the old pool's values into the new pool during `build_patch`
— is not safe. The audio thread continuously writes to the old pool while the
control thread is building the new plan. Any read of the old pool from the control
thread is a data race. Synchronising the copy with the audio thread would require
either blocking the audio thread (violating the real-time constraint) or a complex
double-buffering scheme.

### The alternative: pool outlives the plan

If the pool lives outside `ExecutionPlan` and persists across re-plans, unchanged
cables can keep their buffer indices. The audio thread reads and writes the same
memory before and after the plan swap — no discontinuity, no race.

### Dynamic pool growth is unsafe at audio time

A `Vec` that grows reallocates and moves its storage. The audio thread holds
references into the pool's backing memory; after a reallocation those references
dangle. The pool therefore cannot be grown while the audio thread uses it.
Options:

- **Fixed capacity** (simple, hard upper bound on concurrent output ports).
- **Segmented pool** (chunks of fixed-size arrays; appending a chunk does not
  move existing chunks). More complex; moves the bound from slot count to chunk count.

A fixed capacity is chosen for now. The bound applies to *concurrent* output ports,
not to output port assignments over the engine's lifetime; the freelist handles
lifetime exhaustion (see below).

### Freelist for index recycling

Without a freelist, buffer indices are assigned monotonically and never recycled.
The index space would exhaust over the engine's lifetime even if only a small number
of modules are ever active simultaneously. A freelist on the control thread recycles
released indices, bounding the required pool capacity to the maximum number of
concurrent output ports rather than the cumulative total over the engine's lifetime.

### Zeroing freed slots

Recycled slots carry stale values from their previous connection. If a recycled
slot is read before its new owning module has had a chance to write to it, the
downstream module sees the old value. Zeroing must happen after the plan swap (so
the audio thread is now using the new plan's index assignments) but before the
first `tick()` with the new plan.

Only the audio thread safely writes to the pool (no synchronisation). The control
thread therefore embeds a `to_zero: Vec<usize>` list in the new `ExecutionPlan`;
the audio thread zeroes those slots immediately on plan acceptance, before ticking.
Zeroing happens at *release time* (when a connection is removed and its index is
returned to the freelist) rather than at *acquisition time* (when a recycled index
is next assigned). This ensures a slot is clean even if it sits in the freelist
across several re-plans before being reused.

Freshly allocated slots (those beyond `next_hwm`) are already zero because the
pool is zero-initialised at construction; they do not appear in `to_zero`.

### Module destruction

When a module is removed from the graph, it is currently dropped silently on the
control thread at some point after re-planning. There is no hook for releasing
resources that cannot safely run on an arbitrary thread (e.g. dropping audio device
handles, flushing file I/O). A `destroy()` lifecycle hook provides this.

`destroy()` cannot run on the audio thread (no allocations, no blocking, no I/O).
It should not run synchronously on the control thread at re-plan time, because the
module being destroyed may still be held by the audio thread's running plan until
the plan swap is accepted. Instead, removed modules are sent to a cleanup thread
that calls `destroy()` after the plan swap has propagated.

## Decision

1. **Buffer pool in SoundEngine.** `SoundEngine` pre-allocates a fixed-capacity
   `pool: Box<[[f32; 2]]>` at construction. `ExecutionPlan` holds only indices.
   `tick()` accepts the pool by mutable reference.

2. **Stable index allocation via `BufferAllocState`.** `build_patch` accepts a
   `&BufferAllocState` (containing the previous `output_buf` map, freelist, and
   high-water mark) and returns a new `BufferAllocState`. Unchanged cables reuse
   their existing index. New cables acquire from the freelist or increment the HWM.
   Released cables are pushed to the new freelist and added to `plan.to_zero`.
   The planner remains a pure function; `PatchEngine` threads state forward.

3. **Audio-thread zeroing on plan acceptance.** `ExecutionPlan` carries
   `to_zero: Vec<usize>`. On plan acceptance the audio thread zeroes those pool
   slots before the first tick.

4. **`Module::destroy` and tombstoning.** `Module` gains `fn destroy(&mut self) {}`
   (default no-op). `build_patch` returns the `InstanceId`s of modules absent from
   the new graph (tombstoned). `PatchEngine` sends the corresponding module instances
   to a cleanup thread, which calls `destroy()` before dropping them. The planner
   itself does not mutate or call methods on any module.

## Consequences

**Stable cables are continuous across re-plans.** A cable that exists in both the
old and new graph is guaranteed to read the same pool index before and after the
swap. There is no discontinuity in its signal.

**New and recycled cables start from zero.** This is the correct and expected
behaviour: a newly patched connection has no prior signal.

**Fixed pool capacity.** There is an upper bound on concurrent output ports,
configurable at `SoundEngine` construction. For typical live-coding patches (tens
to low hundreds of modules) a default of 4096 slots is effectively unlimited.
Exceeding it is a `BuildError::PoolExhausted` returned at plan-build time, not a
panic or silent failure on the audio thread.

**Freelist prevents lifetime exhaustion.** The index space does not grow with the
number of re-plans. A patch that repeatedly adds and removes the same module will
recycle the same pool index each time.

**Planner remains pure and testable.** `BufferAllocState` is an explicit input and
output of `build_patch`. No global mutable state. Tests can exercise the planner
without a running engine.

**`Module::destroy` must not block indefinitely.** Modules with long teardown
should manage their own background work and keep `destroy()` itself short.

## Alternatives considered

**Copying old buffer values during re-plan.** Rejected: data race with the audio
thread. Any copy without synchronisation is undefined behaviour; any synchronisation
blocks the audio thread.

**Segmented pool (linked chunks).** Would remove the fixed-capacity constraint by
allowing new chunks to be appended without moving existing ones. Rejected for now
on complexity grounds. The fixed-capacity bound is acceptable for the expected use
cases. If it proves limiting in practice, the segmented approach can replace the
pool implementation without changing the allocation or zeroing logic.

**Zeroing at acquisition time rather than release time.** Equivalent in terms of
correctness. Release-time zeroing is preferred because it zeroes the slot as soon
as it is known to be unused, rather than at the moment it is next assigned. A slot
that sits in the freelist for several re-plans before being recycled is always
guaranteed to be clean when it is finally reused.

**Calling `destroy()` synchronously on the control thread.** Safe for the
tombstoned module instances themselves (they are no longer in the registry), but
the audio thread may still be reading the previous plan's module list when `update`
returns. The cleanup thread makes the ordering explicit and provides a natural
extension point for heavier teardown.
