# Patches — Claude context

## Project overview

Patches is a Rust system for defining modular audio patches using a DSL. Patches can be reloaded at runtime to modify the patch setup, enabling live-coding performance. The system also includes an efficient audio engine for running these patches.

The two key concerns are:
1. **DSL and patch definition** — a format for describing signal graphs of audio modules
2. **Audio engine** — real-time audio processing with hot-reload capability

## Workspace layout

```
patches-core/     Core types, traits, DSL parsing, and the audio engine runtime
patches-modules/  Module implementations (oscillators, filters, effects, etc.)
patches-engine/   Patch builder, sound engine, CPAL integration, and examples
tickets/          Work tracking (see Ticket workflow below)
```

`patches-modules` depends on `patches-core`. `patches-engine` depends on both. New audio modules should live in `patches-modules` unless they are foundational types needed by the engine itself.

## Commands

```bash
cargo build               # build all crates
cargo test                # run all tests
cargo clippy              # lint (fix all warnings before considering work done)
cargo test -p patches-core    # test a single crate
```

## Ticket workflow

Work is tracked in `tickets/` using markdown files organised by status:

- `tickets/open/` — not yet started
- `tickets/in-progress/` — currently being worked on
- `tickets/closed/` — done

Filename convention: `NNNN-short-description.md` (e.g. `0001-dsl-parser.md`).

Use `tickets/TEMPLATE.md` as the starting point for new tickets.

When starting a ticket: move it to `in-progress/`. When done: move it to `closed/`.

## Architecture decision records

Design decisions with trade-offs are recorded in `adr/` as numbered markdown files (`NNNN-short-description.md`). Reference the relevant ADR from tickets and code comments where a decision might otherwise seem arbitrary.

## Audio engine conventions

- **No allocations on the audio thread.** All buffers and module state must be pre-allocated.
- **No blocking on the audio thread.** No mutexes, no I/O, no syscalls in the processing path.
- **Real-time/non-real-time boundary.** Use lock-free data structures (e.g. ring buffers, atomics) to communicate between the audio thread and the hot-reload/control thread.

## Design desiderata

These are qualities the system should preserve as it evolves. They inform design decisions but are not hard rules — trade-offs are recorded in `adr/`.

- **Parallelism-ready execution.** The 1-sample cable delay means modules can run in any order. The execution plan should remain structured so that splitting modules across threads is a contained change to `ExecutionPlan::tick()` and the builder's buffer layout, with no impact on the Module trait, ModuleGraph, or module implementations.
- **Cache-friendly buffer layout.** Cable buffers should be packed densely in memory. When parallelism arrives, the builder should partition buffers by thread affinity (buffers accessed by the same thread are contiguous) and pad partition boundaries to cache lines to avoid false sharing.
- **Zero-cost descriptors.** Module descriptors (port names, counts) are compile-time constants defined by module implementations, not by the DSL. Port names are `&'static str`; accessing descriptors should not allocate. The DSL specifies *which* modules to instantiate and how to wire them, but port layouts are fixed per module type.
- **Backend-agnostic core.** `patches-core` defines traits and data structures with no knowledge of audio backends, file formats, or UI. Concrete backends live in `patches-engine` or dedicated crates.

## General conventions

- No `unwrap()` or `expect()` in library code — use proper error propagation.
- Keep `patches-core` free of audio-backend dependencies so it can be tested without hardware.
- Run `cargo clippy` and `cargo test` before considering any implementation ticket done.
- Ask before adding new dependencies to `Cargo.toml`.
