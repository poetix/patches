---
id: "E022"
title: Polyphonic cables
created: 2026-03-11
adr: "0015"
tickets: ["0113", "0114", "0115", "0116", "0117", "0118", "0119", "0120"]
---

## Summary

Introduce polyphonic cables so that a single connection in the signal graph can
carry up to 16 simultaneous voice channels. This keeps tick-loop overhead
O(modules) regardless of voice count — a 16-voice patch has the same number of
module slots as its mono equivalent. Per-voice arithmetic lives inside each
module's `process()` call as a tight inner loop that the compiler can
auto-vectorize.

The design replaces the existing gather-before / scatter-after model with port
objects stored on the module. `InputPort` and `OutputPort` are enum types with
`Mono` and `Poly` variants; the variant is determined at plan-build time and
never changes between ticks. Ports are delivered to modules at plan-accept time
via a new `set_ports` trait method, which also subsumes the existing
`set_connectivity` notification. On each tick, `process()` receives only the
pool slices — the only data that changes between ticks.

See ADR 0015 for the full design rationale, alternatives considered, and
migration strategy.

## Design overview

### Core types

`CableValue` is an inline enum (no heap allocation):

```rust
pub enum CableKind { Mono, Poly }

pub enum CableValue {
    Mono(f32),
    Poly([f32; 16]),
}
```

A port's arity (mono or poly) is fixed at declaration time and never changes.
Connectedness varies over the lifecycle of the patch. These are separate
concerns and represented separately: the enum discriminant encodes arity; a
`connected` field on the concrete type encodes connectedness.

```rust
pub struct MonoInput  { pub cable_idx: usize, pub scale: f32, pub connected: bool }
pub struct PolyInput  { pub cable_idx: usize, pub scale: f32, pub connected: bool }
pub struct MonoOutput { pub cable_idx: usize, pub connected: bool }
pub struct PolyOutput { pub cable_idx: usize, pub connected: bool }

pub enum InputPort  { Mono(MonoInput),  Poly(PolyInput)  }
pub enum OutputPort { Mono(MonoOutput), Poly(PolyOutput) }
```

`MonoInput::read(pool) -> f32` and `PolyInput::read(pool) -> [f32; 16]` have
no wrong-type arm: the type system guarantees the pool slot matches. Both
provide `is_connected()`. `MonoOutput` and `PolyOutput` each provide
`write(pool, value)` and `is_connected()`.

Module fields are typed as the concrete port type, not the enum. `set_ports`
delivers the `InputPort`/`OutputPort` enums; the module extracts the inner
value and assigns it to the correctly-typed field:

```rust
struct MyOscillator { voct_in: MonoInput, audio_out: MonoOutput, /* DSP state */ }

impl Module for MyOscillator {
    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        if let InputPort::Mono(p)  = inputs[VOCT_IN]    { self.voct_in   = p; }
        if let OutputPort::Mono(p) = outputs[AUDIO_OUT] { self.audio_out = p; }
    }
    fn process(&mut self, pool_read: &[CableValue], pool_write: &mut [CableValue]) {
        let v = self.voct_in.read(pool_read);
        self.audio_out.write(pool_write, v);
    }
}
```

Because graph validation (T-0114) guarantees the kind matches the port
declaration, the `if let` pattern in `set_ports` always matches; the else arm
is dead code and can carry a debug assertion.

### Kind enforcement at graph construction

Each port in `ModuleDescriptor` declares `kind: CableKind`. `ModuleGraph::connect()`
looks up the source and destination port descriptors and returns an error if
their `CableKind`s differ. Kind mismatches are therefore impossible in a
well-formed `ModuleGraph`; no additional planner check is needed.

### Updated Module trait

```rust
fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]);

fn process(&mut self, pool_read: &[CableValue], pool_write: &mut [CableValue]);
```

`set_connectivity` is removed; connectivity is `MonoInput::connected`, `PolyInput::connected`, etc.

### Plan-accept sequence

1. Accept new plan from ring buffer.
2. Apply parameter updates (`set_parameter` calls).
3. Broadcast port objects (`set_ports` calls).
4. Begin ticking.

### Migration shim

`MonoShim<M>` wraps legacy module implementations, implements `set_ports` /
`process` on their behalf, and calls the old mono-style logic. Existing modules
continue working correctly on mono-only signals without requiring a full rewrite.

## Tickets

| ID   | Title                                                                  | Priority | Depends on       |
|------|------------------------------------------------------------------------|----------|------------------|
| 0113 | `CableKind`, `CableValue`, `InputPort` and `OutputPort` enum types     | high     | —                |
| 0114 | `CableKind` on `PortDescriptor`; kind enforcement in `connect()`       | high     | 0113             |
| 0115 | New `Module` trait: `process(pool_read, pool_write)` + `set_ports`     | high     | 0113             |
| 0116 | Planner builds port objects; `ExecutionPlan::port_updates`             | high     | 0114, 0115       |
| 0117 | `MonoShim<M>` backward-compatible wrapper                              | medium   | 0115             |
| 0118 | Migrate all `patches-modules` modules to new `Module` trait            | medium   | 0116, 0117       |
| 0119 | DSL `poly: N` field on cable connections                               | medium   | 0114             |
| 0120 | Integration tests for polyphonic cables                                | medium   | 0116, 0118, 0119 |

## Definition of done

- `CableKind`, `CableValue`, `MonoInput`, `PolyInput`, `MonoOutput`, `PolyOutput`,
  `InputPort` (enum), and `OutputPort` (enum) defined in `patches-core` with no
  heap allocation; unit-tested.
- `PortDescriptor::kind: CableKind` exists on every port; `ModuleGraph::connect()`
  returns an error if source and destination `CableKind`s differ.
- `Module::process` signature is `(pool_read: &[CableValue], pool_write: &mut [CableValue])`.
- `Module::set_ports` has a default no-op implementation; documented as
  audio-thread-safe (no allocation, no blocking).
- `Module::set_connectivity` and `ExecutionPlan::connectivity_updates` are
  removed entirely.
- `ExecutionPlan::port_updates` carries port vecs for every module that needs a
  refresh; applied in plan-accept step 3.
- `MonoShim<M>` wraps old mono-only modules with zero changes to their DSP logic.
- All existing modules in `patches-modules` compile and pass tests under the new
  trait (via `MonoShim` or direct implementation).
- DSL accepts `poly: N` on cable connections; the planner allocates
  `CableValue::Poly` slots for those cables.
- Integration tests cover: initial port delivery, poly cable carrying 16 channels,
  `connect()` returning an error on kind mismatch, `connected` field reflecting
  correct state before and after patch edits.
- `cargo build`, `cargo test`, `cargo clippy` clean across all crates with no
  new warnings.
- No `unwrap()` or `expect()` in library code.
