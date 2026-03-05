---
id: "0083"
title: Graph YAML serialisation and deserialisation
priority: medium
created: 2026-03-05
---

## Summary

Add `graph_to_yaml` and `yaml_to_graph` functions to `patches-core` so that a
`ModuleGraph` can be written to and loaded from a compact YAML file. The format
stores only the node id, module name, `ModuleShape`, and parameter values for
each node — just enough to round-trip. On load the `Registry` is consulted to
rehydrate the full `ModuleDescriptor` for each node before inserting it into a
fresh graph.

This gives a usable patch-file format while the proper DSL is still under
development.

## YAML schema

```yaml
nodes:
  osc1:
    module: Oscillator
    channels: 1
    length: 0          # optional; omit or 0 for modules without array params
    params:
      frequency: 440.0
      waveform: sine

  amp1:
    module: Amplifier
    channels: 1
    params:
      gain: 0.5

cables:
  - from: osc1
    output: out        # port name
    output_index: 0    # optional; defaults to 0
    to: amp1
    input: in
    input_index: 0     # optional; defaults to 0
    scale: 1.0         # optional; defaults to 1.0
```

Parameter values are plain YAML scalars (number, bool, string). On
deserialisation the module's `ParameterDescriptor` is used to coerce each value
to the correct `ParameterValue` variant, so no explicit type tags are needed in
the file.

## Acceptance criteria

- [ ] New module `patches-core::graph_yaml` with two public functions:
  - `pub fn graph_to_yaml(graph: &ModuleGraph) -> Result<String, GraphYamlError>`
  - `pub fn yaml_to_graph(yaml: &str, registry: &Registry) -> Result<ModuleGraph, GraphYamlError>`
- [ ] `graph_to_yaml` serialises every node (id, module name, shape, non-default
      params) and every edge (from, output name/index, to, input name/index,
      scale). Edges with `scale == 1.0` may omit the field; `output_index` /
      `input_index` of `0` may also be omitted.
- [ ] `yaml_to_graph` calls `registry.describe(name, &shape)` to obtain the
      `ModuleDescriptor` for each node and returns `GraphYamlError::UnknownModule`
      if the registry has no builder for the given name.
- [ ] Parameter values are coerced using the descriptor: a YAML float is accepted
      for a `Float` parameter, a YAML integer for `Int`, a YAML bool for `Bool`,
      a YAML string for `Enum`, and a YAML sequence of strings for `Array`.
      Type mismatches return `GraphYamlError::ParameterTypeMismatch`.
- [ ] A round-trip test: build a small graph, serialise it, deserialise it,
      verify node ids, module names, shapes, params, and edges are all
      preserved.
- [ ] `cargo clippy` and `cargo test` pass.

## Notes

Requires `serde` and `serde_yaml` (or `serde-yml`) as new dependencies on
`patches-core` — request approval before adding them.

The `Registry::describe` method currently has a bug (returns `Option<_>` but
calls `.ok_or_else(…)?` which doesn't compile — the `?` tries to propagate a
`Result` but the return type is `Option`). Fix this as a prerequisite or inline
the fix in this ticket.

`graph_to_yaml` does not need to call the registry at all; the module name and
shape are already stored in `Node::module_descriptor` and can be written
directly.

Port name/index pairs can be written as the compact `name/index` string
(matching the existing `GraphError` display format) or as separate fields —
either is fine; pick whichever is easier to read.
