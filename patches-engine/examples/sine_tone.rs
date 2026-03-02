use std::process;
use std::thread;
use std::time::Duration;

use patches_core::{ModuleGraph, NodeId, PortRef};
use patches_engine::{build_patch, BufferAllocState, SoundEngine};
use patches_modules::{AudioOut, Sum, SineOscillator};

// A major third above 440 Hz: 440 * 2^(4/12)
const FREQ_A4: f64 = 440.0;
const FREQ_CS5: f64 = 554.365_226_444;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = ModuleGraph::new();
    let sine_a = NodeId::from("sine_a");
    let sine_b = NodeId::from("sine_b");
    let mix = NodeId::from("mix");
    let out = NodeId::from("out");
    graph.add_module(sine_a.clone(), Box::new(SineOscillator::new(FREQ_A4)))?;
    graph.add_module(sine_b.clone(), Box::new(SineOscillator::new(FREQ_CS5)))?;
    graph.add_module(mix.clone(), Box::new(Sum::new(2)))?;
    graph.add_module(out.clone(), Box::new(AudioOut::new()))?;
    let p = |name| PortRef { name, index: 0 };
    graph.connect(&sine_a, p("out"), &mix, PortRef { name: "in", index: 0 }, 0.5)?;
    graph.connect(&sine_b, p("out"), &mix, PortRef { name: "in", index: 1 }, 0.5)?;
    graph.connect(&mix, p("out"), &out, p("left"), 1.0)?;
    graph.connect(&mix, p("out"), &out, p("right"), 1.0)?;

    let (plan, _) = build_patch(graph, None, &BufferAllocState::default(), 4096)?;

    let mut engine = SoundEngine::new(plan, 4096, 64)?;
    engine.start()?;

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
