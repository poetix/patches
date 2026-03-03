---
id: "T-0053"
title: "Integration test: tombstoned modules dropped off the audio thread"
priority: medium
created: 2026-03-03
---

## Summary

Add tests that verify the mechanism introduced in T-0046 and T-0047 is correct: a module
tombstoned during a plan swap must be dropped on the `"patches-cleanup"` thread, not on
the audio callback thread. Two tests are required — one that exercises the channel and
thread in isolation, and one end-to-end test that runs through `SoundEngine`.

## Acceptance criteria

- [ ] **Unit test** in `patches-engine/src/engine.rs` (or a submodule): create the cleanup
      channel and thread directly (no CPAL), push a `Box<dyn Module>` value to the producer,
      drop the producer to signal abandonment, join the thread, and assert that the module
      was dropped on a thread whose name is `"patches-cleanup"`.
- [ ] **Integration test** in `patches-integration-tests/tests/off_thread_deallocation.rs`
      marked `#[ignore]` (requires audio hardware): create a `SoundEngine`, start it, push a
      plan containing a `ThreadIdDropSpy` module, push a second plan that removes the spy,
      stop the engine, and assert the recorded drop thread name is `"patches-cleanup"` and
      not the audio callback thread name.
- [ ] A `ThreadIdDropSpy` module helper is defined (either locally in the test file, or in
      a shared test-helper module if one is introduced). Its `Drop` impl records
      `std::thread::current().name().map(str::to_owned)` into a shared
      `Arc<Mutex<Option<String>>>`.
- [ ] `cargo test` passes (the `#[ignore]` test is excluded by default).
- [ ] `cargo clippy` passes with no warnings.

## Notes

### `ThreadIdDropSpy` sketch

```rust
struct ThreadIdDropSpy {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    drop_thread: Arc<Mutex<Option<String>>>,
}

impl Drop for ThreadIdDropSpy {
    fn drop(&mut self) {
        let name = std::thread::current().name().map(str::to_owned);
        *self.drop_thread.lock().unwrap() = name;
    }
}
```

### Cleanup thread name

The cleanup thread is spawned with `std::thread::Builder::new().name("patches-cleanup")`
(established in T-0046). Asserting `drop_thread_name == Some("patches-cleanup".to_owned())`
is sufficient; there is no need to compare `ThreadId` values.

### Unit test strategy

The unit test should not open a CPAL stream. The cleanup thread and ring buffer can be
instantiated and exercised entirely within the test:

```rust
#[test]
fn tombstoned_module_dropped_on_cleanup_thread() {
    let drop_thread: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let spy = Box::new(ThreadIdDropSpy::new(Arc::clone(&drop_thread)));
    let (mut tx, rx) = rtrb::RingBuffer::<Box<dyn Module>>::new(16);

    let handle = std::thread::Builder::new()
        .name("patches-cleanup".to_owned())
        .spawn(move || {
            let mut rx = rx;
            loop {
                while let Ok(module) = rx.pop() {
                    drop(module);
                }
                if rx.is_abandoned() { break; }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
        .unwrap();

    tx.push(spy).unwrap();
    drop(tx); // abandon producer → cleanup thread exits
    handle.join().unwrap();

    let recorded = drop_thread.lock().unwrap().clone();
    assert_eq!(recorded, Some("patches-cleanup".to_owned()));
}
```

### `#[ignore]` end-to-end test

The end-to-end test requires a default audio output device. Keep it `#[ignore]` so CI
passes on headless machines. Run it locally with:

```
cargo test -p patches-integration-tests -- --ignored off_thread_deallocation
```

The test must wait long enough after the second `swap_plan` for the audio callback to
adopt the plan (one buffer period, ~10 ms at 48 kHz with a 512-frame buffer). A
`std::thread::sleep(Duration::from_millis(50))` before `stop()` is sufficient.
