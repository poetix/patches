---
id: "0014"
title: Clean up sine_tone example error handling
priority: low
created: 2026-02-28
depends_on: ["0010"]
epic: "E002"
---

## Summary

The `sine_tone` example uses `.unwrap()` for graph connections and `process::exit(1)` inside a `map_err` to work around the `BuildError`/`EngineError` type mismatch. Replace with `Box<dyn Error>` as the return type so all error types coexist cleanly, and remove all `.unwrap()` calls.

## Acceptance criteria

- [ ] `run()` returns `Result<(), Box<dyn std::error::Error>>`
- [ ] All `.unwrap()` calls in the example replaced with `?`
- [ ] No `process::exit` calls inside `map_err` closures
- [ ] `cargo build --example sine_tone` succeeds
- [ ] `cargo clippy` is clean

## Notes

**Depends on 0010** because 0010 ensures `BuildError` properly implements `std::error::Error` with any new variants, making `Box<dyn Error>` propagation clean.

This is example code, not library code, so `.unwrap()` is technically allowed by convention. The cleanup is still worthwhile because the example serves as a reference for users building patches.
