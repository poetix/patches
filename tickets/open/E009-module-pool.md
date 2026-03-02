---
id: "E009"
title: Audio-thread-owned module pool and live state preservation
priority: high
created: 2026-03-02
---

## Summary

Replace the current `ModuleInstanceRegistry` / `held_plan` state-preservation mechanism
with an audio-thread-owned module pool, symmetric with the existing buffer pool. Module
instances survive replans by remaining in the pool rather than being moved across the
thread boundary. State preservation becomes automatic and works on every replan, not only
in the channel-full retry edge case.

See `adr/0009-audio-thread-owned-module-pool.md` for the full design rationale.

## Motivation

The current design does not preserve module state across replans in normal operation.
`PatchEngine::held_plan` is `None` after every successful `swap_plan`, so
`Planner::build` always receives `None` and produces a plan with fresh, stateless modules.
The `held_plan` / `ModuleInstanceRegistry` machinery only fires in the channel-full retry
case and is architecturally confused about two unrelated concerns (state source vs retry
buffer).

## Tickets

| Ticket | Title |
|--------|-------|
| T-0042 | Introduce `ModuleAllocState` |
| T-0043 | Migrate `ExecutionPlan` and `SoundEngine` to module pool |
| T-0044 | Remove `ModuleInstanceRegistry`, `held_plan`, and `ExecutionPlan::initialise` |
| T-0045 | Update integration tests for module pool |

## Open tickets resolved by this epic

- T-0031 — state preservation across replans
- T-0034 — held-plan channel-full path (design superseded; no implementation needed)

## Definition of done

- All four tickets closed
- `cargo build`, `cargo test`, `cargo clippy` all clean
- Module state (e.g. oscillator phase) demonstrably survives a replan in integration tests
- No reference to `ModuleInstanceRegistry`, `held_plan`, or `ExecutionPlan::into_registry`
  remains in the codebase
