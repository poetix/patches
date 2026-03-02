# ADR 0007 — No `Module::destroy` hook

**Date:** 2026-03-02
**Status:** Accepted

## Context

T-0026 proposed adding `Module::destroy(&mut self)` as a lifecycle hook called when
a module is removed from the patch graph during a re-plan, together with a dedicated
cleanup thread in `PatchEngine` to call it. The motivation was that `Drop` runs on
whichever thread happens to hold the last reference, which could be unsafe for
resources with thread affinity (e.g. GPU contexts, thread-local FFI handles).

The proposal was implemented, reviewed, and then reverted after analysing the actual
ownership chain.

## Analysis of the ownership chain

When a re-plan removes a module, the sequence is:

1. `PatchEngine::update` calls `self.planner.build(graph, self.held_plan.take())`.
2. Inside `Planner::build`, `prev_plan.into_registry()` consumes the **held plan**
   and moves all its module instances into a `ModuleInstanceRegistry`.
3. `build_patch` claims matching modules from the registry for the new plan.
   Unmatched modules remain in the registry.
4. `Planner::build` returns; the registry goes out of scope and drops on the
   **control thread**. Unmatched (removed) modules drop here.

The key observation is step 2: the input is the *held plan*, not the audio thread's
running plan. These are distinct objects:

- The audio thread receives a plan via a single-slot lock-free channel
  (`SoundEngine::swap_plan`). Once sent, the audio thread owns that plan
  exclusively — `PatchEngine` has no reference to it.
- The held plan is either `None` (normal case, after a successful swap) or the plan
  that was rejected because the channel was full. In the latter case the audio thread
  *never ran that plan* at all.

Therefore, at step 4, no thread other than the control thread holds any reference to
the removed modules. Dropping them there is safe. The premise of T-0026 — that
removed modules "may be running on the audio thread" — was incorrect.

## Decision

Do not add `Module::destroy` or a cleanup thread. Rely on `Drop` for resource cleanup,
which Rust already guarantees runs on the control thread at the end of `Planner::build`.

## Consequences

- **API stays minimal.** The `Module` trait has no `destroy` method; implementors
  have one fewer lifecycle hook to reason about.
- **No background thread.** `PatchEngine` has no cleanup thread, no channel, and no
  join-on-stop logic.
- **`Drop` is sufficient today.** All currently implemented modules hold only
  ordinary Rust-owned resources. If a future module genuinely requires teardown on a
  specific thread (e.g. a WGPU module that must release resources on the render
  thread), that module should manage its own thread coordination internally rather
  than relying on a generic engine-level hook.
- **If thread-affine teardown is needed later**, the right fix is to introduce it at
  the point of concrete need, with a concrete thread target, rather than pre-emptively
  routing all teardown through a generic cleanup thread that is not the correct thread
  for any particular resource type.

## Alternatives considered

**Keep `destroy()` as a no-op hook for future extensibility.** Rejected: it adds
permanent API surface and documentation burden for a scenario that may never arise.
Traits should not carry methods with no present implementors.

**Call `destroy()` on the control thread without a cleanup thread.** This would be
safe (see analysis above) but is equivalent to `Drop`, so it provides no benefit over
removing the hook entirely.
