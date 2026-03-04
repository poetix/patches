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

The core engine and a practical set of modules are in place:

- `Module` trait with `initialise` (called once on plan activation) and `process`
  (called per sample, allocation-free).
- `ModuleGraph` for building signal graphs with scaled connections.
- `ExecutionPlan` produced by a pure `build_patch` function; uses a flat buffer
  pool with a 1-sample cable delay so modules can run in any order.
- Audio-thread-owned module pool: module instances (and their state — oscillator
  phase, filter history, envelope position) live on the audio thread and survive
  hot-reloads automatically without crossing the thread boundary.
- Lock-free plan handoff to the audio thread via an rtrb ring buffer.
- Control-rate signalling: `ControlSignal` enum and `Module::receive_signal`;
  the engine distributes signals at a configurable control rate using chunked
  sample processing so there is no per-sample branch overhead.
- Modules: sine oscillator, sawtooth oscillator, square oscillator, sum/crossfade,
  ADSR envelope, step sequencer, clock sequencer, VCA, audio output.
- Examples: `sine_tone`, `chord_swap`, `freq_sweep`, `demo_synth` (16-step
  melodic sequence at 120 BPM demonstrating the full module set).

In progress: off-thread module deallocation (`E010`) — tombstoned modules are
currently dropped inline in the audio callback; the next epic moves drops to a
dedicated cleanup thread via a ring buffer.

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
cargo run --example demo_synth   # plays a 16-step melodic sequence at 120 BPM
```

## Design constraints

- No allocations on the audio thread.
- No blocking on the audio thread (no mutexes, I/O, or syscalls in the processing path).
- `patches-core` has no knowledge of audio backends, file formats, or UI.
