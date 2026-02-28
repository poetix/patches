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
tickets/          Work tracking (see Ticket workflow below)
```

`patches-modules` depends on `patches-core`. New audio modules should live in `patches-modules` unless they are foundational types needed by the engine itself.

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

## Audio engine conventions

- **No allocations on the audio thread.** All buffers and module state must be pre-allocated.
- **No blocking on the audio thread.** No mutexes, no I/O, no syscalls in the processing path.
- **Real-time/non-real-time boundary.** Use lock-free data structures (e.g. ring buffers, atomics) to communicate between the audio thread and the hot-reload/control thread.

## General conventions

- No `unwrap()` or `expect()` in library code — use proper error propagation.
- Keep `patches-core` free of audio-backend dependencies so it can be tested without hardware.
- Run `cargo clippy` and `cargo test` before considering any implementation ticket done.
- Ask before adding new dependencies to `Cargo.toml`.
