---
id: "E006"
title: Integration test suite
created: 2026-03-02
tickets: ["0029", "0031", "0032", "0033", "0034"]
---

## Summary

Unit tests in `patches-core` and `patches-engine` validate individual functions and
data structures in isolation, but the replanning lifecycle — where a control thread
builds a new plan while the audio thread runs the old one — involves interactions
across all three crates that unit tests cannot reach. This epic introduces a
dedicated `patches-integration-tests` crate and a growing suite of tests that
exercise the system end-to-end without opening any audio hardware.

The `HeadlessEngine` test fixture replicates the CPAL audio-callback contract
synchronously: it zeroes released buffer slots, swaps the active plan (dropping the
old one), and ticks samples one at a time. It exposes no method for extracting the
active plan, enforcing the same control/audio-thread boundary that exists in
production.

## Acceptance criteria

- [ ] All tickets closed
- [ ] `cargo build`, `cargo test`, `cargo clippy` all clean
- [ ] No integration test opens audio hardware

## Tickets

| ID   | Title                                   | Priority |
|------|-----------------------------------------|----------|
| 0029 | Integration test suite (infra + T-0030) | medium   |
| 0031 | State preservation across replans       | medium   |
| 0032 | Stable buffer indices end-to-end        | medium   |
| 0033 | Multi-source mixing integration test    | medium   |
| 0034 | Held-plan / channel-full path           | medium   |

## Notes

Tests in `patches-integration-tests` are the right home for any scenario that
needs to import from more than one of `patches-core`, `patches-modules`, and
`patches-engine` simultaneously, or that needs to observe cross-boundary behaviour.

Any test that requires a real audio device must be gated with `#[ignore]` and
documented clearly.
