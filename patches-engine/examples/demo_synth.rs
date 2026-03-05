use std::process;
use std::thread;
use std::time::Duration;

use patches_core::{Module, ModuleGraph, ModuleShape, NodeId, PortRef};
use patches_core::parameter_map::{ParameterMap, ParameterValue};
use patches_engine::PatchEngine;
use patches_modules::{
    AdsrEnvelope, AudioOut, ClockSequencer, Glide, SawtoothOscillator, SineOscillator,
    SquareOscillator, StepSequencer, Sum, Vca,
};

const BPM: f64 = 120.0;
const BEATS_PER_BAR: u32 = 4;
const QUAVERS_PER_BEAT: u32 = 2;

// AdsrEnvelope "attack" parameter minimum is 0.001 s.
const ATTACK_SECS: f64 = 0.001;
const DECAY_SECS: f64 = 0.05;
const SUSTAIN: f64 = 0.4;
const RELEASE_SECS: f64 = 0.2;

const RUN_SECS: u64 = 8; // ~4 bars at 120 BPM

const PATTERN: &[&str] = &[
    "C3", "Eb3", "F3", "G3", "-", "Bb3", "-", "C4", "-", "G3", "F3", "Eb3", "-", "C3", "_", "-",
];

fn build_graph() -> Result<ModuleGraph, Box<dyn std::error::Error>> {
    let mut graph = ModuleGraph::new();

    let clock = NodeId::from("clock");
    let seq = NodeId::from("seq");
    let glide = NodeId::from("glide");
    let lfo = NodeId::from("lfo");
    let saw = NodeId::from("saw");
    let sq = NodeId::from("sq");
    let mix = NodeId::from("mix");
    let env = NodeId::from("env");
    let vca = NodeId::from("vca");
    let out = NodeId::from("out");

    let mut clock_params = ParameterMap::new();
    clock_params.insert("bpm".to_string(), ParameterValue::Float(BPM));
    clock_params.insert("beats_per_bar".to_string(), ParameterValue::Int(BEATS_PER_BAR as i64));
    clock_params.insert("quavers_per_beat".to_string(), ParameterValue::Int(QUAVERS_PER_BEAT as i64));
    graph.add_module(clock.clone(), ClockSequencer::describe(&ModuleShape { channels: 0 }), &clock_params)?;

    let mut seq_params = ParameterMap::new();
    seq_params.insert("steps".to_string(), ParameterValue::Array(
        PATTERN.iter().map(|s| s.to_string()).collect(),
    ));
    graph.add_module(seq.clone(), StepSequencer::describe(&ModuleShape { channels: 0 }), &seq_params)?;

    let mut glide_params = ParameterMap::new();
    glide_params.insert("glide_ms".to_string(), ParameterValue::Float(50.0));
    graph.add_module(glide.clone(), Glide::describe(&ModuleShape { channels: 0 }), &glide_params)?;

    let mut lfo_params = ParameterMap::new();
    lfo_params.insert("frequency".to_string(), ParameterValue::Float(0.2));
    graph.add_module(lfo.clone(), SineOscillator::describe(&ModuleShape { channels: 0 }), &lfo_params)?;

    let mut saw_params = ParameterMap::new();
    saw_params.insert("base_voct".to_string(), ParameterValue::Float(0.0));
    graph.add_module(saw.clone(), SawtoothOscillator::describe(&ModuleShape { channels: 0 }), &saw_params)?;

    let mut sq_params = ParameterMap::new();
    sq_params.insert("base_voct".to_string(), ParameterValue::Float(1.005));
    graph.add_module(sq.clone(), SquareOscillator::describe(&ModuleShape { channels: 0 }), &sq_params)?;

    graph.add_module(mix.clone(), Sum::describe(&ModuleShape { channels: 2 }), &ParameterMap::new())?;

    let mut env_params = ParameterMap::new();
    env_params.insert("attack".to_string(), ParameterValue::Float(ATTACK_SECS));
    env_params.insert("decay".to_string(), ParameterValue::Float(DECAY_SECS));
    env_params.insert("sustain".to_string(), ParameterValue::Float(SUSTAIN));
    env_params.insert("release".to_string(), ParameterValue::Float(RELEASE_SECS));
    graph.add_module(env.clone(), AdsrEnvelope::describe(&ModuleShape { channels: 0 }), &env_params)?;

    graph.add_module(vca.clone(), Vca::describe(&ModuleShape { channels: 0 }), &ParameterMap::new())?;
    graph.add_module(out.clone(), AudioOut::describe(&ModuleShape { channels: 0 }), &ParameterMap::new())?;

    let p = |name| PortRef { name, index: 0 };

    // clock.semiquaver → seq.clock
    graph.connect(&clock, p("semiquaver"), &seq, p("clock"), 1.0)?;

    // seq.pitch → glide.in
    graph.connect(&seq, p("pitch"), &glide, p("in"), 1.0)?;
    // glide.out → saw.voct
    graph.connect(&glide, p("out"), &saw, p("voct"), 1.0)?;
    // glide.out → sq.voct
    graph.connect(&glide, p("out"), &sq, p("voct"), 1.0)?;

    // seq.trigger → env.trigger
    graph.connect(&seq, p("trigger"), &env, p("trigger"), 1.0)?;
    // seq.gate → env.gate
    graph.connect(&seq, p("gate"), &env, p("gate"), 1.0)?;

    // lfo -> sq.pulse_width
    graph.connect(&lfo, p("out"), &sq, p("pulse_width"), 0.8)?;
    // saw.out → mix.in[0]
    graph.connect(&saw, p("out"), &mix, PortRef { name: "in", index: 0 }, 0.3)?;
    // sq.out → mix.in[1]
    graph.connect(&sq, p("out"), &mix, PortRef { name: "in", index: 1 }, 0.7)?;

    // mix.out → vca.in
    graph.connect(&mix, p("out"), &vca, p("in"), 1.0)?;
    // env.out → vca.cv
    graph.connect(&env, p("out"), &vca, PortRef { name: "cv", index: 0 }, 1.0)?;

    // vca.out → out.left and out.right
    graph.connect(&vca, p("out"), &out, p("left"), 1.0)?;
    graph.connect(&vca, p("out"), &out, p("right"), 1.0)?;

    Ok(graph)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let registry = patches_modules::default_registry();
    let mut engine = PatchEngine::new(registry)?;
    let graph = build_graph()?;
    engine.start(&graph)?;
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
