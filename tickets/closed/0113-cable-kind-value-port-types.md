---
id: "0113"
title: "`CableKind`, `CableValue`, `MonoInput`/`PolyInput`/`MonoOutput`/`PolyOutput`, `InputPort`/`OutputPort` enums"
priority: high
epic: "E022"
depends_on: []
created: 2026-03-11
---

## Summary

Introduce the foundational polyphonic cable types in `patches-core`. A port's
arity (mono or poly) is fixed at declaration time and never changes over the
lifetime of a module instance; connectedness varies as the patch is edited.
These are separate concerns: the enum discriminant encodes arity, and a
`connected` field on the concrete type encodes connectedness.

## Acceptance criteria

- [ ] `CableKind` enum exists in `patches-core`:
      ```rust
      pub enum CableKind { Mono, Poly }
      ```
- [ ] `CableValue` enum exists in `patches-core` with no heap allocation:
      ```rust
      pub enum CableValue { Mono(f32), Poly([f32; 16]) }
      ```
- [ ] Four concrete port structs exist:
      ```rust
      pub struct MonoInput  { pub cable_idx: usize, pub scale: f32, pub connected: bool }
      pub struct PolyInput  { pub cable_idx: usize, pub scale: f32, pub connected: bool }
      pub struct MonoOutput { pub cable_idx: usize, pub connected: bool }
      pub struct PolyOutput { pub cable_idx: usize, pub connected: bool }
      ```
- [ ] `MonoInput` provides:
      - `is_connected(&self) -> bool` — returns `self.connected`.
      - `read(&self, pool: &[CableValue]) -> f32` — indexes the pool at
        `self.cable_idx`, extracts the `CableValue::Mono` inner value, and
        applies `self.scale`. Panics with `unreachable!()` in debug if the slot
        is `CableValue::Poly` (graph validation makes this unreachable in
        well-formed graphs).
- [ ] `PolyInput` provides:
      - `is_connected(&self) -> bool`.
      - `read(&self, pool: &[CableValue]) -> [f32; 16]` — indexes the pool at
        `self.cable_idx`, extracts the `CableValue::Poly` inner array, and
        applies `self.scale` to each channel. Returns by value (`[f32; 16]` on
        the caller's stack; no heap allocation). Panics with `unreachable!()`
        in debug if the slot is `CableValue::Mono`.
- [ ] `MonoOutput` provides:
      - `is_connected(&self) -> bool`.
      - `write(&self, pool: &mut [CableValue], value: f32)` — writes
        `CableValue::Mono(value)` into the pool at `self.cable_idx`.
- [ ] `PolyOutput` provides:
      - `is_connected(&self) -> bool`.
      - `write(&self, pool: &mut [CableValue], value: [f32; 16])` — writes
        `CableValue::Poly(value)` into the pool at `self.cable_idx`.
- [ ] Two enum wrappers exist for use in `set_ports` delivery:
      ```rust
      pub enum InputPort  { Mono(MonoInput),  Poly(PolyInput)  }
      pub enum OutputPort { Mono(MonoOutput), Poly(PolyOutput) }
      ```
      These carry no additional data; they exist solely so the planner can
      hand a heterogeneous slice to `set_ports` without boxing.
- [ ] All six types derive `Clone` and `Debug`. `MonoInput`, `PolyInput`,
      `MonoOutput`, and `PolyOutput` also derive `Default` (with
      `cable_idx: 0`, `scale: 1.0`, `connected: false` as appropriate).
- [ ] Unit tests cover:
      - `MonoInput::read` with `connected: true`, with and without `scale ≠ 1.0`.
      - `PolyInput::read` with `connected: true`; scale applied to all 16 channels.
      - `is_connected()` returns `false` when `connected: false` and `true` when
        `connected: true`, for all four concrete types.
      - `MonoOutput::write` and `PolyOutput::write` round-trip their values
        through a pool slot.
- [ ] `cargo clippy` and `cargo test -p patches-core` clean.
- [ ] No `unwrap()` or `expect()` in library code.

## Notes

Because module fields are typed as `MonoInput`, `PolyInput`, etc. (not as the
`InputPort` enum), a module's `set_ports` implementation casts the delivered
enum value to the expected concrete type:

```rust
fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
    if let InputPort::Mono(p) = inputs[VOCT_IN] { self.voct_in = p; }
    // debug_assert! on the else arm — dead code after graph validation
}
```

The `if let` always matches in a validated graph; the else arm is dead code and
optionally carries a `debug_assert!(false, "...")`.

`PolyInput::read` returns `[f32; 16]` by value. On ARM64, 16 doubles fit in 8
NEON registers; whether the compiler keeps them register-resident depends on the
surrounding code and is not assumed here.
