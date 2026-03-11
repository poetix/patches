use std::process;
use std::thread;
use std::time::Duration;

use patches_core::{Module, ModuleGraph, ModuleShape, NodeId, PortRef};
use patches_core::parameter_map::{ParameterMap, ParameterValue};
use patches_engine::{build_patch, PlannerState, SoundEngine};
use patches_modules::{AudioOut, Sum, Oscillator};

// A major third above 440 Hz: 440 * 2^(4/12)
const FREQ_A4: f64 = 440.0;
const FREQ_CS5: f64 = 554.365_226_444;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = ModuleGraph::new();
    let sine_a = NodeId::from("sine_a");
    let sine_b = NodeId::from("sine_b");
    let mix = NodeId::from("mix");
    let out = NodeId::from("out");

    let mut params_a = ParameterMap::new();
    params_a.insert("frequency".to_string(), ParameterValue::Float(FREQ_A4));
    let mut params_b = ParameterMap::new();
    params_b.insert("frequency".to_string(), ParameterValue::Float(FREQ_CS5));

    graph.add_module(sine_a.clone(), Oscillator::describe(&ModuleShape { channels: 0, length: 0 }), &params_a)?;
    graph.add_module(sine_b.clone(), Oscillator::describe(&ModuleShape { channels: 0, length: 0 }), &params_b)?;
    graph.add_module(mix.clone(), Sum::describe(&ModuleShape { channels: 2, length: 0 }), &ParameterMap::new())?;
    graph.add_module(out.clone(), AudioOut::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new())?;

    let p = |name| PortRef { name, index: 0 };
    graph.connect(&sine_a, p("sine"), &mix, PortRef { name: "in", index: 0 }, 0.5)?;
    graph.connect(&sine_b, p("sine"), &mix, PortRef { name: "in", index: 1 }, 0.5)?;
    graph.connect(&mix, p("out"), &out, p("left"), 1.0)?;
    graph.connect(&mix, p("out"), &out, p("right"), 1.0)?;

    // Two-phase startup: open the device first to get the real sample rate,
    // then build the plan and start the audio thread.
    let registry = patches_modules::default_registry();
    let mut engine = SoundEngine::new(4096, 1024)?;
    let env = engine.open()?;
    let (plan, _state) = build_patch(&graph, &registry, &env, &PlannerState::empty(), 4096, 1024)?;
    engine.start(plan)?;

    println!("Playing A4 + C#5 (major third) for 3 seconds…");
    thread::sleep(Duration::from_secs(3));

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
