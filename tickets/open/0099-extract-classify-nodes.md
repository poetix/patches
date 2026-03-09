---
id: "0099"
title: Extract and test classify_nodes decision function
priority: high
created: 2026-03-08
epic: E019
depends-on: "0096"
---

## Summary

The core logic of `build_slots` — deciding what to do with each node in the
incoming graph — is currently tangled with buffer resolution, slot assembly, and
module instantiation. This ticket extracts it as a pure decision function and
gives it a focused test suite.

After this ticket, `build_patch` has a clear two-phase structure:

1. **Decision phase**: classify every node, resolve buffers, compute
   connectivity — no side effects.
2. **Action phase**: mint `InstanceId`s, call `registry.create`, allocate
   module pool slots, assemble slots and plan — no branching logic.

## Design

### NodeDecision enum

```rust
pub(crate) enum NodeDecision<'a> {
    /// Node is new or type/shape-changed. A fresh module must be instantiated.
    Install {
        module_name: &'static str,
        shape: &'a ModuleShape,
        params: &'a ParameterMap,
    },
    /// Node is surviving. The existing module stays; apply diffs if non-empty.
    Update {
        instance_id: InstanceId,
        param_diff: ParameterMap,       // empty if parameters unchanged
        connectivity_changed: bool,
    },
}
```

### classify_nodes function

```rust
fn classify_nodes<'a>(
    graph: &'a ModuleGraph,
    order: &[NodeId],
    edges: &[EdgeTuple],
    prev_state: &PlannerState,
) -> Result<Vec<(NodeId, NodeDecision<'a>)>, BuildError>
```

For each node in `order`:
- Look up `prev_state.nodes` to decide Install vs Update.
- For Update: compute the parameter diff and whether connectivity changed.
- For Install: capture the name, shape, and params needed for later
  instantiation.

Connectivity for each node is computed here (via `compute_connectivity` from
T-0098) so the diff against `prev_state` is available at classification time.

### Action phase

After `classify_nodes` returns, the action phase:
1. Calls `InstanceId::mint()` and `registry.create` for each `Install` node.
2. Collects all `InstanceId`s (minted + surviving) for `ModuleAllocState::diff`.
3. Assembles `ModuleSlot`s, `NodeState`s, and the `ExecutionPlan`.

`build_slots` is replaced entirely by this two-phase structure.

## Acceptance criteria

- [ ] `NodeDecision` enum defined as above (or equivalent).
- [ ] `classify_nodes` function extracted and used by `build_patch`.
- [ ] `build_patch` is visibly split into decision and action phases with a
      clear boundary (comment or helper function).
- [ ] Unit tests for `classify_nodes` — all constructable without a registry
      or module pool:
  - [ ] New node (not in prev_state) → `Install` with correct name/shape/params.
  - [ ] Type-changed node (same NodeId, different module_name) → `Install`.
  - [ ] Shape-changed node (same NodeId, same name, different shape) → `Install`.
  - [ ] Surviving node, no changes → `Update` with empty diff,
        `connectivity_changed: false`.
  - [ ] Surviving node, parameter changed → `Update` with non-empty param diff.
  - [ ] Surviving node, connectivity changed (edge added) → `Update` with
        `connectivity_changed: true`.
  - [ ] Surviving node, connectivity changed (edge removed) → `Update` with
        `connectivity_changed: true`.
  - [ ] Multiple nodes in one call: each classified independently.
- [ ] Existing integration tests in `patches-integration-tests` continue to
      pass unchanged.
- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.

## Notes

T-0096 is a hard dependency: `classify_nodes` produces `Install` decisions
without minting IDs, so the action phase must be able to mint them separately
before calling `registry.create`.

T-0097 and T-0098 can be done before or after this ticket; `classify_nodes`
calls `compute_connectivity` internally and the tests benefit from T-0098 being
independently verified first.
