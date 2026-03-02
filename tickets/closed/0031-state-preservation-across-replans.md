---
id: "0031"
epic: "E006"
title: State preservation across replans
priority: medium
created: 2026-03-02
---

## Summary

Verify that a module surviving a re-plan retains its internal state (e.g. oscillator
phase), and that a module replaced by a fresh instance of the same type starts from
its default state.

## Acceptance criteria

- [ ] Integration test: build a graph with a stateful module (e.g. `SineOscillator`);
      tick N samples; re-plan to an identical graph; tick further samples and confirm
      the surviving module instance is the same object (same `InstanceId`) and that
      its output is continuous (no phase reset)
- [ ] Integration test: re-plan to a graph that replaces the module with a new node
      of the same type; confirm the new instance has a different `InstanceId` and
      that its output starts from the initial state
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean

## Notes

Use `InstanceId` equality to distinguish reuse from replacement. A phase-continuity
check can compare the output sample immediately before and after the re-plan against
the expected sinusoidal progression.
