---
id: "0096"
title: Externally-assigned InstanceId — remove auto-mint from constructors
priority: high
created: 2026-03-08
epic: E019
---

## Summary

`InstanceId` is currently minted inside each module's constructor via a global
`AtomicU64` counter. This couples identity to construction, making it impossible
to know a node's `InstanceId` before instantiating it — which blocks the
builder refactor (E019) from deferring instantiation to the action phase.

This ticket moves `InstanceId` minting out of module constructors. The builder
will mint IDs at the point of instantiation and pass them in; the module stores
the given ID rather than generating its own.

## Acceptance criteria

- [ ] `InstanceId::mint()` (or equivalent) is a free function / associated
      function accessible from the builder without constructing a module.
- [ ] Each module's constructor (or `registry.create`) accepts an `InstanceId`
      parameter and stores it, rather than generating one internally.
- [ ] `Module::instance_id()` continues to return the stored ID unchanged.
- [ ] All module implementations in `patches-modules` updated accordingly.
- [ ] No public API change visible to callers of `ExecutionPlan` or
      `PatchBuilder` — only the registry and module constructors change.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.

## Notes

The global `AtomicU64` counter can stay as the backing mechanism for
`InstanceId::mint()` — the only change is that minting is a separate step from
construction, callable by the builder before `registry.create` is invoked.

If it is simpler to pass the ID through the `Registry::create` signature rather
than each module constructor individually, that is acceptable — the constraint
is that the builder controls when minting happens, not the module.
