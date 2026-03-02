---
id: "0035"
epic: "E007"
title: Add indexed port references (PortRef + PortDescriptor.index)
priority: high
created: 2026-03-02
---

## Summary

Port names alone are insufficient for modules that expose multiple ports with the
same semantic name (e.g. a `Sum` with `in/0`, `in/1`, `in/2`). Add an explicit
`index: u32` to `PortDescriptor` and introduce a `PortRef { name: &'static str,
index: u32 }` value type that callers use in `ModuleGraph::connect()`. Update all
port-resolution paths in the graph and builder accordingly.

## Acceptance criteria

- [ ] `PortDescriptor` gains `pub index: u32`. All existing module implementations
      updated to set `index: 0` (or the correct index for multi-port groups).
- [ ] `PortRef { pub name: &'static str, pub index: u32 }` is defined in
      `patches-core::module` and re-exported from `patches-core`.
- [ ] `ModuleGraph::connect()` signature changes from `(output: &str, …, input: &str)`
      to `(output: PortRef, …, input: PortRef)`.
- [ ] Edge storage in `graph.rs` identifies ports by `(name: String, index: u32)`.
      `GraphError::OutputPortNotFound` and `InputPortNotFound` include both name and
      index in their `port` field (format as `"name/index"`, e.g. `"in/2"`).
- [ ] `ModuleDescriptor` port lookup (in both `graph.rs` and `builder.rs`) matches
      on `(port.name, port.index)` rather than `port.name` alone.
- [ ] All call-sites in `patches-modules` and `patches-engine` updated to pass
      `PortRef` values. All existing tests updated and passing.
- [ ] `cargo clippy` clean, `cargo test` green across the workspace.

## Notes

**`PortRef` design:**

```rust
pub struct PortRef {
    pub name: &'static str,
    pub index: u32,
}
```

Port names are always `&'static str` (defined by module implementations at compile
time). A DSL layer that resolves port names will produce static string literals by
referencing the module's descriptor; it does not need to allocate.

**Builder port resolution:** `builder.rs` currently finds driving edges with
`input == port.name`. After this ticket it matches `input_name == port.name &&
input_index == port.index`. The same change applies to output-port position lookup
(`from_desc.outputs.iter().position(|p| p.name == out_name)` → match on both fields).

**Vec position vs. index field:** The position of a `PortDescriptor` in
`ModuleDescriptor::inputs` / `outputs` still determines the slice offset passed to
`Module::process`. The `index` field is the user-visible number in the port name
(`in/2`). For most existing modules these happen to be identical (a single
`("out", 0)` port at position 0), but they are semantically distinct.

**Ordering of related tickets:** T-0036 (`Sum` module) depends on this ticket.
T-0037 and T-0038 are independent.
