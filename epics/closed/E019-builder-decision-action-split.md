---
id: "E019"
title: Builder refactor — separate decision phase from action phase
created: 2026-03-08
tickets: ["0096", "0097", "0098", "0099", "0100", "0101"]
---

## Summary

`build_slots` in `patches-engine/src/builder.rs` is a ~175-line method that
interleaves six concerns: input buffer resolution, connectivity computation,
module classification, parameter diffing, module instantiation, and slot
assembly. The logic is hard to follow and nearly impossible to unit-test without
a full graph and registry.

This epic restructures the builder into two clean phases:

1. **Decision phase** — pure functions over graph data and `PlannerState`.
   Determines what will change: which nodes are new/type-changed (and what
   they need), which are surviving (and what diffs to apply), what buffers to
   assign, what connectivity each node has. No side effects.

2. **Action phase** — mechanical translation of decisions into the new plan.
   Mints `InstanceId`s, calls `registry.create`, allocates module pool slots,
   assembles `ModuleSlot`s and `NodeState`s.

The chief goal is a rich, focused test suite for the decision phase: small,
readable tests that pin each classification and diffing rule without any
registry, CPAL, or module instantiation machinery.

A prerequisite structural change — making `InstanceId` assignable externally
rather than auto-minted in the module constructor — is handled as its own
ticket because it touches every module implementation.

## Tickets

| ID   | Title                                                                | Priority | Depends on |
|------|----------------------------------------------------------------------|----------|------------|
| 0096 | Externally-assigned `InstanceId`: remove auto-mint from constructors | high     | —          |
| 0097 | Extract `resolve_input_buffers` and `partition_inputs`               | medium   | —          |
| 0098 | Extract `compute_connectivity`                                       | medium   | —          |
| 0099 | Extract and test `classify_nodes` decision function                  | high     | 0096       |
| 0100 | Build edge lookup caches to eliminate O(N·E) scans                  | medium   | 0099       |
| 0101 | Encapsulate edge lookup caches in `GraphIndex` / `ResolvedGraph`     | low      | 0100       |

## Definition of done

- `build_slots` is replaced by a set of named, focused functions.
- The decision phase (node classification, buffer resolution, connectivity
  computation) consists entirely of pure or near-pure functions with no
  registry or module-pool dependencies.
- Each decision-phase function has unit tests exercising its cases directly,
  without constructing a full plan or touching the registry.
- The action phase (instantiation, slot assembly, plan construction) is a
  thin, obviously-correct pass over the decisions.
- `cargo build`, `cargo test`, `cargo clippy` clean.
- No `unwrap()` or `expect()` in library code.
