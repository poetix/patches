use std::process;
use std::thread;
use std::time::Duration;

use patches_core::{ModuleGraph, NodeId, PortRef};
use patches_engine::PatchEngine;
use patches_modules::{
    AdsrEnvelope, AudioOut, ClockSequencer, SawtoothOscillator, SquareOscillator, StepSequencer,
    Sum, Vca,
};

const BPM: f64 = 120.0;
const BEATS_PER_BAR: u32 = 4;
const QUAVERS_PER_BEAT: u32 = 2;

const ATTACK_SECS: f64 = 0.0002;
const DECAY_SECS: f64 = 0.05;
const SUSTAIN: f64 = 0.5;
const RELEASE_SECS: f64 = 0.1;

const RUN_SECS: u64 = 8; // ~4 bars at 120 BPM

const PATTERN: &[&str] = &[
    "C3", "Eb3", "F3", "G3", "-", "Bb3", "-", "C4", "-", "G3", "F3", "Eb3", "-", "C3", "_", "-",
];

fn build_graph() -> Result<ModuleGraph, Box<dyn std::error::Error>> {
    let mut graph = ModuleGraph::new();

    let clock = NodeId::from("clock");
    let seq = NodeId::from("seq");
    let saw = NodeId::from("saw");
    let sq = NodeId::from("sq");
    let mix = NodeId::from("mix");
    let env = NodeId::from("env");
    let vca = NodeId::from("vca");
    let out = NodeId::from("out");

    graph.add_module(
        clock.clone(),
        Box::new(ClockSequencer::new(BPM, BEATS_PER_BAR, QUAVERS_PER_BEAT)),
    )?;
    graph.add_module(
        seq.clone(),
        Box::new(StepSequencer::new(PATTERN)?),
    )?;
    graph.add_module(saw.clone(), Box::new(SawtoothOscillator::new(0.0)))?;
    graph.add_module(sq.clone(), Box::new(SquareOscillator::new(0.005)))?;
    graph.add_module(mix.clone(), Box::new(Sum::new(2)))?;
    graph.add_module(
        env.clone(),
        Box::new(AdsrEnvelope::new(ATTACK_SECS, DECAY_SECS, SUSTAIN, RELEASE_SECS)),
    )?;
    graph.add_module(vca.clone(), Box::new(Vca::new()))?;
    graph.add_module(out.clone(), Box::new(AudioOut::new()))?;

    let p = |name| PortRef { name, index: 0 };

    // clock.semiquaver → seq.clock
    graph.connect(
        &clock,
        p("semiquaver"),
        &seq,
        p("clock"),
        1.0,
    )?;

    // seq.pitch → saw.voct
    graph.connect(&seq, p("pitch"), &saw, p("voct"), 1.0)?;
    // seq.pitch → sq.voct
    graph.connect(&seq, p("pitch"), &sq, p("voct"), 1.0)?;

    // seq.trigger → env.trigger
    graph.connect(&seq, p("trigger"), &env, p("trigger"), 1.0)?;
    // seq.gate → env.gate
    graph.connect(&seq, p("gate"), &env, p("gate"), 1.0)?;

    // saw.out → mix.in[0] (scale 0.5)
    graph.connect(&saw, p("out"), &mix, PortRef { name: "in", index: 0 }, 0.3)?;
    // sq.out → mix.in[1] (scale 0.5)
    graph.connect(&sq, p("out"), &mix, PortRef { name: "in", index: 1 }, 0.7)?;

    // mix.out → vca.in
    graph.connect(&mix, p("out"), &vca, p("in"), 1.0)?;
    // env.out → vca.cv
    graph.connect(&env, p("out"), &vca, PortRef { name: "cv", index: 1 }, 1.0)?;

    // vca.out → out.left and out.right
    graph.connect(&vca, p("out"), &out, p("left"), 1.0)?;
    graph.connect(&vca, p("out"), &out, p("right"), 1.0)?;

    Ok(graph)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut engine = PatchEngine::new(build_graph()?)?;
    engine.start()?;
    println!("Playing 16-step minor-pentatonic phrase at {BPM} BPM for {RUN_SECS} seconds…");
    thread::sleep(Duration::from_secs(RUN_SECS));
    engine.stop();
    println!("Done.");
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        process::exit(1);
    }
}
