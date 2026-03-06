# patches

A Rust system for defining modular audio patches and running them in a real-time
audio engine, with support for hot-reloading patches at runtime. The intended use
case is live-coding performance: the patch graph can be rebuilt and swapped in
without stopping the audio stream or resetting module state (oscillator phase,
filter history, etc.).

![A synth patch that patches can run](https://github.com/poetix/patches/blob/main/demo_synth.png?raw=true)

## Goals

- **Patch DSL** — a DSL for describing signal graphs of audio modules
  (connections, scaling, routing).
- **Audio engine** — a real-time processing pipeline that accepts new patch plans
  without allocating or blocking on the audio thread.
- **Live-reload** — stateful module instances survive re-planning; only structurally
  changed parts of the graph are reset.

## Current state

The full intended DSL is not implemented, but a temporary YAML format is in place which enables patches to be loaded from file. `patches-player` will watch its input file and re-plan the patch (keeping existing modules running) if a new version is saved.

The core engine and a small practical set of modules are all in place:

- `Module` trait with `prepare` (called once on plan activation) and `process`
  (called per sample, allocation-free).
- `ModuleGraph` for building signal graphs with scaled connections.
- `ExecutionPlan` produced by a pure `build_patch` function; uses a flat buffer
  pool with a 1-sample cable delay so modules can run in any order.
- Audio-thread-owned module pool: module instances (and their state — oscillator
  phase, filter history, envelope position) live on the audio thread and survive
  hot-reloads automatically without crossing the thread boundary.
- Lock-free plan handoff to the audio thread via an rtrb ring buffer.
- YAML patch DSL: `graph_to_yaml` / `yaml_to_graph` in `patches-core`; the format
  supports nodes, cables, parameters (float, int, bool, enum, array), scaled
  connections, and indexed ports (`in/1`).
- `patch_player` binary (`patches-player`): loads a YAML patch, plays it, and
  hot-reloads whenever the file changes on disk.
- Modules: sine oscillator, sawtooth oscillator, square oscillator, sum/mix,
  ADSR envelope, step sequencer, clock sequencer, VCA, glide (portamento),
  audio output.
- Examples: `sine_tone`, `chord_swap`, `demo_synth` (16-step minor-pentatonic
  sequence at 120 BPM with glide, LFO-modulated square, and ADSR).

In progress:
- E010: off-thread module deallocation — tombstoned modules are currently dropped
  inline in the audio callback; tickets 0052–0053 move drops to a cleanup thread.
- E015: port connectivity awareness — modules will be notified when their ports
  are connected or disconnected (tickets 0078–0081).

## Workspace layout

```text
patches-core/     Core types, traits, YAML DSL, and the execution plan runtime.
                  No audio-backend dependencies; fully testable without hardware.

patches-modules/  Audio module implementations (oscillators, mixing, effects, …).

patches-engine/   Patch builder, Planner, PatchEngine, CPAL sound engine,
                  and runnable examples.

patches-player/   `patch_player` binary: load a YAML patch, play it, hot-reload
                  on file change.

tickets/          Work tracking (open / in-progress / closed).
epics/            Epics grouping related tickets.
adr/              Architecture decision records.
```

## Building and running

```bash
cargo build
cargo test
cargo clippy

# Run a built-in example (generates audio in code):
cargo run --example sine_tone    # 440 Hz sine tone
cargo run --example demo_synth   # 16-step melodic sequence at 120 BPM

# Run the player with a YAML patch file (hot-reloads on save):
cargo run -p patches-player -- demo_synth.yaml
```

## Patch format

```yaml
nodes:
  osc:
    module: SineOscillator
    params:
      frequency: 440.0

  out:
    module: AudioOut

cables:
  - from: osc
    output: out
    to: out
    input: left
  - from: osc
    output: out
    to: out
    input: right
```

Parameters are plain YAML scalars (float, int, bool, string for enums, sequences
for arrays). Port indices use `name/n` notation (e.g. `in/1`). Cable `scale`
defaults to `1.0` and is omitted when serialising at that value.

## Design constraints

- No allocations on the audio thread.
- No blocking on the audio thread (no mutexes, I/O, or syscalls in the processing path).
- `patches-core` has no knowledge of audio backends, file formats, or UI.
