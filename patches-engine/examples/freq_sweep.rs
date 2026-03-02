use std::process;
use std::thread;
use std::time::Duration;

use patches_core::{ControlSignal, InstanceId, Module, ModuleGraph, PortRef};
use patches_engine::{PatchEngine, PatchEngineError};
use patches_modules::{AudioOut, SineOscillator};

/// Number of steps per direction (rise or fall).
///
/// At 20 ms per step this gives ~2 seconds up and ~2 seconds down.
const STEPS: usize = 100;

const FREQ_LOW: f64 = 110.0;
const FREQ_HIGH: f64 = 880.0;

fn build_graph() -> Result<(ModuleGraph, InstanceId), Box<dyn std::error::Error>> {
    let mut graph = ModuleGraph::new();

    // Capture the InstanceId before moving the oscillator into the graph.
    let osc = SineOscillator::new(FREQ_LOW);
    let osc_id = osc.instance_id();
    graph.add_module("osc", Box::new(osc))?;
    graph.add_module("out", Box::new(AudioOut::new()))?;

    let p = |name| PortRef { name, index: 0 };
    graph.connect(&"osc".into(), p("out"), &"out".into(), p("left"), 1.0)?;
    graph.connect(&"osc".into(), p("out"), &"out".into(), p("right"), 1.0)?;

    Ok((graph, osc_id))
}

/// Send a frequency update, printing a warning if the signal buffer is full.
fn send_freq(
    engine: &mut PatchEngine,
    osc_id: InstanceId,
    freq: f64,
) -> Result<(), PatchEngineError> {
    if let Err(_dropped) =
        engine.send_signal(osc_id, ControlSignal::Float { name: "freq", value: freq })
    {
        eprintln!("warning: signal buffer full, skipping freq={freq:.1} Hz");
    }
    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (graph, osc_id) = build_graph()?;
    let mut engine = PatchEngine::new(graph)?;
    engine.start()?;

    println!("Sweeping {FREQ_LOW} Hz → {FREQ_HIGH} Hz → {FREQ_LOW} Hz over ~4 seconds…");

    let ratio = FREQ_HIGH / FREQ_LOW;

    // Rise: FREQ_LOW → FREQ_HIGH (exponential, perceptually linear in pitch).
    for step in 0..STEPS {
        let freq = FREQ_LOW * ratio.powf(step as f64 / (STEPS - 1) as f64);
        send_freq(&mut engine, osc_id, freq)?;
        thread::sleep(Duration::from_millis(20));
    }

    // Fall: FREQ_HIGH → FREQ_LOW.
    for step in 0..STEPS {
        let freq = FREQ_HIGH * (1.0 / ratio).powf(step as f64 / (STEPS - 1) as f64);
        send_freq(&mut engine, osc_id, freq)?;
        thread::sleep(Duration::from_millis(20));
    }

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
