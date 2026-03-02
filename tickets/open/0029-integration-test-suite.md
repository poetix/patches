---
id: "0029"
epic: "E006"
type: epic
title: Integration test suite
priority: medium
created: 2026-03-02
---

## Summary

Unit tests in `patches-core` and `patches-engine` validate individual functions and
data structures in isolation, but the replanning lifecycle — where a control thread
builds a new plan while the audio thread runs the old one — involves interactions
across all three crates that unit tests cannot reach. This epic introduces a
dedicated `patches-integration-tests` crate and a growing suite of tests that
exercise the system end-to-end without opening any audio hardware.

## Key fixture: HeadlessEngine

The `HeadlessEngine` test fixture replicates the CPAL audio-callback contract
synchronously:

1. Zero every slot in `new_plan.to_zero`.
2. Replace the active plan — **dropping the old plan** and all modules it contains.
3. `tick` samples one at a time.

It intentionally exposes no method for extracting the active plan, enforcing the
same boundary that exists in production: the running plan is owned by the audio
thread and is never accessible to the control thread.

## Scope

### Done

- `patches-integration-tests` crate added to workspace (`publish = false`,
  `[[test]]` targets only)
- `HeadlessEngine` and `DropSpy` fixtures implemented
- Replanning lifecycle tests (ticket 0030)

### Planned

- **State preservation across replans** — verify that a module surviving a re-plan
  retains its internal state (e.g. oscillator phase), and that a replaced module
  starts fresh.
- **Stable buffer indices** — end-to-end check that a cable surviving a re-plan
  reads from the same pool slot before and after, producing no discontinuity in
  the output signal.
- **Multi-source mixing** — replanning a graph with `Mix`, verifying correct stereo
  output and correct slot reuse for the mixer's output buffer.
- **Held-plan / channel-full path** — simulate `swap_plan` returning a plan
  (channel full) and verify that module state is preserved through the retry cycle.

## Notes

Integration tests must not open audio hardware. Any test that requires a real
device should be gated with `#[ignore]` and documented clearly.

Tests in `patches-integration-tests` are the right home for any scenario that
needs to import from more than one of `patches-core`, `patches-modules`, and
`patches-engine` simultaneously, or that needs to observe cross-boundary behaviour
(e.g. what a module's Drop impl sees vs. what the pool contains).
