use std::process;
use std::thread;
use std::time::Duration;

use patches_core::{Module, ModuleGraph, ModuleShape, NodeId, PortRef};
use patches_core::parameter_map::{ParameterMap, ParameterValue};
use patches_engine::{PatchEngine, PatchEngineError};
use patches_modules::{AudioOut, Sum, SineOscillator};

// Equal-temperament frequencies (A4 = 440 Hz)
const FREQ_C4: f64 = 261.625_565_3;
const FREQ_E4: f64 = 329.627_556_9;
const FREQ_F4: f64 = 349.228_231_4;

/// C4 + E4 (major third) wired through a mixer to stereo out.
fn initial_graph() -> Result<ModuleGraph, Box<dyn std::error::Error>> {
    let mut graph = ModuleGraph::new();
    let osc_c4 = NodeId::from("osc_c4");
    let osc_e4 = NodeId::from("osc_e4");
    let mix = NodeId::from("mix");
    let out = NodeId::from("out");

    let mut params_c4 = ParameterMap::new();
    params_c4.insert("frequency".to_string(), ParameterValue::Float(FREQ_C4));
    let mut params_e4 = ParameterMap::new();
    params_e4.insert("frequency".to_string(), ParameterValue::Float(FREQ_E4));

    graph.add_module(osc_c4.clone(), SineOscillator::describe(&ModuleShape { channels: 0, length: 0 }), &params_c4)?;
    graph.add_module(osc_e4.clone(), SineOscillator::describe(&ModuleShape { channels: 0, length: 0 }), &params_e4)?;
    graph.add_module(mix.clone(), Sum::describe(&ModuleShape { channels: 2, length: 0 }), &ParameterMap::new())?;
    graph.add_module(out.clone(), AudioOut::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new())?;
    let p = |name| PortRef { name, index: 0 };
    graph.connect(&osc_c4, p("out"), &mix, PortRef { name: "in", index: 0 }, 0.5)?;
    graph.connect(&osc_e4, p("out"), &mix, PortRef { name: "in", index: 1 }, 0.5)?;
    graph.connect(&mix, p("out"), &out, p("left"), 1.0)?;
    graph.connect(&mix, p("out"), &out, p("right"), 1.0)?;
    Ok(graph)
}

/// C4 + F4 (perfect fourth): E4 removed, F4 added in its place.
///
/// The C4 oscillator keeps the same NodeId ("osc_c4") so the planner can
/// reuse its accumulated phase, avoiding an audible discontinuity.
fn updated_graph() -> Result<ModuleGraph, Box<dyn std::error::Error>> {
    let mut graph = ModuleGraph::new();
    let osc_c4 = NodeId::from("osc_c4");
    let osc_f4 = NodeId::from("osc_f4");
    let mix = NodeId::from("mix");
    let out = NodeId::from("out");

    let mut params_c4 = ParameterMap::new();
    params_c4.insert("frequency".to_string(), ParameterValue::Float(FREQ_C4));
    let mut params_f4 = ParameterMap::new();
    params_f4.insert("frequency".to_string(), ParameterValue::Float(FREQ_F4));

    graph.add_module(osc_c4.clone(), SineOscillator::describe(&ModuleShape { channels: 0, length: 0 }), &params_c4)?;
    graph.add_module(osc_f4.clone(), SineOscillator::describe(&ModuleShape { channels: 0, length: 0 }), &params_f4)?;
    graph.add_module(mix.clone(), Sum::describe(&ModuleShape { channels: 2, length: 0 }), &ParameterMap::new())?;
    graph.add_module(out.clone(), AudioOut::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new())?;
    let p = |name| PortRef { name, index: 0 };
    graph.connect(&osc_c4, p("out"), &mix, PortRef { name: "in", index: 0 }, 0.5)?;
    graph.connect(&osc_f4, p("out"), &mix, PortRef { name: "in", index: 1 }, 0.5)?;
    graph.connect(&mix, p("out"), &out, p("left"), 1.0)?;
    graph.connect(&mix, p("out"), &out, p("right"), 1.0)?;
    Ok(graph)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Build the initial plan and start the audio thread.
    let registry = patches_modules::default_registry();
    let mut engine = PatchEngine::new(registry)?;
    let initial = initial_graph()?;
    engine.start(&initial)?;
    println!("Playing C4 + E4 (major third) for 1 second…");
    thread::sleep(Duration::from_secs(1));

    // Replan: swap E4 for F4. Retry if the engine's single-slot channel is
    // still occupied by the previous swap (clears within one buffer period,
    // typically ~10 ms).
    println!("Switching to C4 + F4 (perfect fourth)…");
    let updated = updated_graph()?;
    loop {
        match engine.update(&updated) {
            Ok(()) => break,
            Err(PatchEngineError::ChannelFull) => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e.into()),
        }
    }
    thread::sleep(Duration::from_secs(1));

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
