---
id: "E011"
title: Sound engine internal decomposition
created: 2026-03-03
tickets: ["0047", "0048", "0049", "0050"]
---

## Summary

Following the audio-thread module pool work (E009), several internal structures in
`patches-engine` were left with mixed responsibilities and vtable dispatch in
performance-critical paths. This epic cleans up those internals: `ModulePool` gains
explicit sink awareness so the hot read path becomes a field access; `AudioCallback`
is decomposed into clearly named methods; signal dispatch logic is encapsulated on
`ExecutionPlan`; and `AudioCallback` is separated into its own module file.

## Tickets

| ID   | Title                                                      | Priority |
|------|------------------------------------------------------------|----------|
| 0047 | Add sink output caching to ModulePool                      | high     |
| 0048 | Decompose AudioCallback.fill_buffer into named methods     | medium   |
| 0049 | Encapsulate signal dispatch inside ExecutionPlan           | medium   |
| 0050 | Extract AudioCallback into its own module file             | low      |

## Definition of done

- All tickets closed.
- `cargo build`, `cargo test`, `cargo clippy` all clean.
- `ModulePool` exposes no module references outside the pool boundary.
- `fill_buffer` contains only top-level control flow; all inner work is in named methods.
- `signal_dispatch` is private to `ExecutionPlan`.
- `AudioCallback` lives in `callback.rs`; `engine.rs` contains only `SoundEngine`.
