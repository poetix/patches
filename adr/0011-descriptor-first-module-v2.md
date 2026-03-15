# ADR 0011 — Descriptor-first module construction (Module v2)

**Date:** 2026-03-04
**Status:** Accepted

## Context

### The old model conflated description with instantiation

Under the previous design (`Module v1`), a `ModuleGraph` node held a `Box<dyn Module>` — a
live, initialised instance. The graph was therefore both a topology structure and a container
for runtime state. This created several problems:

**Graph validation required live instances.** `add_module` accepted a `Box<dyn Module>` and
called `descriptor()` on it to validate port connections. A module had to be fully
constructed — including any audio-environment-dependent state — before the graph could be
wired. This made it impossible to validate or inspect a patch without having an audio
environment available (e.g. in tests, in a DSL compiler, or in a headless tooling context).

**Module construction was opaque.** The `Module` trait exposed `initialise(&mut self, env)`
as a post-construction hook, but the construction itself (`new()` or similar) was
unconstrained and module-specific. There was no uniform way to instantiate a module from a
data-driven description — a prerequisite for hot-reload, serialisation, and the DSL
compilation pipeline described in ADR 0005.

**Parameters were stringly typed and unvalidated at the graph level.** Control signals used
`ControlSignal::Float { name, value: f32 }`. There was no declared schema for what parameters
a module accepted, no bounds checking, and no defaults — all validation was pushed into each
module's `receive_signal` implementation.

**The `ModuleGraph` could not be used as a serialisable config.** Because nodes were live
instances, the graph could not be serialised, diffed, or compared without running module
logic. Hot-reload required roundabout mechanisms to transfer state across plan rebuilds
(see ADR 0003).

### What the DSL pipeline needs

ADR 0005 describes a compilation pipeline that lowers a text patch description into an
`ExecutionPlan`. That pipeline needs to:

1. Validate port connections from a descriptor without instantiating modules.
2. Apply and validate typed parameters before deciding to instantiate anything.
3. Instantiate modules reproducibly from `(env, shape, params)` — the same inputs each
   time — so that a plan rebuild produces semantically identical modules to the ones it
   replaces, differing only in their `InstanceId`.

None of these are possible when the graph holds live instances and module construction is
freeform.

## Decision

### `ModuleGraph` becomes topology-only

`ModuleGraph` nodes are changed from `Box<dyn Module>` to:

```rust
pub struct Node {
    pub module_descriptor: ModuleDescriptor,
    pub parameter_map: ParameterMap,
}
```

The graph stores a static description of each module's port layout and its current parameter
values. No module instances are created during graph construction.

`add_module` now accepts a `ModuleDescriptor` and a `&ParameterMap`. Port validation in
`connect` reads from `node.module_descriptor` directly.

### `ModuleDescriptor` is enriched

The descriptor is extended to carry everything needed to describe a module fully at
compile time:

```rust
pub struct ModuleDescriptor {
    pub module_name: &'static str,
    pub shape: ModuleShape,          // e.g. channel count
    pub inputs: Vec<PortDescriptor>,
    pub outputs: Vec<PortDescriptor>,
    pub parameters: Vec<ParameterDescriptor>,
}
```

`ParameterDescriptor` names each parameter, gives it an index, and assigns a `ParameterKind`:

```rust
pub enum ParameterKind {
    Float { min: f32, max: f32, default: f32 },
    Int   { min: i64, max: i64, default: i64 },
    Bool  { default: bool },
    Enum  { variants: &'static [&'static str], default: &'static str },
}
```

All fields use `&'static str`; accessing a descriptor never allocates.

### `ParameterMap` replaces ad-hoc signals

Parameters are stored and communicated as a typed `ParameterMap` (`String → ParameterValue`
where `ParameterValue` is `Float(f32) | Int(i64) | Bool(bool) | Enum(&'static str)`).

`ControlSignal` is updated to `ParameterUpdate { name: &'static str, value: ParameterValue }`
to use the same typed value representation.

`validate_parameters` is a free function that checks a `ParameterMap` against a
`ModuleDescriptor`: unknown keys, wrong types, and out-of-bounds values are all rejected with
a structured `BuildError`.

### The `Module` trait gains a uniform construction protocol

The old `initialise` hook is replaced by three methods:

```rust
fn describe(shape: &ModuleShape) -> ModuleDescriptor;          // static; no instance needed
fn prepare(env: &AudioEnvironment, descriptor: ModuleDescriptor) -> Self; // allocate + store env
fn update_validated_parameters(&mut self, params: &ParameterMap) -> Result<(), BuildError>;
```

And a provided `build` method that composes them:

```rust
fn build(env: &AudioEnvironment, shape: &ModuleShape, params: &ParameterMap)
    -> Result<Self, BuildError>
{
    let descriptor = Self::describe(shape);
    let mut instance = Self::prepare(env, descriptor);
    // fill missing params from descriptor defaults, then validate and apply
    instance.update_parameters(&filled)?;
    Ok(instance)
}
```

`describe` is a static method: the descriptor for a module type can be obtained without
constructing an instance. This is what allows graph validation to be decoupled from
instantiation.

### `ModuleBuilder` enables data-driven construction

A new object-safe trait wraps a module type so it can be stored in a `Registry` and
invoked without knowing the concrete type:

```rust
pub trait ModuleBuilder: Send + Sync {
    fn describe(&self, shape: &ModuleShape) -> ModuleDescriptor;
    fn build(&self, env: &AudioEnvironment, shape: &ModuleShape, params: &ParameterMap)
        -> Result<Box<dyn Module>, BuildError>;
}
```

`Builder<T>(PhantomData<fn() -> T>)` is a zero-cost implementation for any `T: Module`.
The DSL compiler and the planner use `ModuleBuilder` to construct instances from a
`Registry` keyed by module name — the names appearing in the patch DSL map directly to
registry entries.

## Consequences

### Benefits

- **Graph validation is environment-free.** Port connections can be validated from descriptors
  alone; no audio environment or module instances are required. Tests, static analysis tools,
  and the DSL compiler can all validate graphs without a running audio engine.

- **Module construction is reproducible and data-driven.** Given the same `(env, shape, params)`,
  `build` always produces a semantically equivalent instance. The planner can safely rebuild
  a plan from scratch and match surviving instances by `InstanceId` without needing to transfer
  internal state.

- **Parameters are validated at the graph boundary.** Type errors, unknown keys, and
  out-of-range values are caught when a `ParameterMap` is applied to a descriptor — before
  any module is built. This surfaces errors earlier and removes defensive validation from
  individual module implementations.

- **The graph is a serialisable config.** Nodes contain only `&'static str`, `Vec`, and
  primitive values. A `ModuleGraph` can be serialised, compared, and diffed without running
  any module logic — a prerequisite for the DSL hot-reload pipeline.

- **`ModuleDescriptor` fields are compile-time constants.** All `&'static str` fields ensure
  descriptor access never allocates, consistent with the zero-cost descriptor desideratum.

### Costs

- **Module implementations must adopt the new protocol.** All existing modules must implement
  `describe`, `prepare`, and `update_validated_parameters` instead of the old `initialise`
  hook. Migration is mechanical but touches every module in `patches-modules`.

- **`ModuleGraph::add_module` API changes.** Callers that previously passed a constructed
  instance now pass a descriptor and parameter map. Any code that stored live instances in
  the graph must be updated.

- **`into_modules` → `into_nodes`.** The graph's consumption method is renamed and returns
  `HashMap<NodeId, Node>` rather than `HashMap<NodeId, Box<dyn Module>>`.

## Alternatives considered

### Keep instances in the graph; add a separate static descriptor method

A static `Module::describe_static(shape)` could provide a descriptor without requiring an
instance, while `add_module` continued to accept `Box<dyn Module>`. This avoids breaking
the consumption API but doesn't solve the serialisation problem (the graph still holds live
instances) and leads to duplication between static and instance-level descriptors. Rejected
in favour of a clean separation.

### Use a `ModuleSpec` enum rather than typed `ParameterMap`

A module-specific `enum` per parameter set would give strong types but requires upstream
code to know the concrete module type — defeating the purpose of `ModuleBuilder` and the
registry. `ParameterMap` with a validated schema gives comparable safety without coupling.

### Retain `initialise` and add `build` alongside it

Keeping `initialise` as a fallback would allow incremental migration. Rejected: two
construction hooks with overlapping responsibilities is confusing. The new three-method
protocol (`describe` / `prepare` / `update_validated_parameters`) is explicit about each
concern, and `build` provides the single entry point callers actually need.
