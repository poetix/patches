---
id: "0070"
title: Make update_validated_parameters infallible
priority: high
epic: "E014"
created: 2026-03-04
---

## Summary

Change `Module::update_validated_parameters` from returning `Result<(), BuildError>` to
returning nothing. The method receives pre-validated parameters — there is no meaningful
error to return. This removes boilerplate `Ok(())` from every module implementation and
eliminates the awkwardness of calling a `Result`-returning method in the audio callback
(where errors cannot be meaningfully handled).

## Acceptance criteria

- [ ] `Module::update_validated_parameters(&mut self, params: &ParameterMap)` returns `()`
      instead of `Result<(), BuildError>`.
- [ ] The default `Module::update_parameters` wrapper still returns `Result<(), BuildError>`
      (validation can fail), but delegates to the now-infallible inner method.
- [ ] The default `Module::build` still returns `Result<Self, BuildError>` (validation in
      `update_parameters` can fail at construction time).
- [ ] All module implementations in `patches-modules` updated: remove `-> Result<(), BuildError>`
      and trailing `Ok(())`.
- [ ] All call sites in `patches-engine` that currently `.unwrap()` or `?` the result of
      `update_validated_parameters` are simplified (the call is now infallible).
- [ ] `cargo clippy` and `cargo test` clean across all crates.

## Notes

This is a mechanical change that touches every module implementation but requires no
logic changes. The `update_parameters` → `update_validated_parameters` delegation still
validates first and returns `Result` to external callers; only the inner "already
validated" method becomes infallible.

See ADR 0012 § "update_validated_parameters becomes infallible".
