# ADR 0013 — Port connectivity notification

**Date:** 2026-03-05
**Status:** Proposed

## Context

Modules currently have no way to know which of their ports are connected in the
active patch graph. Every call to `process()` receives a full `inputs` slice, but
a zero value in an unconnected slot is indistinguishable from a connected-but-silent
signal. This creates two concrete problems:

### Expensive per-sample recalculation

Modules such as filters recompute internal state (e.g. biquad coefficients) every
sample when a modulation input is present. If the modulation input is not connected
— the common case in a static patch — this work is wasted. The module cannot
currently detect this condition and skip it.

### Stereo channel conventions

Stereo modules follow the convention that connecting only the left input means the
left signal should be mirrored into the right processing path (and likewise for
outputs). Without connectivity information, a module cannot distinguish between
"right channel is silent" and "right channel is not patched"; it must always process
both channels identically or make arbitrary assumptions.

### What the planner already knows

The planner has complete connectivity information at plan-build time. `build_patch`
Phase 6 (`build_slots`) iterates over every edge in the graph to resolve input and
output buffer indices. Which input and output ports have live connections is a
by-product of this work. The information simply has not been surfaced to modules.

## Decision

### `PortConnectivity` type

A new struct is added to `patches-core`:

```rust
pub struct PortConnectivity {
    pub inputs: Box<[bool]>,   // inputs[i] = true iff port i has an incoming edge
    pub outputs: Box<[bool]>,  // outputs[j] = true iff port j has at least one outgoing edge
}
```

Indexed to match the port order in `ModuleDescriptor::inputs` and
`ModuleDescriptor::outputs`. Allocated once off the audio thread during plan
building; owned by each module thereafter.

### `Module::set_connectivity`

A new method is added to the `Module` trait with a default no-op implementation:

```rust
fn set_connectivity(&mut self, _connectivity: PortConnectivity) {}
```

Modules that care about connectivity override this and store the value as internal
state for use in `process()`:

```rust
fn set_connectivity(&mut self, conn: PortConnectivity) {
    self.freq_is_modulated = conn.inputs[FREQ_PORT_INDEX];
    self.right_input_connected = conn.inputs[RIGHT_IN_PORT_INDEX];
}

fn process(&mut self, inputs: &[f32], outputs: &mut [f32]) {
    if self.freq_is_modulated {
        self.recalc_coefficients(inputs[FREQ_PORT_INDEX]);
    }
    // ...
}
```

### Computing connectivity in `build_patch`

During Phase 6 (`build_slots`), the planner computes a `PortConnectivity` for each
node by examining the edge list: an input port is connected if it appears as the
target of any edge; an output port is connected if it appears as the source of any
edge.

**New modules** — `set_connectivity` is called directly on the boxed module before
it is placed in `ExecutionPlan::new_modules`. The module arrives at the audio thread
already configured. No extra mechanism needed.

**Surviving modules** — connectivity may have changed (a cable may have been added
or removed). A new field is added to `ExecutionPlan`:

```rust
pub connectivity_updates: Vec<(usize, PortConnectivity)>,
```

Each entry is `(pool_index, new_connectivity)`. The planner emits an entry only
when the connectivity differs from the previous build, comparing against a
`connectivity: PortConnectivity` field stored in `NodeState`.

### Applying updates on plan adoption

During plan swap in the audio callback, connectivity updates are applied after new
modules are installed and parameter updates are applied:

```rust
for (idx, conn) in plan.connectivity_updates.drain(..) {
    pool.set_connectivity(idx, conn);
}
```

`ModulePool` gains a corresponding method:

```rust
pub fn set_connectivity(&mut self, idx: usize, conn: PortConnectivity) {
    if let Some(m) = self.modules[idx].as_mut() {
        m.set_connectivity(conn);
    }
}
```

### Thread safety

`set_connectivity` is called on the **control thread** for new modules (before they
are enqueued in `new_modules`) and on the **audio thread** for surviving modules
(during plan swap, inside the audio callback). Implementations must therefore be
audio-thread safe: no allocation, no blocking, no syscalls. Storing primitive
values (`bool`, `usize`) into module fields satisfies this requirement. The
`PortConnectivity` value itself moves into the module — the `Vec` inside
`connectivity_updates` is dropped after the drain, which is acceptable since drop
of a `Box<[bool]>` on the audio thread is a single deallocation and happens at most
once per plan swap.

If audio-thread deallocation of `PortConnectivity` is later identified as a
concern, the existing cleanup-channel infrastructure (ADR 0010) can be used to
defer the drop. This is not done initially.

### Change detection

`NodeState` gains a `connectivity: PortConnectivity` field. The planner compares
the newly computed `PortConnectivity` against the stored value; an update is
emitted only when they differ. This is consistent with the parameter diffing
strategy introduced in ADR 0012.

## Consequences

### Benefits

- **Modules can skip expensive per-sample work.** A filter whose modulation input
  is unconnected can skip coefficient recalculation entirely, checking a single
  stored bool.

- **Stereo conventions become implementable.** Stereo modules can reliably
  distinguish "unconnected" from "connected and silent" and implement
  left-mirrors-right behaviour correctly.

- **No audio-thread overhead in steady state.** `connectivity_updates` is empty
  between plan swaps. The per-sample `process()` path is unchanged; modules store
  the connectivity state they need as ordinary fields.

- **Follows existing patterns.** The mechanism mirrors `parameter_updates` exactly:
  computed during build, carried in the plan, applied on plan adoption, change-
  detected via `NodeState`. No new architectural concepts.

### Costs

- **`NodeState` grows.** A `PortConnectivity` value (two `Box<[bool]>` slices) is
  added to every `NodeState`. This is heap-allocated and lives on the control
  thread; no audio-thread impact in steady state.

- **`set_connectivity` is called on the audio thread for survivors.** Implementations
  must respect audio-thread constraints. The default no-op satisfies this; modules
  that override it must be written carefully. This is documented in the trait.

- **Minor additional work in `build_slots`.** The planner computes connectivity
  for every node on every build. This is O(ports + edges) and happens off the audio
  thread; cost is negligible.

## Alternatives considered

### Pass connectivity via `AudioEnvironment`

`AudioEnvironment` is graph-topology-agnostic hardware context. Mixing topology
information into it would conflate concerns and require constructing a separate
`AudioEnvironment` per node. Rejected.

### Check `inputs[i] == 0.0` in `process()`

A connected-but-silent port also produces zeros. The value in an unconnected input
scratch buffer depends on the initialisation state of that buffer; it is unreliable
as a signal. Rejected.

### Always send connectivity updates for all modules, not just changed ones

Simpler diffing but sends more data over the ring buffer on each plan swap. The
existing `parameter_updates` design uses diffing; connectivity follows the same
principle for consistency. Rejected.

### Deliver connectivity via `ControlSignal`

`ControlSignal` is the channel for external real-time control (MIDI, OSC).
Connectivity is structural plan metadata, not an external control event; it should
be atomic with plan adoption. Rejected, for the same reasons as parameter updates
(see ADR 0012 § "Alternatives considered").
