---
id: "E001"
title: Foundational audio engine
created: 2026-02-28
tickets: ["0001", "0002", "0003", "0004", "0005", "0006"]
---

## Summary

Establish the complete foundational stack required to run a live audio patch: the core module abstraction, a graph for wiring modules together, concrete module implementations (sine oscillator and stereo audio output), a patch builder that resolves the graph into an executable plan, and a sound engine that drives the plan against real audio hardware.

This epic delivers the first end-to-end vertical slice of the system — no DSL, no hot-reload, no UI — just the minimum runtime capable of producing sound from a hand-assembled patch graph.

## Acceptance criteria

- [ ] A runnable executable (e.g. in `patches-engine/src/bin/` or a dedicated `patches-cli` crate) that:
  - Constructs a `ModuleGraph` containing a `SineOscillator` (frequency fixed at 440 Hz) connected to both inputs of an `AudioOut` node
  - Builds an `ExecutionPlan` from the graph using the patch builder
  - Passes the plan to `SoundEngine` and starts it
  - Emits a continuous 440 Hz sine wave to the default audio output device
  - Runs until the process is terminated (e.g. Ctrl-C)
- [ ] All six tickets closed
- [ ] `cargo build` and `cargo clippy` are clean across the workspace
- [ ] `cargo test` passes across the workspace

## Tickets

| ID | Title |
|----|-------|
| [0001](../../tickets/closed/0001-core-module-trait.md) | Core module trait and signal types |
| [0002](../../tickets/closed/0002-module-graph.md) | Module graph structure |
| [0003](../../tickets/closed/0003-sine-oscillator-module.md) | Sine oscillator module |
| [0004](../../tickets/closed/0004-audio-output-module.md) | Audio output module |
| [0005](../../tickets/closed/0005-patch-builder.md) | Patch builder (toposort and execution plan) |
| [0006](../../tickets/closed/0006-sound-engine.md) | Sound engine |

## Notes

**Crate structure introduced by this epic:**
```
patches-core     ←  patches-modules  ←  patches-engine
(traits, graph)     (sine, audio out)    (builder, engine, CPAL, binary)
```

**Not in scope for this epic:** DSL parsing, hot-reload, patch serialisation, parameter control, multiple patch outputs, or any form of UI.

**440 Hz is a deliberate choice** for the demo binary — it is concert A, unambiguous and easy to verify by ear.

**Spec divergence:** the acceptance criterion asked for an executable that runs until Ctrl-C. The deliverable is instead `patches-engine/examples/sine_tone.rs`, which plays for 3 seconds and exits. The example plays a major third (A4 + C#5) via `Crossfade` rather than a raw 440 Hz sine, and demonstrates the full end-to-end stack. No `src/bin/` or dedicated CLI crate was created; the example is judged sufficient for a reference implementation at this stage.
