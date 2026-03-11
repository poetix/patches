# ADR 0008 — Control rate and MIDI/OSC architecture

## Status

Superseded by [ADR 0016 — MIDI as the sole external control mechanism](0016-midi-only-control-architecture.md)

## Context

Patches needs to accept real-time control input — MIDI and OSC — and propagate
those values into the audio graph as control signals. Two deployment targets are
in scope: a standalone application (CPAL audio backend) and a future VST plugin
(host-driven audio callback). These two targets differ significantly in how
control events are delivered.

**MIDI** events arrive with sample-accurate timestamps relative to an audio
block when a VST host provides them, or as wall-clock-timestamped callbacks from
`midir` on a separate thread in the standalone case.

**OSC** events arrive as UDP packets on a network thread, with no timing
guarantees whatsoever, and no concept of sample-accurate delivery.

A naive approach of updating control values once per audio block is insufficient
at typical block sizes: at 512 samples and 44 100 Hz the update rate is ~86 Hz,
below the ~100 Hz threshold where continuous controllers (pitch bend, mod wheel)
exhibit audible zipper noise.

## Decision

### 1. Fixed 64-sample control-rate interval

The audio block is subdivided into 64-sample *control slices*, independent of
the configured audio block size. Control-rate state is updated once per slice.
At 44 100 Hz this gives ~689 Hz — above the zipper-noise threshold for
continuous controllers at all practical block sizes.

The 64-sample quantum is fixed, not configurable. It is a well-established
convention (SuperCollider, Pd/Max) and making it a runtime parameter adds
complexity for no practical gain.

### 2. Control events are pre-targeted at specific modules

Each control event in the ring buffer carries a target module identifier
alongside its value. The binding from raw MIDI/OSC input to a specific module
and parameter is resolved *before* the event enters the ring buffer, by the
caller (the binding/routing layer on the control thread). The audio thread
simply applies the value to the named target — it performs no dispatch logic.

The MIDI/OSC binding layer is out of scope for this ADR. In a first
implementation there may be a single module that receives all inbound MIDI
events (note on/off, mod wheel, etc.) and converts them into audio-rate signals
sent via cables to downstream synthesis modules. Over time, bindings may become
more granular — for example, wiring a physical knob on a control surface
directly to an internal parameter of a specific module. Either way, the
resolution of "which module does this event target" happens on the control
thread, not the audio thread.

### 3. Unified control signal handoff via lock-free ring buffer

All external control sources — MIDI (standalone), OSC, and any future sources —
write into a lock-free ring buffer (one per source type, or a shared tagged
queue). The audio thread drains this buffer at each 64-sample slice boundary.

This decouples the delivery mechanism from the consumption mechanism and
satisfies the no-blocking, no-allocation constraint on the audio thread.

### 4. Engine loop subdivides blocks into control slices

`SoundEngine::process()` (or equivalent) iterates over 64-sample slices within
each audio block. At each slice boundary it:

1. Drains incoming control events from the ring buffer(s).
2. Updates control-rate module state.
3. Runs the audio graph for that slice.

The audio block size remains a pure latency/CPU trade-off for the CPAL or host
layer and no longer affects control resolution.

### 5. MIDI event sources differ by deployment target

| Target | MIDI source | Timing |
|---|---|---|
| Standalone (CPAL) | `midir` callback thread → ring buffer | Wall-clock, best-effort |
| VST plugin | Host-provided buffer in audio callback | Sample-accurate offsets |

In the standalone case, the `midir` callback pushes raw MIDI bytes plus a
wall-clock timestamp into the ring buffer. The audio thread dispatches these at
the next 64-sample slice boundary — equivalent to block-boundary precision.

In the VST case, the host supplies a MIDI buffer alongside the audio buffer,
with each event carrying a sample offset within the block. The engine maps
each event to the appropriate 64-sample slice.

Both targets feed the same internal control signal mechanism; they differ only
in the ring buffer producer and in timestamp precision.

### 6. OSC is always treated as a control-rate signal

OSC packets carry no sample-accurate timing and cannot be made to do so. A
network/listener thread receives UDP packets, parses OSC messages, and writes
target values into atomics or a ring buffer. The audio thread reads these at
each 64-sample slice boundary. Per-sample smoothing (a one-pole lowpass on each
control input) eliminates residual zipper noise from the coarse update rate.

### 7. Engine abstraction supports both standalone and VST

The core engine is backend-agnostic: it exposes a `tick` interface that accepts
an audio output buffer and a slice of `TimedMidiEvent` (carrying a sample
offset). The standalone binary and the future VST shell each provide their own
producer side, feeding the same core.

```
// Conceptual interface (not final API)
fn tick(&mut self, audio_out: &mut [f32], midi_events: &[TimedMidiEvent])
```

This mirrors the existing principle that `patches-core` is backend-agnostic,
extending the same separation to the engine layer.

## Consequences

**Control resolution is decoupled from block size.** A 512-sample block is
processed as eight 64-sample slices. Configuring a larger block size for lower
CPU overhead does not degrade control quality.

**MIDI timing in standalone is block-boundary precision, not sample-accurate.**
Wall-clock timestamps from `midir` cannot be mapped to exact sample offsets
without additional infrastructure (correlating the audio clock with the system
clock). This is acceptable for the standalone use case; sample-accurate MIDI
dispatch is available in the VST path via host-supplied timestamps.

**OSC and standalone MIDI share the same handoff pattern.** Both use the
lock-free ring buffer / control-rate polling model, simplifying the audio
thread's event consumption code.

**Per-sample smoothing is recommended for continuous controllers.** Even at
~689 Hz, abrupt value changes can produce faint zipper noise on high-frequency
parameters. A one-pole lowpass applied in each module to its control inputs
eliminates this and is essentially free.

**VST support requires a separate engine wrapper.** `patches-engine` will be
split: a backend-agnostic core that implements the 64-sample slice loop, and
separate standalone (CPAL + midir) and VST integration crates that provide the
producer side.

## Alternatives considered

**Block-rate control (one update per audio block).** Rejected because the
effective rate is too low at larger block sizes (86 Hz at 512 samples / 44 100
Hz), causing audible zipper noise on continuous controllers.

**Audio-rate control signals.** Unnecessary for MIDI/OSC sources and expensive
to compute. Audio-rate modulation within the patch graph (e.g. LFO → filter
cutoff) already works via the normal cable mechanism at audio rate; this ADR
concerns only externally-sourced control signals.

**Configurable control rate.** Rejected in favour of a fixed 64-sample quantum.
Configurability adds API surface and build complexity for no practical benefit —
64 samples is an established, well-understood choice.

**Separate handling per source type.** Rejected in favour of a unified
lock-free handoff model for OSC and standalone MIDI. Unifying the consumption
path on the audio thread reduces complexity.
