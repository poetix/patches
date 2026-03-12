---
id: "0123"
title: Migrate patches-modules implementations to CablePool API
priority: high
created: 2026-03-12
epic: E023
depends-on: "0121"
---

## Summary

Update every `Module::process` implementation in `patches-modules` to use the
new `CablePool` API. Each module drops `let ri = 1 - wi;` and replaces
`self.port.read_from(pool, ri)` / `self.port.write_to(pool, wi, v)` with
`pool.read_mono(&self.port)` / `pool.write_mono(&self.port, v)`.

## Acceptance criteria

- [ ] All modules in `patches-modules/src/` compile against the new
  `Module::process` signature.
- [ ] No module implementation references `wi`, `ri`, or `1 - wi`.
- [ ] No calls to `read_from` or `write_to` remain anywhere in `patches-modules`.
- [ ] `cargo test -p patches-modules` passes.
- [ ] `cargo clippy -p patches-modules` clean.

## Notes

Modules to update (at minimum): oscillator, sum, and any others present under
`patches-modules/src/`. Check with `grep -r "read_from\|write_to\|1 - wi"
patches-modules/` to find all call sites before starting.
