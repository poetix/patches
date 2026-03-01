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

The DSL is compiled in three distinct stages:

```
.patches source text
    │
    ▼  Stage 1 — PEG parser
 AST  (preserves spans for error messages; no semantic analysis yet)
    │
    ▼  Stage 2 — Expander
 Flat AST  (templates inlined, poly voices duplicated; only concrete instances remain)
    │
    ▼  Stage 3 — Graph builder
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
  fully expanded if `T` is a template. Edges that use `[*]` index syntax are
  expanded: `a[*] -> b[*]` (zip) becomes N individual edges; `mono -> b[*]`
  (broadcast) becomes N edges from the same source; `a[*] -> sink` (collect)
  becomes N edges to N ports on the destination.

After Stage 2 the flat AST contains only concrete module declarations (a type
name, a `NodeId`, and an init-param map) and concrete edge declarations (two
fully-resolved `(NodeId, port-name)` pairs and an optional scale factor). There
are no templates or poly indices.

**Stage 3 — Graph builder.**  The graph builder iterates the flat AST and
populates a `ModuleGraph` using a *module factory registry* — a map from type
name strings to factory closures `fn(&ParamMap) -> Result<Box<dyn Module>>`.
Module factories receive the init-param map from the DSL and return a
constructed module. Edges are added via `ModuleGraph::connect`. Port names are
validated against the module's `ModuleDescriptor` at this stage.

**Crate structure.**  The DSL lives in a new `patches-dsl` crate that depends
on `patches-core` only (no audio-backend or CPAL dependencies). The module
factory registry is populated by `patches-engine`, which registers the concrete
module types from `patches-modules`. This keeps `patches-dsl` free of hardware
dependencies and fully testable in isolation.

## Consequences

**`ModuleGraph` is unchanged.** The runtime IR has no knowledge of templates or
polyphony; it only ever contains concrete module instances. All existing
planner, buffer-pool, and execution-plan logic continues to work without
modification.

**Templates are a compile-time construct only.** There is no runtime notion of
a "template instance". Hot-reload re-runs the full three-stage pipeline and
produces a new `ModuleGraph`. Module state is preserved across reloads via the
existing `InstanceId`-based registry mechanism (ADR 0003), which requires that
the `NodeId`s produced by template expansion are stable across reloads for
unchanged modules — the namespacing scheme (`foo/osc`) provides this as long as
the patch source uses stable module names.

**Poly voices are independent mono modules.** The audio thread, execution plan,
and buffer pool have no notion of polyphony. Each voice is a set of ordinary
module instances. Adding or removing voices requires a full re-plan; there is no
partial hot-swap of voice count.

**Factory registry is the extension point for new module types.** Adding a new
module type requires implementing `Module`, registering a factory closure, and
documenting the init-param names and types. The DSL parser requires no changes.

**Init params are not signal ports.** Values in a module's `{ key: value }`
block are passed to the factory at graph-build time. They cannot change at
audio rate or be driven by other modules' outputs. Modules that need runtime
variation expose a signal input port instead.

**Error reporting spans Stage 1 and Stage 3.** The parser records source spans
in the AST so that Stage 3 validation errors (unknown module type, bad port
name, scale out of range) can be reported with line-and-column context from the
original source.

## Alternatives considered

**Interpret templates at runtime (keep them in `ModuleGraph`).** Would require
`ModuleGraph` to understand template hierarchy, complicating buffer allocation,
the planner, and the execution plan. Rejected: the expander approach keeps all
complexity in the DSL layer and leaves the runtime IR simple.

**Compile directly to `ModuleGraph` in a single pass.** Eliminating the
intermediate flat AST would make it harder to validate poly expansion (you need
to know all N instances before wiring them) and would tangle parsing, expansion,
and validation. The three-stage separation makes each stage testable in
isolation.

**Represent polyphony in the execution engine.** A dedicated poly-aware executor
could run N voices with a single module descriptor. Rejected for now: it would
require significant changes to the `Module` trait, buffer layout, and execution
plan, and the benefits (cache locality, simpler voice management) can be pursued
later as a contained optimisation without changing the DSL or the `ModuleGraph`
IR.
