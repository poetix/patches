---
id: "0018"
title: Add stable InstanceId to every module
priority: high
created: 2026-02-28
---

## Summary

To enable stateful re-planning (re-using module instances across plan rebuilds),
each module instance needs a stable, immutable identity assigned at construction
and exposed via the `Module` trait. Introduce `InstanceId(u64)` backed by a global
atomic counter. No new Cargo dependencies.

## Acceptance criteria

- [ ] `InstanceId(u64)` newtype added to `patches-core` (Copy, Eq, Hash, Debug, Display)
- [ ] Global `AtomicU64` counter in `patches-core` for ID assignment
- [ ] `Module::instance_id(&self) -> InstanceId` added to trait (required, no default)
- [ ] `SineOscillator`, `AudioOut`, and `Mix` each store and return an `InstanceId`
- [ ] Test `StubModule` in `patches-core` updated to implement `instance_id`
- [ ] Two independently constructed modules always have distinct InstanceIds
- [ ] `InstanceId` exported from `patches-core::lib`
- [ ] `cargo clippy` clean, all tests passing

## Notes

Part of epic E003. `InstanceId` is a newtype to avoid confusion with `NodeId`.
`Relaxed` ordering is sufficient — we only need uniqueness, not ordering guarantees.
