# ADR 0005 — DSL compilation pipeline

## Status

Accepted

## Context

The system needs a human-writable patch format that supports:
- Scalar, array, and table initialisation values for modules
- Template definitions: named sub-patches with explicit signal ports that can
  be instantiated as if they were single modules
- Polyphony: concise expression of N duplicated mono voices with fan-out/fan-in
  cable syntax

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
 Flat AST  (templates inlined, poly voices duplicated; only concrete instances remain)
    │
    ▼  Stage 3 — Graph builder  (patches-interpreter)
 ModuleGraph  (existing IR, unchanged)
```

**Stage 1 — PEG parser.**  A grammar defined using the `pest` crate parses
source text into an AST. The AST faithfully represents the surface syntax,
including template definitions, poly declarations, and init-param blocks. No
semantic validation happens here; the parser only enforces syntactic well-
formedness.

**Stage 2 — Expander.**  The expander performs two macro-like transformations,
both of which eliminate high-level constructs before the graph is built:

- *Template expansion.* Each `module foo : TemplateName { ... }` declaration
  is replaced by the template's body, with all internal `NodeId`s namespaced
  under `foo/` (e.g. `foo/osc`, `foo/env`). Template in-ports and out-ports
  become rewrite rules: top-level edges that reference `foo.in_port` are
  rewritten to target the appropriate internal node and port. Templates may be
  nested; the expander recurses until only primitive module types remain.

- *Poly expansion.* Each `module voices[N] : T { ... }` declaration is
  expanded into N declarations: `voices_0 : T`, …, `voices_{N-1} : T`, each
  fully expanded if `T` is a template. Edges using `[*]` index syntax are
  expanded: `a[*] -> b[*]` (zip) becomes N individual edges; `mono -> b[*]`
  (broadcast) becomes N edges from the same source. Fan-in from poly to a
  single module requires an explicit mix module in the source — no implicit
  fan-in sugar on first pass.

After Stage 2 the flat AST contains only concrete module declarations (a type
name, a `NodeId`, and an init-param map) and concrete edge declarations (two
fully-resolved `(NodeId, port-name, port-index)` triples and an optional scale
factor). There are no templates or poly indices.

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

The scale field on edges is relaxed from `[-1.0, 1.0]` to any finite `f64`.
Values outside `[-1.0, 1.0]` represent amplification; the caller is
responsible for ensuring the signal remains in a meaningful range. The
`ModuleGraph::connect` validation is updated accordingly.

### Factory-configured ports and internal polyphony

Because the factory receives init params before returning a module, the
resulting module's `descriptor()` can reflect those params — both port count
and port layout. This enables two things:

1. *Modules with variable port counts*, such as `Mixer { channels: N }` which
   exposes `N` input channel groups (`("in", 0..N)`, `("pan", 0..N)`,
   `("level", 0..N)`) and two outputs (`("out_l", 0)`, `("out_r", 0)`). The
   DSL author wires individual voices to `mix.in[0]`, `mix.in[1]`, etc. The
   module is a single instance in `ModuleGraph`.

2. *Internally polyphonic modules*, where a single module instance manages N
   voices in its own `process` implementation. Such a module declares `N`
   indexed input sets and handles voice allocation internally. This is an
   alternative to DSL-level poly expansion for modules where the internal
   structure is tightly coupled (e.g. a reverb with shared diffusion network
   across voices).

Neither of these requires any change to the audio thread, execution plan, or
buffer pool: the flat graph still contains a single node with a fixed (though
factory-determined) port count, and the `Module` trait is unchanged.

## Consequences

**`ModuleGraph` and the `Module` trait are largely unchanged.** The runtime IR
has no knowledge of templates or polyphony. `PortDescriptor` gains an `index`
field and edge records gain a port index; these are contained changes.

**Templates are a compile-time construct only.** There is no runtime notion of
a "template instance". Hot-reload re-runs the full three-stage pipeline.
Module state is preserved across reloads via the `InstanceId`-based registry
mechanism (ADR 0003); the `foo/osc` namespacing scheme provides stable
`NodeId`s as long as the patch source uses stable module names.

**Poly voices expanded by the DSL are independent mono modules.** The audio
thread, execution plan, and buffer pool have no notion of polyphony. Adding or
removing voices requires a full re-plan.

**Modules may alternatively implement polyphony internally.** A module with
factory-configured `N`-indexed input groups is indistinguishable from any other
module at the graph level. This is appropriate when internal coupling makes
per-voice cloning inefficient or incorrect.

**Fan-in of poly voices requires an explicit mix module.** There is no implicit
reduction syntax on first pass. This keeps the language unambiguous; mixing
semantics (sum, average, max) are explicit.

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
would make poly expansion harder to validate (you need all N instances resolved
before wiring). Rejected in favour of the three-stage separation, where each
stage is independently testable.

**Implicit fan-in sugar (`a[*] -> b.in` reduces to a mix automatically).**
Deferred: the mixing semantics (sum vs average vs something else) would need to
be specified, and the implicit mix node would appear in error messages without
a user-visible name. An explicit `Mixer` module is unambiguous. Sugar can be
added once there is practical experience with the verbose form.

**Represent polyphony only in the execution engine.**  A poly-aware executor
could run N voices with a single descriptor. Rejected for now: it would require
significant changes to the `Module` trait, buffer layout, and execution plan.
The factory-configured port approach achieves the same efficiency for tightly-
coupled modules without touching the runtime.
