# ADR 0005 — DSL compilation pipeline

## Status

Accepted

## Context

The system needs a human-writable patch format that supports:
- Scalar, array, and table initialisation values for modules
- Template definitions: named sub-patches with explicit signal ports that can
  be instantiated as if they were single modules
- Polyphony: poly cables carrying N channels simultaneously on instruments that
  support them, declared at connection time (see ADR 0015)

The existing runtime IR is `ModuleGraph`: a flat directed graph of
`Box<dyn Module>` nodes keyed by string `NodeId`, connected by named-port edges
with an optional scale factor. The DSL must ultimately produce a `ModuleGraph`.

## Decision

### Compilation pipeline

The DSL is compiled in three distinct stages:

```
.patches source text
    │
    ▼  Stage 1 — PEG parser  (patches-dsl)
 AST  (preserves spans for error messages; no semantic analysis yet)
    │
    ▼  Stage 2 — Expander  (patches-dsl)
 Flat AST  (templates inlined; only concrete instances remain)
    │
    ▼  Stage 3 — Graph builder  (patches-interpreter)
 ModuleGraph  (existing IR, unchanged)
```

**Stage 1 — PEG parser.**  A grammar defined using the `pest` crate parses
source text into an AST. The AST faithfully represents the surface syntax,
including template definitions, poly cable annotations, and init-param blocks. No
semantic validation happens here; the parser only enforces syntactic well-
formedness.

**Stage 2 — Expander.**  The expander performs one macro-like transformation
that eliminates high-level constructs before the graph is built:

- *Template expansion.* Each `module foo : TemplateName { ... }` declaration
  is replaced by the template's body, with all internal `NodeId`s namespaced
  under `foo/` (e.g. `foo/osc`, `foo/env`). Template in-ports and out-ports
  become rewrite rules: top-level edges that reference `foo.in_port` are
  rewritten to target the appropriate internal node and port. Templates may be
  nested; the expander recurses until only primitive module types remain.

Polyphony is not handled by module duplication in the expander. Instead, poly
cables carry N channels simultaneously on a single connection (see ADR 0015).
A `poly N` annotation on a connection tells the planner to allocate a
`CableValue::Poly` buffer slot; no additional graph nodes are introduced.

After Stage 2 the flat AST contains only concrete module declarations (a type
name, a `NodeId`, and an init-param map) and concrete edge declarations (two
fully-resolved `(NodeId, port-name, port-index)` triples, an optional scale
factor, and an optional poly channel count). There are no templates.

**Stage 3 — Graph builder.**  The graph builder iterates the flat AST and
populates a `ModuleGraph` using a *module factory registry* — a map from type
name strings to factory closures `fn(&ParamMap) -> Result<Box<dyn Module>>`.
Module factories receive the init-param map from the DSL and return a
constructed module, whose `descriptor()` reflects the init params (including
any factory-configured port counts). Edges are added via `ModuleGraph::connect`.
Port labels, indices, and init-param keys are validated against the module's
`ModuleDescriptor` and init-param schema at this stage. Error messages carry
source spans from Stage 1.

### Crate structure

```
patches-core        Core types, Module trait, ModuleGraph, ExecutionPlan
patches-modules     Module implementations (oscillators, filters, etc.)
patches-dsl         PEG grammar, AST types, template expander — no module knowledge
patches-interpreter Module factory registry; AST → ModuleGraph validation and build
patches-runtime     File-watching, reload orchestration, wires interpreter to engine
patches-engine      Planning, buffer allocation, audio execution — no DSL knowledge
```

`patches-dsl` depends only on `patches-core` (for AST value types). It has no
knowledge of any concrete module type and no audio-backend dependencies.
`patches-interpreter` depends on `patches-core`, `patches-dsl`, and
`patches-modules`. `patches-runtime` depends on `patches-interpreter` and
`patches-engine`; it is the only crate that knows about both sides.
`patches-engine` has no dependency on `patches-dsl` or `patches-interpreter`.

### ModuleDescriptor init-param schema

`ModuleDescriptor` gains a list of `ParamDescriptor` entries, each declaring a
parameter name and type (`scalar`, `array`, `table`). This allows
`patches-interpreter` to validate the DSL init-param map against the schema
before calling the factory, and to report errors with field-level granularity.

### Port identity: label + index

`PortDescriptor` currently carries only a `name: &'static str`. It is extended
to carry an `index: usize` alongside the label:

```
PortDescriptor { label: &'static str, index: usize }
```

For modules with a fixed, compile-time-known port count, all ports have
`index: 0` and are distinguished by distinct labels (`"freq"`, `"gate"`,
`"out"`). For modules with a factory-configured port count — such as a mixer
whose channel count is an init param — the factory produces multiple
`PortDescriptor` entries sharing the same label and differing only by index:
`("in", 0)`, `("in", 1)`, …, `("in", N-1)`.

In the DSL, `mix.in[2]` addresses label `"in"` at index `2`. In the flat AST
(and in `ModuleGraph` edge records), a port is identified by the pair
`(label, index)`. The `ModuleGraph` API and `ExecutionPlan` are updated to use
this pair in place of a bare string.

### Signal scale factor

The scale field on edges is relaxed from `[-1.0, 1.0]` to any finite `f32`.
Values outside `[-1.0, 1.0]` represent amplification; the caller is
responsible for ensuring the signal remains in a meaningful range. The
`ModuleGraph::connect` validation is updated accordingly.

### Factory-configured ports

Because the factory receives init params before returning a module, the
resulting module's `descriptor()` can reflect those params — both port count
and port layout. This enables *modules with variable port counts*, such as
`Mixer { channels: N }` which exposes `N` input channel groups
(`("in", 0..N)`, `("pan", 0..N)`, `("level", 0..N)`) and two outputs
(`("out_l", 0)`, `("out_r", 0)`). The DSL author wires inputs to
`mix.in[0]`, `mix.in[1]`, etc. The module is a single instance in
`ModuleGraph`.

This does not require any change to the audio thread, execution plan, or
buffer pool: the flat graph contains a single node with a fixed (though
factory-determined) port count, and the `Module` trait is unchanged.

Polyphony — modules that process N simultaneous voice channels — is handled
via poly cables and poly-typed ports in `PortDescriptor`, not by
factory-configured indexed port groups. See ADR 0015 for the full design.

## Consequences

**`ModuleGraph` and the `Module` trait are largely unchanged.** The runtime IR
has no knowledge of templates or polyphony as graph-level constructs.
`PortDescriptor` gains an `index` field and a `kind: CableKind` field (see ADR
0015); edge records gain a port index and an optional poly channel count. These
are contained changes.

**Templates are a compile-time construct only.** There is no runtime notion of
a "template instance". Hot-reload re-runs the full three-stage pipeline.
Module state is preserved across reloads via the `InstanceId`-based registry
mechanism (ADR 0003); the `foo/osc` namespacing scheme provides stable
`NodeId`s as long as the patch source uses stable module names.

**Polyphony is carried by poly cables, not by module duplication.** A
polyphonic patch uses the same number of module slots as its mono equivalent.
Voice-level arithmetic is a tight inner loop inside each poly-capable module's
`process()` call. See ADR 0015 for the full design including `CableKind`,
`CableValue`, `InputPort`/`OutputPort`, and the `MonoShim` migration path.

**Factory registry is the extension point for new module types.**  Adding a new
module type requires implementing `Module`, registering a factory closure, and
declaring a `ParamDescriptor` schema. The DSL parser requires no changes.

**Init params are not signal ports.** Values in a module's `{ key: value }`
block are passed to the factory at graph-build time; they cannot change at
audio rate. Modules that need runtime variation expose a signal input port.

**`patches-engine` has no DSL dependency.** It receives a `ModuleGraph` from
`patches-runtime` and knows nothing about how that graph was produced.
`patches-dsl` similarly has no knowledge of concrete module types.

## Alternatives considered

**Interpret templates at runtime (keep them in `ModuleGraph`).** Would require
`ModuleGraph` to understand template hierarchy, complicating buffer allocation,
the planner, and the execution plan. Rejected: the expander approach keeps all
complexity in the DSL layer and leaves the runtime IR simple.

**Compile directly to `ModuleGraph` in a single pass.** Eliminating the
intermediate flat AST would tangle parsing, expansion, and validation, and
would make template expansion harder to validate (all instances must be resolved
before connections can be rewritten). Rejected in favour of the three-stage
separation, where each stage is independently testable.

**Poly voices duplicated by the expander (original approach).**  An earlier
version of this ADR proposed Stage 2 duplicating `module voices[N] : T`
declarations into N independent mono modules. This approach was superseded by
ADR 0015 (poly cables): duplicating modules scales tick-loop overhead
O(modules × voices), whereas poly cables keep it O(modules). Module duplication
was rejected as the primary polyphony mechanism.

**Represent polyphony only in the execution engine.**  A poly-aware executor
running N voices with a single descriptor was considered but would have required
significant changes to the `Module` trait, buffer layout, and execution plan
with no clear entry point. The poly-cables approach (ADR 0015) achieves the
same result with contained changes: `CableKind`/`CableValue` in the buffer pool
and `InputPort`/`OutputPort` as module-owned state, with no change to the
module graph structure or expander.
