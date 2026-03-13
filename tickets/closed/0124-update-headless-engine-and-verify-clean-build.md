---
id: "0124"
title: Update HeadlessEngine, test stubs; verify clean build
priority: high
created: 2026-03-12
epic: E023
depends-on: "0122, 0123"
---

## Summary

Update `patches-integration-tests` — `HeadlessEngine` and any local test stub
modules — to use the new `CablePool`-based API. Verify a clean build, test run,
and clippy pass across the entire workspace.

## Acceptance criteria

- [ ] `HeadlessEngine::tick` constructs `CablePool::new(&mut self.buffer_pool, self.wi)`
  and passes it to `ExecutionPlan::tick`.
- [ ] All test stub modules in `patches-integration-tests` updated to the new
  `Module::process` signature using `CablePool` methods.
- [ ] No references to `read_from`, `write_to`, `wi`, or `ri` remain anywhere
  outside of `CablePool` itself.
- [ ] `cargo build` clean across all crates.
- [ ] `cargo test` passes across all crates.
- [ ] `cargo clippy` clean across all crates.
