---
id: "T-0052"
title: Redirect tombstone drops to cleanup channel
priority: medium
created: 2026-03-03
---

## Summary

Change the tombstone loop in the audio callback (currently in `build_stream` in
`patches-engine/src/engine.rs`) so that tombstoned `Box<dyn Module>` values are sent to
the cleanup ring buffer rather than dropped inline. Update the `SoundEngine` doc comment
to reflect the new step ordering.

## Acceptance criteria

- [ ] The tombstone loop no longer drops `Box<dyn Module>` on the audio thread.
      The new loop body is: take the module from the pool; push it to `cleanup_tx`; if
      `push` returns `Err` (ring buffer full), drop the module and log via `eprintln!` as a
      last resort (see Notes).
- [ ] The `SoundEngine` struct-level doc comment step 2 is updated from
      "Takes tombstoned modules out of the pool (dropping them)." to
      "Sends tombstoned modules to the cleanup ring buffer for off-thread deallocation."
- [ ] No `Box<dyn Module>` is constructed or dropped inside the audio callback other than
      via the fallback path.
- [ ] `cargo build`, `cargo test`, `cargo clippy` all pass.

## Notes

### Fallback log message

When the fallback fires, print a message that makes the RT violation visible:

```
eprintln!("patches: cleanup ring buffer full — dropping module on audio thread (slot {idx})");
```

This should not happen under normal usage. See `adr/0010` for the sizing analysis.

### Ordering constraint preserved

The tombstone loop must still run **before** installing `new_modules`, because the freelist
may have recycled a tombstoned slot for a new module. The ordering is:
1. Send tombstoned modules to cleanup channel (clear pool slots).
2. Install `new_modules` into pool slots.
3. Zero `to_zero` cable buffers.
4. Replace `current_plan`.

This is the same ordering as before — only the disposal of the taken value changes.

### `rtrb::PushError`

`rtrb::Producer::push` returns `Err(rtrb::PushError::Full(value))`. Destructure to recover
the `Box<dyn Module>` for the fallback drop:

```rust
if let Err(rtrb::PushError::Full(module)) = cleanup_tx.push(module) {
    eprintln!("patches: cleanup ring buffer full — dropping module on audio thread (slot {idx})");
    drop(module);
}
```
