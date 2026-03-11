# ADR 0016 — MIDI as the sole external control mechanism

**Date:** 2026-03-11
**Status:** Accepted
**Supersedes:** [ADR 0008 — Control rate and MIDI/OSC architecture](0008-control-rate-and-midi-osc-architecture.md), [ADR 0014 — Control signals convey external events, not parameter updates](0014-control-signal-purpose.md)

## Context

ADR 0008 introduced a general-purpose `ControlSignal` / `receive_signal` mechanism
for delivering external control input to modules at a fixed 64-sample control rate.
ADR 0014 subsequently narrowed its intended use: `ControlSignal` was to carry only
raw MIDI/OSC events to dedicated receiver modules, not parameter updates.

During E021 (sample-accurate MIDI event pipeline) the implementation revealed
that the `ControlSignal` abstraction added no value over direct MIDI delivery.
MIDI is the only external control source in scope; OSC support has not been
pursued and has no concrete timeline. The `ControlSignal` ring buffer, the
`receive_signal` method on `Module`, and the `control_period` sub-block loop were
all separate from — and redundant with — the MIDI sub-block dispatcher built
in E021.

Keeping both mechanisms would mean maintaining two parallel dispatch paths with
overlapping responsibilities. The `ControlSignal` layer also required callers to
pre-target events at module `InstanceId`s, a binding concern better handled
inside dedicated MIDI receiver modules.

## Decision

The `ControlSignal` enum and `Module::receive_signal` method have been removed
entirely. OSC support is deferred indefinitely; if it is ever added it will be
designed from scratch to fit the architecture as it exists at that time.

MIDI is handled by a dedicated, purpose-built pipeline (E021) with the following
properties:

### Sub-block dispatch at 64-sample granularity

The audio callback processes its output buffer in 64-sample chunks. At each
chunk boundary it drains the `EventQueue` for events whose `target_sample` falls
within `[current_sample, current_sample + 64)` and delivers them to every module
listed in `ExecutionPlan::midi_receiver_indices`. Late events are clamped to
offset 0 of the current chunk.

The 64-sample quantum gives ≈1.33 ms granularity at 48 kHz — below the
perceptual JND for rhythmic timing in all practical musical contexts.

### Sample-accurate timestamps via seqlock `AudioClock`

The audio thread publishes a clock anchor after each output buffer:
`(sample_count, playback_wall_time)`. The anchor is written through a seqlock
so the MIDI connector thread can read it without blocking and without any
refcount operations on either side.

When a MIDI event arrives on the connector thread it computes:

```
target_sample = sample_count
              + (event_wall_time − playback_wall_time) × sample_rate
              + lookahead_samples
```

`lookahead_samples` (default 128, ≈2.7 ms at 48 kHz) absorbs thread-scheduling
jitter and ensures events arrive at the audio thread before their target
sub-block. This adds a fixed, bounded latency that is well below the perceptual
threshold.

### Opt-in via `ReceivesMidi` trait

`Module` does not grow a default `receive_midi` method. Instead a separate
`ReceivesMidi` trait carries `receive_midi(&mut self, offset: usize, event:
MidiEvent)`. Modules that want MIDI implement this trait and override
`Module::as_midi_receiver` to return `Some(self)`. The planner identifies such
modules during plan construction and records their pool indices in
`ExecutionPlan::midi_receiver_indices`. Only those modules are iterated by the
dispatcher.

### No `Arc` on the audio thread

`AudioCallback` holds a raw `*const AudioClock` pointer rather than an
`Arc<AudioClock>`. `SoundEngine` owns the `Arc` and is responsible for dropping
the stream (and thus the callback) before releasing it, keeping the pointer
valid for the entire lifetime of the callback. This avoids any refcount
operations on the audio thread.

## Consequences

**One dispatch path, not two.** The audio callback loop now contains only the
64-sample MIDI sub-block boundary. The previous `control_period` loop that
drained the `ControlSignal` ring buffer is gone.

**MIDI timing is sample-accurate in the standalone case.** The clock anchor and
lookahead mechanism resolve the problem described in ADR 0008 §5: wall-clock
timestamps from the connector thread are mapped to sample positions rather than
dispatched at the next block boundary.

**OSC is out of scope until there is a concrete use case.** If OSC support is
ever added, it will be treated as a first-class design exercise rather than an
extension of the removed `ControlSignal` path.

**Parameter changes still go through the planner.** The sole mechanism for
changing module parameters remains graph modification followed by re-planning,
as established in ADR 0014. Nothing in this decision changes that.

## Alternatives considered

### Retain `ControlSignal` for future OSC support

Keeping a general-purpose control signal path would preserve a hook for OSC
without requiring a redesign. Rejected because (a) OSC has no concrete timeline,
(b) maintaining dead code raises cognitive overhead, and (c) any future OSC
design would benefit from being matched to the architecture at the time rather
than retrofitted to a mechanism designed for an earlier, simpler model.

### Unify MIDI and `ControlSignal` in a single ring buffer

A shared tagged queue could carry both MIDI events and typed control signals.
Rejected because it mixes concerns (raw MIDI bytes vs. higher-level events),
complicates the audio-thread dispatch loop, and provides no benefit while MIDI
is the only source.
