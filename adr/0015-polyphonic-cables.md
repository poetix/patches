# ADR 0015 — Polyphonic cables

**Date:** 2026-03-07
**Status:** Proposed

## Context

The current execution model processes one sample per module per tick. A polyphonic
patch (e.g. 16-note chords) would require 16 independent module graphs — one per
voice — causing the tick loop to iterate 16× more slots, performing 16× more
buffer gather/scatter operations and 16× more virtual dispatches. At 16 voices,
the tick overhead alone is projected to consume ~50% of the audio callback deadline
for a typical patch, with no headroom to spare.

### How VoltageModular and VCV Rack solve this

These systems use *polyphonic cables*: a single cable carries N channels
simultaneously (N ≤ 16). A polyphonic oscillator receives all N V/oct values in
one `process()` call and produces all N outputs. The signal graph remains the same
size regardless of voice count. Per-voice arithmetic is a tight inner loop within
each `process()` call — the kind that compilers auto-vectorize over contiguous
float data.

This keeps tick-loop overhead O(modules) rather than O(modules × voices), and
concentrates the voice-count scaling inside module implementations where SIMD can
address it.

### Why `ModuleShape::channels` is not the right hook

`ModuleShape::channels` parameterises the *structure* of a module — for example,
the number of mixing channels in a configurable mix bus, or the tap count in a
variable-length delay. It controls how many ports the module declares at build
time. This is unrelated to polyphony: a 4-channel mix bus is a 4-channel mix bus
regardless of whether its signals are mono or poly, and a polyphonic oscillator
does not change the number of ports it exposes based on voice count. The channels
parameter remains useful for its current purpose.

Polyphony is a property of *cable connections*, not of module structure.

## Decision

### Port and cable types

Each port in `ModuleDescriptor` declares whether it carries a mono or poly signal:

```rust
pub enum CableKind { Mono, Poly }

pub struct PortDescriptor {
    pub name: &'static str,
    pub index: usize,
    pub kind: CableKind,
}
```

The graph validator (run at plan-build time, off the audio thread) rejects any
connection where the source and destination ports have different `CableKind`s. This
means the cable type of every connection is statically known and correct by the
time any module runs; no runtime type coercion or broadcast is ever needed.

### Cable storage

A cable carries either a single `f32` (mono) or a fixed array of up to 16 `f32`
values (poly). At the storage level this is an inline enum, not a trait object, to
avoid heap allocation and pointer chasing:

```rust
pub enum CableValue {
    Mono(f32),
    Poly([f32; 16]),
}
```

The buffer pool becomes `Vec<[CableValue; 2]>` — the same ping-pong pair structure
as today, but each slot now holds a `CableValue` rather than a bare `f32`. The
`[CableValue; 2]` size (≈272 bytes) is larger than the current `[f32; 2]` (16
bytes), but cable count is small relative to sample count and the total pool fits
comfortably in L1/L2.

### Port objects stored on the module

`Module::process` currently receives pre-gathered `&[f32]` inputs and writes to
`&mut [f32]` outputs. Under this design the signature is reduced to just the pool
slices:

```rust
fn process(
    &mut self,
    pool_read: &[CableValue],
    pool_write: &mut [CableValue],
);
```

`InputPort` and `OutputPort` are built once at plan-build time and broadcast to
each module via a new trait method called at plan-accept, on the same cadence as
parameter updates and connectivity notifications:

```rust
fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]);
```

This call supersedes the existing `set_connectivity` callback: connectivity state
is implicit in `InputPort::is_connected()` and `OutputPort::is_connected()`, so no
separate connectivity notification is needed.

`InputPort` encapsulates a cable index, the per-connection scale factor, and a
connected flag:

```rust
pub struct InputPort {
    cable_idx: usize,  // index into the read half of the pool
    scale: f32,        // 1.0 for unscaled connections
    connected: bool,
}

impl InputPort {
    pub fn is_connected(&self) -> bool { self.connected }

    /// For ports declared `CableKind::Mono`. Panics in debug if the cable is poly
    /// (which graph validation makes unreachable in release builds).
    pub fn read_mono(&self, pool: &[CableValue]) -> f32 {
        let CableValue::Mono(v) = &pool[self.cable_idx] else { unreachable!() };
        v * self.scale
    }

    /// For ports declared `CableKind::Poly`. Returns a stack-allocated [f32; 16];
    /// no heap allocation. `std::array::from_fn` constructs the array in place.
    pub fn read_poly(&self, pool: &[CableValue]) -> [f32; 16] {
        let CableValue::Poly(vs) = &pool[self.cable_idx] else { unreachable!() };
        std::array::from_fn(|i| vs[i] * self.scale)
    }
}

pub struct OutputPort {
    cable_idx: usize,
    connected: bool,
}

impl OutputPort {
    pub fn is_connected(&self) -> bool { self.connected }

    pub fn write(&self, pool: &mut [CableValue], value: CableValue) {
        pool[self.cable_idx] = value;
    }
}
```

Because graph validation has already confirmed the cable type matches the port
declaration, the `unreachable!()` arm is dead code; the compiler eliminates the
branch. There is no double dispatch: the module calls `read_mono` or `read_poly`
knowing exactly what it will receive.

Module authors store port objects as named fields in their module struct. There is
no requirement to index into a slice by a constant — the module's own field names
serve as port identifiers:

```rust
struct MyOscillator {
    voct_in: InputPort,
    fm_in: InputPort,
    audio_out: OutputPort,
    // ... DSP state
}

impl Module for MyOscillator {
    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.voct_in  = inputs[VOCT_IN];
        self.fm_in    = inputs[FM_IN];
        self.audio_out = outputs[AUDIO_OUT];
    }

    fn process(&mut self, pool_read: &[CableValue], pool_write: &mut [CableValue]) {
        if !self.voct_in.is_connected() { return; }
        let v = self.voct_in.read_mono(pool_read);
        // ...
        self.audio_out.write(pool_write, CableValue::Mono(sample));
    }
}
```

`set_ports` is called once per plan refresh before any ticks run under the new
plan. This is the same ordering constraint that already governs parameter updates
and (formerly) connectivity notifications — it is an existing solved problem, not
a new class of risk. The planner's plan-accept sequence is:

1. Accept new plan from ring buffer.
2. Apply parameter updates (`set_parameter` calls).
3. Broadcast port objects (`set_ports` calls).
4. Begin ticking under the new plan.

For poly cables, scale is a uniform gain across all 16 channels — one multiply per
voice, a loop the compiler can auto-vectorize. `[f32; 16]` is returned by value;
the 128 bytes of storage come from the caller's stack frame (sret convention), not
the heap. On ARM64, 16 doubles fit in 8 NEON registers; if the result feeds
directly into a tight inner loop the compiler may keep the values in registers
entirely and never write them to the stack at all. Whether this happens for a given
module is visible in the disassembly and cannot be assumed in general.

Unscaled connections (scale = 1.0) still pass through the multiply. The compiler
can constant-fold this away when the scale is statically known; in the general case
it is one multiply per input per `process()` call — far cheaper than the
per-sample-per-input cost the current `scaled_inputs` / `unscaled_inputs`
segregation was introduced to eliminate. If profiling later shows this matters, the
planner can emit two concrete `InputPort` types so unscaled ports skip the multiply
entirely.

Fan-out (one output connected to multiple inputs) is free in the read direction:
multiple `InputPort`s pointing at the same cable index all read the same slot.

Borrow-checker safety is maintained by indexing: `InputPort` and `OutputPort`
hold cable pool indices, not references. The read-half and write-half are
disjoint slices of the same backing `Vec` (split by the ping-pong parity), so
both `&pool_r` and `&mut pool_w` can be held simultaneously without aliasing.

### Migration shim for existing modules

To avoid rewriting all modules at once, a `MonoShim<M>` wrapper type is provided
that stores port objects in internal `Vec<InputPort>` / `Vec<OutputPort>` fields,
implements `set_ports`, and provides an adapter `process()` that pre-reads all
connected mono inputs into a `[f32]` scratch buffer and calls the old
`process_mono(&[f32], &mut [f32])` signature. Modules using the old convention
continue to work correctly on mono-only signals. They receive zero for channels
beyond channel 0 on a poly input and their output is emitted as mono.

Poly-aware modules implement `set_ports` and `process` directly.

### Poly-aware module convention

A module that supports polyphony reads the channel count from its first relevant
input:

```rust
fn process(&mut self, pool_read: &[CableValue], pool_write: &mut [CableValue]) {
    match self.voct_in.read_poly(pool_read) {
        // read_poly returns [f32; 16]; mono case is handled via the mono path
    }
    // or, for a module that handles both:
    if self.voct_in.is_connected() {
        let vs: [f32; 16] = self.voct_in.read_poly(pool_read);
        for (ch, v) in vs.iter().enumerate() {
            // per-voice arithmetic — tight loop, compiler-vectorizable
        }
    }
}
```

The voice count is implicit in the data, not a separate parameter. A module that
receives a mono cable on a poly-capable input runs its single-voice path; one that
receives a poly cable runs the multi-voice path. This mirrors the VCV Rack
convention.

### DSL changes

Cable type is declared at connection time in the DSL:

```yaml
cables:
  - from: sequencer.voct   to: oscillator.voct   poly: 16
  - from: oscillator.out   to: filter.in
```

A connection with `poly: N` creates a `Poly([f32; 16])` cable (zero-padded for
voices not yet active). Connections without `poly` create `Mono` cables. The
planner allocates the buffer pool slot for the declared cable type.

## Consequences

### Benefits

- **Tick-loop overhead stays O(modules), not O(modules × voices).** A 16-voice
  polyphonic patch has the same number of module slots as its mono equivalent.
  Tick overhead (currently ~36% of audio CPU per the March 2026 profiling run)
  does not grow with voice count.

- **Per-voice arithmetic is auto-vectorizable.** Tight loops over `[f32; 16]`
  arrays — sine table lookups, phase advance, envelope ramping — are the kind
  of code that LLVM auto-vectorizes using NEON on ARM without source-level SIMD
  intrinsics.

- **Mono and poly coexist in the same graph.** CV signals (LFO rate, global
  reverb send) remain mono. Only connections that carry per-voice data need poly
  cables. The DSL author opts in per-connection.

- **Scaling-in-read eliminates the gather phase.** `InputPort::read_mono/poly()`
  replaces the pre-gather loop in the tick body. The scatter phase (writing module
  outputs to the pool) is unchanged.

- **`process()` signature is minimal.** Only the pool slices — the only
  call-site data that changes between ticks — are passed as arguments. Port
  objects are part of the module's own state, accessed via `self`.

- **`is_connected()` is first-class.** Modules can cheaply skip computation when
  an input has no cable without maintaining separate connectivity state.

- **Named port fields are self-documenting.** Module struct definitions directly
  reflect I/O topology; no parallel array of index constants is needed.

- **`set_connectivity` is subsumed.** Port objects carry connectivity state;
  the separate connectivity notification callback is no longer needed.

- **`ModuleShape::channels` is unaffected.** It continues to parameterise module
  structure (variable-arity modules such as mix buses) and is independent of
  polyphony.

### Costs

- **`Module::process` and trait signatures change.** This is a breaking change to
  the core trait. The `MonoShim` wrapper reduces blast radius, but poly-aware
  modules must be rewritten.

- **Buffer pool slots are larger.** `[CableValue; 2]` ≈ 272 bytes vs `[f32; 2]`
  = 16 bytes. For a 64-cable graph, the pool grows from 1 KB to ~17 KB — still
  L1-resident, but a 17× increase. Mixed mono/poly graphs can use a two-tier pool
  (separate `Vec` for mono and poly slots) if this becomes a concern.

- **Parallel execution requires pool partitioning.** When modules are eventually
  split across threads, multiple threads will simultaneously call
  `OutputPort::write()`, requiring `&mut` access to the buffer pool. There are no
  actual write conflicts — each cable has exactly one writer — but Rust's borrow
  rules do not permit multiple `&mut` references into the same `Vec` simultaneously,
  even with provably disjoint index ranges. The resolution is to partition the pool
  by output ownership at plan-build time: if each module's output cables occupy a
  contiguous range, the planner can distribute disjoint `&mut` sub-slices via
  `split_at_mut` without `unsafe`. `split_at_mut` on a `&mut [T]` yields two
  non-overlapping `&mut [T]` sub-slices that Rust statically verifies do not alias;
  extending this with repeated splits or `chunks_mut` gives N disjoint slices for N
  modules. Each slice can be moved into a separate thread. `OutputPort` holds a
  relative offset within the module's assigned slice rather than an absolute pool
  index.

  The read side requires no partitioning. During any given tick all reads come from
  the `ri` (read) half of the ping-pong buffer — values written by the *previous*
  tick, which are fully stable for the entire duration of the current tick. Every
  parallel thread holds a shared `&[CableValue]` reference to the whole read half
  simultaneously; shared immutable references never conflict. A module in one thread
  reading a cable whose *write* slot belongs to a module in a different thread has
  no coordination problem, because it is reading last tick's frozen value, not this
  tick's in-progress write.

  The 1-sample cable delay — introduced to free the engine from topological ordering
  constraints — is therefore also precisely the mechanism that makes lock-free
  parallel writes safe. Within a single tick, reads and writes always target
  different halves of the ping-pong buffer and can never alias.

  This partitioning is a contained change to Phase 6 of `build_slots` (cable index
  assignment) with no impact on the `Module` trait, cable semantics, or read-side
  `InputPort` behaviour. It is deferred until parallel execution is planned. Padding
  slice boundaries to cache lines at that point would also eliminate false sharing
  between threads, as noted in the design desiderata.

  Port objects stored on the module do not complicate this picture. In the parallel
  case, each module's `process()` is called by one thread; `self` is exclusively
  owned by that thread for the duration of the call. `set_ports` is called during
  plan-accept, which is a sequential single-thread operation before any parallel
  tick begins. There is no new synchronisation requirement.

- **`read_mono`/`read_poly` contain an `unreachable!()` arm.** Because graph
  validation guarantees cable types are correct before any module runs, the
  wrong-type arm in each read method is dead code. The compiler eliminates it in
  release builds; in debug builds it becomes a useful assertion. No branch cost in
  practice.

- **`CableValue::Poly` always allocates 16 slots** regardless of active voice
  count. Voice management (allocating voices to channels, handling note-off) is an
  application-level concern above the engine.

## Alternatives considered

### 16 independent module graphs (one per voice)

The naive polyphony approach. Tick-loop overhead and dispatch cost scale linearly
with voice count. At 16 voices, projected to consume ~50% of the audio deadline
for a typical patch. Rejected as the primary mechanism; may still be appropriate
for patches that use effects modules with no poly-capable implementation.

### Poly as a module property via `ModuleShape::channels`

`channels` already exists and is passed to `describe()` and `prepare()`. It was
originally anticipated as a potential hook for polyphony. However, polyphony is a
cable property (how many channels of data flow between two ports), not a module
structure property (how many ports a module exposes). A poly cable between two
mono-structured modules is coherent; a module with 16 declared channels but only
mono cables is also coherent. Conflating the two would force the module's port
count to scale with polyphony, creating an awkward descriptor (16 `voct_0`…
`voct_15` inputs instead of one `voct` carrying 16 channels). Rejected.

### `Box<dyn Cable>` trait objects

Using a trait object for `CableValue` allows arbitrary channel counts and avoids
committing to a fixed maximum. The cost is a heap allocation per cable and a
vtable dispatch per `read()`. For audio, where cable count is small but read count
is 48 000 per second per cable, the per-read vtable cost is unacceptable.
Rejected in favour of the inline enum.

### Pre-gather into `[f32; 16]` scratch per input port

Keep the existing gather-before / scatter-after structure but expand scratch
buffers to hold 16-channel data. Avoids changing the `process()` signature.
However, it retains the gather pass as a separate loop (today's 36% tick overhead)
and prevents modules from choosing their path based on the actual cable type.
Rejected.

### Pass port objects as arguments to `process()`

An earlier version of this ADR passed port objects as slices to `process()` on
every tick:

```rust
fn process(
    &mut self,
    inputs: &[InputPort],
    pool_read: &[CableValue],
    outputs: &[OutputPort],
    pool_write: &mut [CableValue],
);
```

`InputPort` and `OutputPort` would be stored in `ModuleSlot` and passed unchanged
on each call (only the pool slices change between ticks as `wi`/`ri` flip).

This was rejected in favour of storing ports on the module for the following
reasons:

- **`process()` arguments that never change are noise.** Port objects are stable
  between plan refreshes. Passing them on every tick conveys no new information
  and adds four parameters to the hottest call in the system.
- **Modules access ports by index constant, not by name.** Accessing
  `inputs[VOCT_IN]` requires a separately maintained index constant; storing
  `self.voct_in` requires none. The struct definition becomes the port manifest.
- **`is_connected()` has no natural representation.** An unconnected port either
  must be represented by a sentinel `InputPort` in the slice or by a shorter slice
  with a separate index mapping. Both are more awkward than a `connected: bool`
  field on a named port struct.
- **The broadcast-at-plan-accept pattern already exists.** Parameter updates and
  (formerly) connectivity notifications are already delivered to modules at
  plan-accept time. Delivering port objects on the same cadence is a natural
  extension — it is not a new category of synchronisation requirement.

### `is_connected()` flag on passed-in `InputPort` (hybrid)

A lighter variant of the above: keep port objects as `process()` arguments but add
a `connected: bool` field to `InputPort` so modules can call `is_connected()`
without storing separate connectivity state. This was considered as a way to
capture the ergonomic benefit of `is_connected()` without moving port state into
the module. It is strictly dominated by the chosen design: it retains the
unchanged-arguments-on-every-tick cost and the index-constant boilerplate, while
only partially addressing the ergonomic gap. Rejected.
