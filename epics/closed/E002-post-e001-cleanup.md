---
id: "E002"
title: Post-E001 cleanup and hardening
created: 2026-02-28
tickets: ["0009", "0010", "0011", "0012", "0013", "0014", "0015", "0016"]
---

## Summary

A code review of the E001 deliverables identified improvements across real-time safety, API clarity, code conventions, flexibility, and documentation. This epic addresses all of them: replacing the Mutex in the audio callback with a lock-free handoff, eliminating `.unwrap()` from library code, tightening the type system around port descriptors and sink modules, and cleaning up minor rough edges throughout the codebase.

No new features or modules are added. The system's observable behaviour is unchanged — the same patches produce the same audio.

## Acceptance criteria

- [x] All eight tickets closed
- [x] `cargo build` and `cargo clippy` are clean across the workspace
- [x] `cargo test` passes across the workspace
- [x] Zero `.unwrap()` or `.expect()` calls in non-test library code
- [x] `SoundEngine` audio callback uses no `Mutex`
- [x] `cargo run --example sine_tone` still plays audio correctly

## Tickets

| ID | Title | Priority |
|----|-------|----------|
| [0009](../../tickets/closed/0009-lock-free-engine.md) | Replace Mutex with triple-buffer in SoundEngine | high |
| [0010](../../tickets/closed/0010-eliminate-unwrap.md) | Eliminate `.unwrap()` from library code | high |
| [0011](../../tickets/closed/0011-static-port-names.md) | Use `&'static str` port names and return `&ModuleDescriptor` | medium |
| [0012](../../tickets/closed/0012-remove-port-direction.md) | Remove redundant `PortDirection` from `PortDescriptor` | low |
| [0013](../../tickets/closed/0013-sink-trait.md) | Introduce `Sink` trait to decouple engine from `AudioOut` | medium |
| [0014](../../tickets/closed/0014-example-error-handling.md) | Clean up `sine_tone` example error handling | low |
| [0015](../../tickets/closed/0015-misc-improvements.md) | Miscellaneous small improvements | low |
| [0016](../../tickets/closed/0016-update-documentation.md) | Update documentation and fix broken links | low |

## Dependency graph

```
0009 (Mutex → triple-buffer)          0016 (docs)
0010 (unwrap removal) → 0014 (example errors)
0011 (static port names) → 0012 (remove PortDirection)
0011 → 0013 (Sink trait)
0011, 0012, 0013 → 0015 (misc improvements)
```

## Notes

**No new crates.** All changes are within existing `patches-core`, `patches-modules`, and `patches-engine` crates.

**One new dependency:** `triple_buffer` is added to `patches-engine` for ticket 0009.

**Ordering flexibility:** Tickets 0009, 0010, 0011, and 0016 have no dependencies on each other and can be worked in any order. The remainder follow the dependency graph above.
