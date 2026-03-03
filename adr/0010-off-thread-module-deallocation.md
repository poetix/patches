# ADR 0010 — Off-thread module deallocation via cleanup ring buffer

**Date:** 2026-03-03
**Status:** Accepted

## Context

### Deallocation on the audio thread violates the no-allocation invariant

When the audio callback adopts a new `ExecutionPlan`, it removes tombstoned modules from the
module pool by calling `module_pool[idx].take()`. The returned `Box<dyn Module>` is
immediately dropped, which runs the module's destructor and then calls the global allocator's
`dealloc`. Allocator calls are not bounded in time on most operating systems (the allocator
may acquire an internal lock, or trigger page-reclamation work), violating the project's
core constraint that no allocations or blocking occur on the audio thread.

### Current code (the problem site)

In `patches-engine/src/engine.rs`, inside the audio callback closure in `build_stream`:

```rust
for &idx in &new_plan.tombstones {
    module_pool[idx].take();   // ← Box<dyn Module> dropped here on the audio thread
}
```

The slot in the module pool is cleared (so the freelist can reuse it), but the
`Box<dyn Module>` value is also deallocated here. These two concerns must be separated.

### Why `Module: Send` is sufficient

The `Module` trait already requires `Send` (defined in `patches-core/src/module.rs`).
`Box<dyn Module + Send>` can therefore be moved across the thread boundary to a cleanup
thread without unsafe code.

## Decision

Add a dedicated **cleanup thread** and an `rtrb` ring buffer channel from the audio callback
to the cleanup thread. The audio callback still calls `module_pool[idx].take()` to clear
the slot and obtain the `Box<dyn Module>` value, but instead of dropping it immediately it
pushes the value onto the cleanup ring buffer. The cleanup thread periodically drains the
ring buffer, dropping each `Box<dyn Module>` in a non-real-time context.

### Ring buffer sizing and the fallback policy

The cleanup ring buffer must be sized so that overflow is impossible under normal usage.
The worst-case fill in a single plan swap is `module_pool_capacity`: every module in the
pool is removed simultaneously. Because the plan channel holds at most one pending plan, the
audio thread can receive at most two consecutive tombstone batches before it would process
any cleanup:

- Batch 1 (current swap): up to `module_pool_capacity` tombstones → ring buffer fills.
- Batch 2 (next swap): occurs only after the audio thread has already installed batch 1's
  new modules, meaning the pool is repopulated before anything can be tombstoned again.
  Only a second full-pool removal before the cleanup thread runs would overflow.

In practice this requires two consecutive maximum-sized replans with zero cleanup cycles
between them — an extreme edge case that cannot arise under normal live-coding usage. The
ring buffer is sized at `module_pool_capacity` entries.

**Fallback:** if `producer.push()` returns `Err` (buffer full), the `Box<dyn Module>` is
dropped on the audio thread. This is non-RT-safe but correct: the module is not leaked, and
the failure is logged via `eprintln!` so it is visible during development. An alternative
(blocking until the cleanup thread drains) is explicitly rejected as it would cause a
priority inversion and stall the audio callback.

### Cleanup thread lifecycle

The cleanup producer is moved into the audio callback closure, so it is dropped when the
CPAL stream is dropped (i.e., when `SoundEngine::stop` calls `self.stream.take()`).
The cleanup thread detects producer abandonment via `rtrb::Consumer::is_abandoned()` and
exits after draining any remaining entries. `SoundEngine::stop` joins the thread handle
after dropping the stream to ensure all tombstoned modules are fully dropped before
`stop` returns.

## Consequences

### Benefits

- Deallocation of module memory moves entirely off the audio thread, eliminating a class
  of potential audio glitches from allocator contention on hot-reload.
- No change to the `Module` trait, `ExecutionPlan`, `ModuleGraph`, or any module
  implementation.
- The fallback guarantees correctness (no memory leaks) even in the pathological overflow
  case.

### Costs

- A second background thread is now always running alongside the audio stream. Its CPU
  usage is negligible (it is mostly sleeping), but it adds a small amount of complexity to
  the engine lifecycle.
- `SoundEngine::stop` now blocks briefly to join the cleanup thread. This is acceptable
  because `stop` is called from the non-real-time control thread.
- The `rtrb` ring buffer for the cleanup channel adds a fixed allocation of
  `module_pool_capacity` pointer-sized entries (~8 KB for the default 1024-slot pool) at
  engine startup. This is pre-allocated, consistent with the existing pools.

## Alternatives considered

### Drop `Box<dyn Module>` into a `std::sync::mpsc` channel

`std::sync::mpsc::Sender::send` may allocate on the sending side (the channel grows
unboundedly). Rejected: potential allocation on the audio thread.

### Use `crossbeam-channel` with a bounded channel

Equivalent to `rtrb` for this use case, but introduces a new dependency. The project
already depends on `rtrb` for the plan and signal channels; reuse is preferred.

### Return tombstoned modules to the control thread via a reverse ring buffer

The control thread would drain tombstoned modules and drop them. This has identical
real-time properties but complicates `SoundEngine::swap_plan` (the caller would need to
drain the reverse channel before or after each swap). A dedicated cleanup thread is simpler
to use and tests more cleanly.

### Defer all drops to engine shutdown

Accumulate tombstoned modules in a `Vec` inside the audio callback and drop them when the
stream is destroyed. Rejected: the `Vec` would grow unboundedly and require reallocations
on the audio thread as patches are reloaded repeatedly.
