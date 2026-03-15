---
id: "0001"
title: Core module trait and signal types
priority: high
created: 2026-02-28
depends_on: []
epic: "E001"
---

## Summary

Define the foundational types and traits that all audio modules implement. Everything else — module implementations, the graph, execution plan, and engine — depends on a shared, stable contract for what a module is and how it processes audio.

## Acceptance criteria

- [ ] `Module` trait defined with a single-sample `process` method:
      `fn process(&mut self, inputs: &[f32], outputs: &mut [f32], sample_rate: f32)`
- [ ] `PortDescriptor` type describing a port by name and direction (input/output)
- [ ] `ModuleDescriptor` type (returned by a method on `Module`) listing a module's input and output ports — used by the graph and builder to validate connections and resolve port names to indices
- [ ] `SampleBuffer` type: a 2-element ring buffer (`[f32; 2]` + write index) representing a single patch cable — writer stores into the current slot, reader takes from the previous slot
- [ ] All types are in `patches-core` with no audio-backend dependencies
- [ ] `cargo test -p patches-core` passes
- [ ] `cargo clippy` is clean

## Notes

**Why single-sample processing:** Cycles in the module graph are permitted (e.g. feedback paths). Processing one sample at a time with a 2-sample cable buffer means every connection carries a 1-sample delay, making all cycles safe regardless of execution order. There is no concept of a block size at this layer.

**`SampleBuffer` semantics:** During each engine tick, the writer calls `buffer.write(value)` and the reader calls `buffer.read()` which returns the value written in the *previous* tick. After all modules have processed, the engine advances all buffers by toggling the write index. This ensures feedback connections are always well-defined.

**Port indices:** The `process` method uses `inputs` and `outputs` by index. Port names live on the `ModuleDescriptor`; the patch builder resolves names to indices when building the execution plan. Modules should not need to know their own port names at process time.
