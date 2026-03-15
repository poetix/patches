---
id: "0041"
epic: "E008"
title: Example — frequency sweep via control signals
priority: medium
created: 2026-03-02
depends_on: ["0037", "0038"]
---

## Summary

Add a `freq_sweep` example to `patches-engine/examples/` that demonstrates
control-rate signalling end-to-end. A single `SineOscillator` is wired to
`AudioOut`; the control thread sends `ControlSignal::Float { name: "freq", value }`
calls via `PatchEngine::send_signal` to sweep the oscillator's pitch from 110 Hz
up to 880 Hz and back, without ever rebuilding the graph. The example provides
audible proof that parameter changes arrive at the module and take effect cleanly
mid-stream.

## Acceptance criteria

- [ ] `patches-engine/examples/freq_sweep.rs` compiles with
      `cargo build --example freq_sweep`.
- [ ] Running `cargo run --example freq_sweep` opens the default audio device,
      plays an audibly rising then falling pitch sweep over ~4 seconds, and exits
      cleanly.
- [ ] The example captures the oscillator's `InstanceId` at graph construction
      time and uses it in every `send_signal` call — demonstrating the
      `InstanceId`-addressed dispatch model.
- [ ] The control loop runs on the main thread and calls `send_signal` at roughly
      50–100 Hz (i.e. `thread::sleep(Duration::from_millis(10..=20))` between
      steps). The frequency step size is chosen so the full sweep completes in
      approximately 2 seconds up and 2 seconds down.
- [ ] `send_signal` failures (ring buffer full) are handled: print a warning and
      continue rather than panicking or propagating as a fatal error.
- [ ] No `unwrap`/`expect` in the example outside of code comments — errors
      propagated via `?` and printed in `main`.
- [ ] `cargo clippy` clean.

## Notes

**Graph structure (sketch):**
```rust
let osc_id = {
    let osc = SineOscillator::new(110.0);
    let id = osc.instance_id();
    graph.add_module("osc", Box::new(osc))?;
    id
};
graph.add_module("out", Box::new(AudioOut::new()))?;
graph.connect(&"osc".into(), PortRef { name: "out", index: 0 },
              &"out".into(), PortRef { name: "left", index: 0 }, 1.0)?;
// … right channel …
```

**Control loop (sketch):**
```rust
// Rise: 110 → 880 Hz in N steps
for step in 0..N {
    let freq = 110.0 * (880.0_f32 / 110.0).powf(step as f32 / N as f32);
    if let Err(_) = engine.send_signal(osc_id, ControlSignal::Float { name: "freq", value: freq }) {
        eprintln!("warning: signal buffer full, skipping step");
    }
    thread::sleep(Duration::from_millis(20));
}
// Fall: 880 → 110 Hz (mirror loop)
```

Using an exponential step (`powf`) gives a perceptually linear pitch sweep in
musical terms (each step covers the same number of semitones). A linear step would
sound like it crawls at the bottom and races at the top.

**This example supersedes `chord_swap` as the primary hot-parameter demo.** Both
examples should remain in the repository; `chord_swap` demonstrates plan swapping,
`freq_sweep` demonstrates in-place parameter control.
