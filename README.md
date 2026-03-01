# patches

A Rust system for defining modular audio patches and running them in a real-time
audio engine, with support for hot-reloading patches at runtime. The intended use
case is live-coding performance: the patch graph can be rebuilt and swapped in
without stopping the audio stream or resetting module state (oscillator phase,
filter history, etc.).

## Goals

- **Patch DSL** — a format for describing signal graphs of audio modules (connections,
  scaling, routing).
- **Audio engine** — a real-time processing pipeline that accepts new patch plans
  without allocating or blocking on the audio thread.
- **Live-reload** — stateful module instances survive re-planning; only structurally
  changed parts of the graph are reset.

## Current state

The core infrastructure is in place:

- `Module` trait with `initialise` (called once on plan activation) and `process`
  (called per sample, allocation-free).
- `ModuleGraph` for building signal graphs with scaled connections.
- `ExecutionPlan` produced by a pure `build_patch` function; uses a flat buffer
  pool with a 1-sample cable delay so modules can run in any order.
- `Planner` and `PatchEngine` for coordinating re-planning with module-state
  preservation across hot-reloads.
- Lock-free plan handoff to the audio thread via an rtrb ring buffer.
- A small set of modules: sine oscillator, crossfade, audio output.

In progress: stable cable buffers across re-plans (no discontinuity on hot-reload)
and structured module destruction (`E005`).

Not yet started: the patch DSL and parser.

## Workspace layout

```text
patches-core/     Core types, traits, and the execution plan runtime.
                  No audio-backend dependencies; fully testable without hardware.

patches-modules/  Audio module implementations (oscillators, mixing, effects, …).

patches-engine/   Patch builder, Planner, PatchEngine, CPAL sound engine,
                  and runnable examples.

tickets/          Work tracking (open / in-progress / closed).
epics/            Epics grouping related tickets.
adr/              Architecture decision records.
```

## Building and running

```bash
cargo build
cargo test
cargo clippy
cargo run --example sine_tone    # plays a 440 Hz sine tone
```

## Design constraints

- No allocations on the audio thread.
- No blocking on the audio thread (no mutexes, I/O, or syscalls in the processing path).
- `patches-core` has no knowledge of audio backends, file formats, or UI.
