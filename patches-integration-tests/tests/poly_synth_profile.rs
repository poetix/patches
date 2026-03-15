/// Profiling integration test for the poly_synth patch.
///
/// Builds the poly_synth.yaml patch via the Planner and times 2 seconds of
/// audio at various voice counts, with and without MIDI, to identify scaling
/// behaviour and MIDI dispatch overhead.
///
/// Run with:
///   cargo test -p patches-integration-tests --release poly_synth_headless_profile -- --nocapture
use std::time::{Duration, Instant};

use patches_core::{AudioEnvironment, MidiEvent};
use patches_engine::Planner;
use patches_integration_tests::HeadlessEngine;

// ── Constants ─────────────────────────────────────────────────────────────────

const SAMPLE_RATE: f32 = 44_100.0;
const DURATION_SECS: f32 = 2.0;
const SAMPLE_COUNT: usize = (SAMPLE_RATE * DURATION_SECS) as usize; // 88 200
const BUFFER_CAP: usize = 4096;
const MODULE_CAP: usize = 64;
const WARMUP_SAMPLES: usize = 1024;

// ── MIDI helpers ──────────────────────────────────────────────────────────────

fn note_on(note: u8, vel: u8) -> MidiEvent {
    MidiEvent { bytes: [0x90, note, vel] }
}

fn note_off(note: u8) -> MidiEvent {
    MidiEvent { bytes: [0x80, note, 0] }
}

fn cc(ctrl: u8, val: u8) -> MidiEvent {
    MidiEvent { bytes: [0xB0, ctrl, val] }
}

fn pitch_bend(cents: i32) -> MidiEvent {
    let raw = ((cents.clamp(-200, 200) + 200) as u32 * 16383 / 400) as u16;
    MidiEvent { bytes: [0xE0, (raw & 0x7F) as u8, ((raw >> 7) & 0x7F) as u8] }
}

// ── MIDI schedule ─────────────────────────────────────────────────────────────

/// Build a per-sample MIDI event schedule scaled to `n_voices`.
///
/// The note-firing period is chosen so that all `n_voices` voices become active
/// within the first 400 ms and remain active (via LIFO stealing) for the full
/// duration, keeping ADSR state machines and oscillator V/oct paths hot.
///
/// Each note is held for `note_dur` samples before release; the next note fires
/// `note_period = note_dur / n_voices` samples after the previous one, so by
/// `t = note_dur` all voice slots are occupied. Notes cycle through a 24-pitch
/// chromatic pool so V/oct values vary constantly.
///
/// Additional activity:
/// - **Mod-wheel sweep** (CC 1): 0 → 127 linearly over the 2 s window.
/// - **Pitch-bend sweep**: ±200 cents sinusoidal at 1 Hz, sampled every 25 ms.
fn build_midi_schedule(n_samples: usize, n_voices: usize) -> Vec<Vec<MidiEvent>> {
    let mut schedule: Vec<Vec<MidiEvent>> = (0..n_samples).map(|_| Vec::new()).collect();

    let mut step = |s: usize, ev: MidiEvent| {
        if s < n_samples {
            schedule[s].push(ev);
        }
    };

    // 24 chromatic pitches (C3–B4) cycling, so V/oct changes on every note-on.
    let note_pool: Vec<u8> = (48u8..72).collect();

    // note_dur: long enough to traverse attack+decay and sit in sustain.
    // note_period: tight enough that all voices fill up within note_dur.
    let note_dur    = (SAMPLE_RATE * 0.40) as usize;
    let note_period = (note_dur / n_voices).max(265); // ≥ 6 ms between events
    let mut i = 0usize;
    let mut t = 0usize;
    while t < n_samples {
        let note = note_pool[i % note_pool.len()];
        let vel  = 80 + ((i * 7) % 48) as u8;
        step(t, note_on(note, vel));
        step(t + note_dur, note_off(note));
        i += 1;
        t += note_period;
    }

    // Mod-wheel: 0 → 127 over the full window; one event per ~690 samples.
    let cc_period = (n_samples / 128).max(1);
    for v in 0u8..=127 {
        step(v as usize * cc_period, cc(1, v));
    }

    // Pitch-bend: ±200 cents sinusoidal at 1 Hz, sampled every 25 ms.
    let bend_period = (SAMPLE_RATE * 0.025) as usize;
    let bend_steps  = n_samples / bend_period;
    for i in 0..bend_steps {
        let radians = (i as f32) * 2.0 * std::f32::consts::PI / (1.0 / 0.025);
        let cents   = (radians.sin() * 200.0).round() as i32;
        step(i * bend_period, pitch_bend(cents));
    }

    schedule
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn load_yaml() -> &'static str {
    include_str!("../../examples/poly_synth.yaml")
}

struct RunResult {
    elapsed:  Duration,
    last_l:   f32,
    last_r:   f32,
}

/// Run the poly_synth with `poly_voices` voices and a voice-count-scaled MIDI
/// schedule so that all voice slots are active and ADSR state machines are hot.
fn run_config(poly_voices: usize) -> RunResult {
    let yaml     = load_yaml();
    let registry = patches_modules::default_registry();
    let graph    = patches_core::graph_yaml::yaml_to_graph(yaml, &registry)
        .expect("poly_synth.yaml parse failed");

    let env = AudioEnvironment { sample_rate: SAMPLE_RATE, poly_voices };
    let mut planner = Planner::new();
    let plan = planner.build(&graph, &registry, &env).expect("plan build failed");

    let mut engine = HeadlessEngine::new(plan, BUFFER_CAP, MODULE_CAP);

    for _ in 0..WARMUP_SAMPLES {
        engine.tick();
    }

    let schedule = build_midi_schedule(SAMPLE_COUNT, poly_voices);

    let t0 = Instant::now();
    for events in &schedule {
        for &ev in events {
            engine.send_midi(ev);
        }
        engine.tick();
    }

    RunResult { elapsed: t0.elapsed(), last_l: engine.last_left(), last_r: engine.last_right() }
}

/// Run the poly_synth with `poly_voices` voices and **no MIDI**: oscillators
/// stay at V/oct = 0 and ADSR outputs are zero.  Used as a silent baseline to
/// quantify how much MIDI activity and ADSR state transitions cost.
fn run_config_silent(poly_voices: usize) -> RunResult {
    let yaml     = load_yaml();
    let registry = patches_modules::default_registry();
    let graph    = patches_core::graph_yaml::yaml_to_graph(yaml, &registry)
        .expect("poly_synth.yaml parse failed");

    let env = AudioEnvironment { sample_rate: SAMPLE_RATE, poly_voices };
    let mut planner = Planner::new();
    let plan = planner.build(&graph, &registry, &env).expect("plan build failed");

    let mut engine = HeadlessEngine::new(plan, BUFFER_CAP, MODULE_CAP);

    for _ in 0..WARMUP_SAMPLES {
        engine.tick();
    }

    let t0 = Instant::now();
    for _ in 0..SAMPLE_COUNT {
        engine.tick();
    }

    RunResult { elapsed: t0.elapsed(), last_l: engine.last_left(), last_r: engine.last_right() }
}

fn print_row(label: &str, r: &RunResult) {
    let cpu_load   = r.elapsed.as_secs_f32() / DURATION_SECS * 100.0;
    let per_sample = r.elapsed.as_nanos() as f32 / SAMPLE_COUNT as f32;
    let headroom   = 100.0 / cpu_load;
    println!(
        "  {:<32} {:>8.3} ms  {:>7.1} ns/sample  {:>6.2}%  {:>6.1}x",
        label,
        r.elapsed.as_secs_f32() * 1000.0,
        per_sample,
        cpu_load,
        headroom,
    );
}

// ── Test ──────────────────────────────────────────────────────────────────────

#[test]
fn poly_synth_headless_profile() {
    // ── Run all configurations ─────────────────────────────────────────────────
    // All voice-count runs receive a voice-count-scaled MIDI schedule so that
    // all voice slots are active and ADSR/oscillator paths are genuinely hot.
    // The silent-8 run is kept solely to isolate the MIDI + ADSR overhead.
    let r1       = run_config(1);
    let r4       = run_config(4);
    let r8       = run_config(8);
    let r16      = run_config(16);
    let r8_silent = run_config_silent(8);

    // ── Report ─────────────────────────────────────────────────────────────────
    let n_events_8: usize = build_midi_schedule(SAMPLE_COUNT, 8).iter().map(|v| v.len()).sum();
    let n_events_16: usize = build_midi_schedule(SAMPLE_COUNT, 16).iter().map(|v| v.len()).sum();

    println!();
    println!("╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                    poly_synth headless profile results                      ║");
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!("║  Patch:    poly_synth.yaml   Sample rate: {SAMPLE_RATE:.0} Hz                         ║");
    println!("║  Duration: {DURATION_SECS:.1} s = {SAMPLE_COUNT} samples                                  ║");
    println!("║  MIDI events: 8-voice={n_events_8}  16-voice={n_events_16}  (voice-count-scaled)         ║");
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!("  {:<32} {:>11}  {:>14}  {:>7}  {:>7}",
        "Configuration", "Wall time", "Per-sample", "CPU", "Headroom");
    println!("  {}", "─".repeat(74));

    print_row("1-voice  + MIDI (scaled)",  &r1);
    print_row("4-voice  + MIDI (scaled)",  &r4);
    print_row("8-voice  + MIDI (scaled)",  &r8);
    print_row("16-voice + MIDI (scaled)",  &r16);
    print_row("8-voice  silent (baseline)", &r8_silent);

    println!("╠══════════════════════════════════════════════════════════════════════════════╣");

    // Scaling analysis
    let t1       = r1.elapsed.as_secs_f32();
    let t4       = r4.elapsed.as_secs_f32();
    let t8       = r8.elapsed.as_secs_f32();
    let t16      = r16.elapsed.as_secs_f32();
    let t8s      = r8_silent.elapsed.as_secs_f32();
    let midi_overhead_pct = (t8 - t8s) / t8s * 100.0;

    println!("║  Voice-count scaling (all with active MIDI):                                ║");
    println!("║    1→4  voices: {:.2}x wall time  (ideal 4x if linear)                    ║", t4 / t1);
    println!("║    1→8  voices: {:.2}x wall time  (ideal 8x if linear)                    ║", t8 / t1);
    println!("║    1→16 voices: {:.2}x wall time  (ideal 16x if linear)                   ║", t16 / t1);
    println!("║  MIDI + ADSR overhead vs silent 8-voice: {:+.1}%                            ║", midi_overhead_pct);
    println!("╠══════════════════════════════════════════════════════════════════════════════╣");
    println!("║  Interpretation:                                                            ║");

    let scaling_1_8 = t8 / t1;
    if scaling_1_8 < 6.0 {
        println!("║    Sub-linear scaling: fixed overhead (graph dispatch, CablePool) dominates. ║");
    } else if scaling_1_8 < 10.0 {
        println!("║    Roughly linear scaling: poly voice loops dominate processing time.         ║");
    } else {
        println!("║    Super-linear scaling: possible cache pressure with many active voices.     ║");
    }

    let cpu_8v = t8 / DURATION_SECS * 100.0;
    if cpu_8v < 5.0 {
        println!("║    8-voice active CPU {:.2}%: substantial headroom for more modules.      ║", cpu_8v);
    } else if cpu_8v < 20.0 {
        println!("║    8-voice active CPU {:.2}%: comfortable headroom for a live set.       ║", cpu_8v);
    } else {
        println!("║    8-voice active CPU {:.2}%: worth profiling with cargo-instruments.    ║", cpu_8v);
    }

    if midi_overhead_pct < 5.0 {
        println!("║    MIDI+ADSR overhead < 5%: voice-allocation and envelope state are cheap.   ║");
    } else if midi_overhead_pct < 20.0 {
        println!("║    MIDI+ADSR overhead notable: ADSR state transitions are a measurable cost. ║");
    } else {
        println!("║    MIDI+ADSR overhead > 20%: investigate PolyMidiIn and PolyAdsr hot paths.  ║");
    }

    println!("╚══════════════════════════════════════════════════════════════════════════════╝");

    // Sanity: all MIDI-active configs must produce non-zero output
    for (label, r) in [("1-voice", &r1), ("4-voice", &r4), ("8-voice", &r8), ("16-voice", &r16)] {
        assert!(
            r.last_l.abs() > 1e-10 || r.last_r.abs() > 1e-10,
            "{label}: expected non-zero output after MIDI notes fired"
        );
    }

    // All configurations must be real-time safe (< 100% CPU)
    for (label, r) in [
        ("1-voice",  &r1), ("4-voice", &r4), ("8-voice", &r8),
        ("16-voice", &r16), ("8-voice-silent", &r8_silent),
    ] {
        let cpu = r.elapsed.as_secs_f32() / DURATION_SECS * 100.0;
        assert!(cpu < 100.0, "{label} exceeded real-time budget: {cpu:.1}% CPU");
    }
}
