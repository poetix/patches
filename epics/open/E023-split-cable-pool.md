---
id: "E023"
title: CablePool wrapper — hide ping-pong indexing from modules
created: 2026-03-12
tickets: ["0121", "0122", "0123", "0124"]
---

## Summary

The current `Module::process` signature exposes the ping-pong double-buffering
mechanism directly to module authors:

```rust
fn process(&mut self, pool: &mut [[CableValue; 2]], wi: usize);
```

Modules must compute `ri = 1 - wi` and thread both indexes through every port
accessor call. This is an internal engine concern that leaks into every module
implementation.

The fix is a `CablePool<'a>` wrapper that owns the raw pool reference and the
`wi` index, and exposes typed read/write methods that accept port objects
directly:

```rust
pub struct CablePool<'a> {
    pool: &'a mut [[CableValue; 2]],
    wi: usize,
}

impl<'a> CablePool<'a> {
    pub fn read_mono(&self, input: &MonoInput) -> f32;
    pub fn read_poly(&self, input: &PolyInput) -> [f32; 16];
    pub fn write_mono(&mut self, output: &MonoOutput, value: f32);
    pub fn write_poly(&mut self, output: &PolyOutput, value: [f32; 16]);
}

fn process(&mut self, pool: &mut CablePool<'_>);
```

No buffer layout change is required. `wi` and `1 - wi` become a private
implementation detail of `CablePool`.

As a consequence, the read/write methods currently on `MonoInput`, `PolyInput`,
`MonoOutput`, and `PolyOutput` (`read_from`, `write_to`) are removed.
Port objects become plain index+metadata structs; `CablePool` is the only
place reads and writes occur.

## Tickets

| ID   | Title                                                                         | Priority | Depends on |
|------|-------------------------------------------------------------------------------|----------|------------|
| 0121 | Add CablePool, update Module::process trait, remove port accessor methods     | high     | —          |
| 0122 | Wire CablePool into ExecutionPlan::tick and ModulePool::process               | high     | 0121       |
| 0123 | Migrate patches-modules implementations to CablePool API                      | high     | 0121       |
| 0124 | Update HeadlessEngine, test stubs; verify clean build                         | high     | 0122, 0123 |

## Definition of done

- `Module::process` takes `pool: &mut CablePool<'_>`.
- No module implementation references `wi`, `ri`, or `1 - wi`.
- `MonoInput`, `PolyInput`, `MonoOutput`, `PolyOutput` have no `read_from` /
  `write_to` methods.
- `cargo build`, `cargo test`, `cargo clippy` clean across all crates.
- No `unwrap()` or `expect()` in library code.
