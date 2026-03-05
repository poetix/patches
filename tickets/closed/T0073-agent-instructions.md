# Agent instructions for T-0073: PlannerState and graph-diffing build_patch

## Context

You are on branch `modulev2`. Three prerequisite tickets (T-0070, T-0071, T-0072) have
already been committed:

- T-0070: `update_validated_parameters` is now infallible (returns `()`)
- T-0071: `ModuleDescriptor` has `pub is_sink: bool`
- T-0072: `SoundEngine` has two-phase startup (`open()` → `start(plan)`)

The file `patches-engine/src/builder.rs` has **pre-existing compilation errors** because
it still uses v1 `ModuleGraph` methods (`get_module`, `into_modules`). Your job is to
**rewrite `build_patch`** so it uses the v2 graph API. This will fix those errors.

Read the ADR at `adr/0012-planner-v2-graph-diffing.md` for the full design rationale.

## What to implement

### 1. New types: `NodeState` and `PlannerState`

Add to `patches-engine/src/builder.rs` (or a new `patches-engine/src/planner_state.rs`
if cleaner, re-exported from `lib.rs`):

```rust
pub struct NodeState {
    pub module_name: &'static str,
    pub instance_id: InstanceId,
    pub pool_index: usize,
    pub parameter_map: ParameterMap,
}

pub struct PlannerState {
    pub nodes: HashMap<NodeId, NodeState>,
    pub buffer_alloc: BufferAllocState,
    pub module_alloc: ModuleAllocState,
    next_instance_id: u64,
}
```

`PlannerState` needs:
- `PlannerState::empty()` — returns an empty state (no nodes, default allocs, counter at 0)
- `PlannerState::next_id(&mut self) -> InstanceId` — assigns the next InstanceId. You
  can use `InstanceId::next()` (it's already atomic) or track your own counter.

### 2. Rewrite `build_patch`

New signature:

```rust
pub fn build_patch(
    graph: &ModuleGraph,           // BORROWED, not consumed
    registry: &Registry,           // for instantiating new modules
    env: &AudioEnvironment,        // for Module::build()
    prev_state: &PlannerState,     // previous build state (empty for first build)
    pool_capacity: usize,
    module_pool_capacity: usize,
) -> Result<(ExecutionPlan, PlannerState), BuildError>
```

The function:

1. **Iterates** `graph.node_ids()` and for each node, calls `graph.get_node(id)` to get
   the `Node` (which has `.module_descriptor` and `.parameter_map`).

2. **Diffs** each node against `prev_state.nodes`:
   - **New node** (not in prev_state): assign `InstanceId` (via `InstanceId::next()`),
     instantiate via `registry.create(module_name, env, &descriptor.shape, &param_map)`,
     add to `new_modules`.
   - **Surviving node** (in prev_state, same `module_name`): reuse `InstanceId` and
     `pool_index` from prev_state. Do NOT instantiate. Do NOT add to `new_modules`.
   - **Type-changed node** (in prev_state, different `module_name`): tombstone old
     pool slot, assign new `InstanceId`, instantiate new module, add to `new_modules`.
   - **Removed node** (in prev_state but not in new graph): tombstone the old pool slot.

3. **Identifies the sink** node via `node.module_descriptor.is_sink` (NOT via `as_sink()`).
   Exactly one sink required (error otherwise).

4. **Allocates buffers** using `BufferAllocState` (same logic as current code — reuse for
   same `(NodeId, port_index)` keys, freelist for removed, hwm for new).

5. **Allocates module pool slots** using `ModuleAllocState::diff()` — the existing code
   already handles this; adapt it to work with the new InstanceId assignment.

6. **Builds** `ModuleSlot` entries in execution order (ascending `NodeId`).

7. **Returns** `(ExecutionPlan, new_PlannerState)`.

### 3. Key differences from the current code

The current `build_patch`:
- Takes `graph: ModuleGraph` (owned, consumed)
- Calls `graph.get_module(id)` → returns `&dyn Module` (v1 API, doesn't exist anymore)
- Calls `graph.into_modules()` → returns `HashMap<NodeId, Box<dyn Module>>` (v1, doesn't exist)
- Uses `m.instance_id()` and `m.as_sink()` on live module instances
- Every node gets a fresh module instance; surviving ones are just dropped

The new `build_patch`:
- Takes `graph: &ModuleGraph` (borrowed)
- Calls `graph.get_node(id)` → returns `Option<&Node>` where `Node` has `module_descriptor` and `parameter_map`
- Uses `node.module_descriptor.is_sink` to find the sink
- Uses `node.module_descriptor.module_name` and comparison with `prev_state` to determine new/surviving/type-changed
- Only instantiates genuinely new modules via `registry.create()`
- `InstanceId` comes from the planner, not from module construction

### 4. Update `Planner` struct

In `patches-engine/src/planner.rs`, update `Planner` to hold `PlannerState` instead of
separate `BufferAllocState` + `ModuleAllocState`. Its `build` method should delegate to
the new `build_patch`.

The new `Planner::build` needs `&Registry` and `&AudioEnvironment` parameters (or the
`Planner` holds references/values for them). The simplest approach: change `Planner::build`
to take `(graph: &ModuleGraph, registry: &Registry, env: &AudioEnvironment)`.

### 5. Update `PatchEngine`

`PatchEngine` needs to pass `Registry` and `AudioEnvironment` through to `Planner::build`.
Currently `PatchEngine::start()` calls `self.engine.open()` to get `AudioEnvironment`.
It should store this and pass it to `planner.build()`.

`PatchEngine` needs to hold a `Registry`. Its constructor should accept one. But note:
the current `PatchEngine::new(graph)` doesn't take a registry. You'll need to change
the API. The v2 graph uses `ModuleDescriptor` + `ParameterMap` nodes, not `Box<dyn Module>`.

The new `PatchEngine` API should be:
```rust
PatchEngine::new(registry: Registry) -> Result<Self, PatchEngineError>
PatchEngine::start(&mut self, graph: &ModuleGraph) -> Result<(), PatchEngineError>
PatchEngine::update(&mut self, graph: &ModuleGraph) -> Result<(), PatchEngineError>
```

### 6. Update `BuildError`

Add a variant for registry errors:
```rust
pub enum BuildError {
    // ... existing variants ...
    /// Module creation failed (unknown module name or parameter validation error).
    ModuleCreationError(String),
}
```

Or re-export `patches_core::build_error::BuildError` — check if it already has what you
need. The core `BuildError` has `UnknownModule { name: String }` and
`InvalidParameter { ... }`. You may need to bridge between the core and engine errors.

### 7. Update tests

The existing tests in `builder.rs` use v1 constructors (`SineOscillator::new(440.0)`,
`AudioOut::new()`, `Box::new(...)`) and the old `build_patch` signature. Rewrite them to:

- Use the v2 graph API: `graph.add_module(id, descriptor, &param_map)` where `descriptor`
  comes from `Module::describe(&shape)` and `param_map` is a `ParameterMap`.
- Use `default_registry()` from `patches-modules` for the registry.
- Pass `&AudioEnvironment`, `&PlannerState::empty()` to `build_patch`.

Helper for building a simple graph:
```rust
use patches_modules::default_registry;

fn sine_to_audio_out_graph() -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let sine_desc = patches_modules::SineOscillator::describe(&ModuleShape { channels: 0 });
    let out_desc = patches_modules::AudioOut::describe(&ModuleShape { channels: 0 });
    let mut params = ParameterMap::new();
    params.insert("frequency".to_string(), ParameterValue::Float(440.0));
    graph.add_module("a_sine", sine_desc, &params).unwrap();
    graph.add_module("b_out", out_desc, &ParameterMap::new()).unwrap();
    graph.connect(&NodeId::from("a_sine"), p("out"), &NodeId::from("b_out"), p("left"), 1.0).unwrap();
    graph.connect(&NodeId::from("a_sine"), p("out"), &NodeId::from("b_out"), p("right"), 1.0).unwrap();
    graph
}
```

Add new tests for diffing behaviour:
- **New node**: build from empty state, verify all modules in `new_modules`
- **Surviving node**: build twice with same graph, verify no `new_modules` on second build, same `InstanceId`s
- **Removed node**: build with 2 nodes, then 1 node, verify tombstone for removed
- **Type-changed node**: build with SineOscillator at a NodeId, then SawtoothOscillator at same NodeId, verify tombstone + new module

Also update the tests in `planner.rs` similarly. The `Planner` tests use `simple_graph()`
which constructs modules directly — these need to use the v2 graph API.

### 8. Update examples

Update `patches-engine/examples/sine_tone.rs` to use the new API:
```rust
let registry = default_registry();
let mut engine = SoundEngine::new(4096, 1024, 64)?;
let env = engine.open()?;

let graph = /* build graph using v2 API */;
let (plan, _state) = build_patch(&graph, &registry, &env, &PlannerState::empty(), 4096, 1024)?;
engine.start(plan)?;
```

Update the other examples similarly (`chord_swap`, `demo_synth`, `freq_sweep`).

### 9. Update `patches-engine/src/lib.rs` exports

Make sure `PlannerState`, `NodeState`, and any new public types are exported.

### 10. Verification

Run `cargo clippy` and `cargo test` across ALL crates. Everything must be clean.
The pre-existing builder.rs compilation errors should be GONE after your rewrite.

## Important files to read first

1. `adr/0012-planner-v2-graph-diffing.md` — the design
2. `patches-engine/src/builder.rs` — the current build_patch (to be rewritten)
3. `patches-engine/src/planner.rs` — Planner and PatchEngine (to be updated)
4. `patches-core/src/graph.rs` — v2 ModuleGraph API (Node, get_node, node_ids, edge_list)
5. `patches-core/src/registry.rs` — Registry::create()
6. `patches-core/src/module.rs` — Module trait (describe, prepare, update_validated_parameters)
7. `patches-modules/src/lib.rs` — default_registry() and module types

## Constraints

- No `unwrap()` or `expect()` in library code
- The graph must be BORROWED (`&ModuleGraph`), not consumed
- Do not add new Cargo dependencies
- Keep `BufferAllocState` and `ModuleAllocState` — they still do the buffer/slot allocation
- `ModuleAllocState::diff()` still works; adapt it to use planner-assigned InstanceIds
- Run `cargo clippy` and `cargo test` before considering done
