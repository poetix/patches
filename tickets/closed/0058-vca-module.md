---
id: "0058"
title: VCA module
priority: medium
epic: "E012"
created: 2026-03-03
---

## Summary

Add a `Vca` (voltage-controlled amplifier) module to `patches-modules`. Multiplies an
audio signal by a control voltage. Stateless: no `initialise` override, no fields
beyond `instance_id` and `descriptor`.

## Acceptance criteria

- [ ] `Vca::new() -> Self` compiles.
- [ ] Two input ports: `in/0` (signal), `cv/1` (control voltage).
- [ ] One output port: `out/0`.
- [ ] `process`: `outputs[0] = inputs[0] * inputs[1]`.
- [ ] Exported from `patches-modules::lib`.
- [ ] Unit test: multiplying a known signal by a known cv gives the expected product.
- [ ] `cargo clippy` and `cargo test -p patches-modules` clean.

## Notes

No clamping on the CV input. Amplification above 1.0 and phase inversion with negative
CV are valid use cases. Keep the implementation minimal.
