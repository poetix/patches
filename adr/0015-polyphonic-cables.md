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

A cable carries either a single `f64` (mono) or a fixed array of up to 16 `f64`
values (poly). At the storage level this is an inline enum, not a trait object, to
avoid heap allocation and pointer chasing:

```rust
pub enum CableValue {
    Mono(f64),
    Poly([f64; 16]),
}
```

The buffer pool becomes `Vec<[CableValue; 2]>` — the same ping-pong pair structure
as today, but each slot now holds a `CableValue` rather than a bare `f64`. The
`[CableValue; 2]` size (≈272 bytes) is larger than the current `[f64; 2]` (16
bytes), but cable count is small relative to sample count and the total pool fits
comfortably in L1/L2.

### Port objects replace input/output slices

`Module::process` currently receives pre-gathered `&[f64]` inputs and writes to
`&mut [f64]` outputs. Under this design, modules receive pre-built port objects
and the current pool slices:

```rust
fn process(
    &mut self,
    inputs: &[InputPort],
    pool_read: &[CableValue],
    outputs: &[OutputPort],
    pool_write: &mut [CableValue],
);
```

`InputPort` and `OutputPort` contain only indices and are built once at plan-build
time by `build_slots`, stored in `ModuleSlot`, and passed unchanged on every tick.
No allocation occurs on the audio thread. The pool slices are the only call-site
arguments that change between ticks (as `wi`/`ri` flip). Inside `process()` a
module reads via `input.read(pool_read)` and writes via `output.write(pool_write,
value)`.

`InputPort` encapsulates a cable index and the per-connection scale factor that
today lives in `ModuleSlot::scaled_inputs`:

```rust
pub struct InputPort {
    cable_idx: usize,  // index into the read half of the pool
    scale: f64,        // 1.0 for unscaled connections
}
```

Modules never see `CableValue` directly. `InputPort` exposes typed read methods
that return plain `f64` or `[f64; 16]`:

```rust
impl InputPort {
    /// For ports declared `CableKind::Mono`. Panics in debug if the cable is poly
    /// (which graph validation makes unreachable in release builds).
    pub fn read_mono(&self, pool: &[CableValue]) -> f64 {
        let CableValue::Mono(v) = &pool[self.cable_idx] else { unreachable!() };
        v * self.scale
    }

    /// For ports declared `CableKind::Poly`. Returns a stack-allocated [f64; 16];
    /// no heap allocation. `std::array::from_fn` constructs the array in place.
    pub fn read_poly(&self, pool: &[CableValue]) -> [f64; 16] {
        let CableValue::Poly(vs) = &pool[self.cable_idx] else { unreachable!() };
        std::array::from_fn(|i| vs[i] * self.scale)
    }
}
```

Because graph validation has already confirmed the cable type matches the port
declaration, the `unreachable!()` arm is dead code; the compiler eliminates the
branch. There is no double dispatch: the module calls `read_mono` or `read_poly`
knowing exactly what it will receive.

For poly cables, scale is a uniform gain across all 16 channels — one multiply per
voice, a loop the compiler can auto-vectorize. `[f64; 16]` is returned by value;
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

`OutputPort::write(value: CableValue)` writes to the write-half of the pool
(`wi`). Fan-out (one output connected to multiple inputs) is free in the read
direction: multiple `InputPort`s pointing at the same cable index all read the
same slot.

Borrow-checker safety is maintained by indexing: `InputPort` and `OutputPort`
hold cable pool indices, not references. The read-half and write-half are
disjoint slices of the same backing `Vec` (split by the ping-pong parity), so
both `&pool_r` and `&mut pool_w` can be held simultaneously without aliasing.

### Migration shim for existing modules

To avoid rewriting all modules at once, `Module` provides a blanket default for
the new signature that pre-reads all inputs into a `[f64]` scratch buffer and
post-writes from `[f64]` outputs. Modules using the old `&[f64]` / `&mut [f64]`
convention continue to work correctly on mono-only signals. They receive zero for
channels beyond channel 0 on a poly input and their output is emitted as mono.

Poly-aware modules override the new signature directly.

### Poly-aware module convention

A module that supports polyphony reads the channel count from its first relevant
input:

```rust
fn process(&mut self, inputs: &[InputPort<'_>], outputs: &[OutputPort<'_>]) {
    match inputs[VOCT].read() {
        CableValue::Mono(v) => { /* single-voice path */ }
        CableValue::Poly(vs) => {
            for (ch, &v) in vs.iter().enumerate() {
                // per-voice arithmetic — tight loop, compiler-vectorizable
            }
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

A connection with `poly: N` creates a `Poly([f64; 16])` cable (zero-padded for
voices not yet active). Connections without `poly` create `Mono` cables. The
planner allocates the buffer pool slot for the declared cable type.

## Consequences

### Benefits

- **Tick-loop overhead stays O(modules), not O(modules × voices).** A 16-voice
  polyphonic patch has the same number of module slots as its mono equivalent.
  Tick overhead (currently ~36% of audio CPU per the March 2026 profiling run)
  does not grow with voice count.

- **Per-voice arithmetic is auto-vectorizable.** Tight loops over `[f64; 16]`
  arrays — sine table lookups, phase advance, envelope ramping — are the kind
  of code that LLVM auto-vectorizes using NEON on ARM without source-level SIMD
  intrinsics.

- **Mono and poly coexist in the same graph.** CV signals (LFO rate, global
  reverb send) remain mono. Only connections that carry per-voice data need poly
  cables. The DSL author opts in per-connection.

- **Scaling-in-read eliminates the gather phase.** `InputPort::read_scaled()`
  replaces the pre-gather loop in the tick body. The scatter phase (writing module
  outputs to the pool) is unchanged.

- **`ModuleShape::channels` is unaffected.** It continues to parameterise module
  structure (variable-arity modules such as mix buses) and is independent of
  polyphony.

### Costs

- **`Module::process` signature changes.** This is a breaking change to the core
  trait. The migration shim reduces blast radius, but poly-aware modules must be
  rewritten.

- **Buffer pool slots are larger.** `[CableValue; 2]` ≈ 272 bytes vs `[f64; 2]`
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

### Pre-gather into `[f64; 16]` scratch per input port

Keep the existing gather-before / scatter-after structure but expand scratch
buffers to hold 16-channel data. Avoids changing the `process()` signature.
However, it retains the gather pass as a separate loop (today's 36% tick overhead)
and prevents modules from choosing their path based on the actual cable type.
Rejected.
