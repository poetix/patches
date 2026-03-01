use std::process;
use std::thread;
use std::time::Duration;

use patches_core::ModuleGraph;
use patches_engine::{build_patch, BufferAllocState, SoundEngine};
use patches_modules::{AudioOut, Mix, SineOscillator};

// A major third above 440 Hz: 440 * 2^(4/12)
const FREQ_A4: f64 = 440.0;
const FREQ_CS5: f64 = 554.365_226_444;

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = ModuleGraph::new();
    let sine_a = graph.add_module(Box::new(SineOscillator::new(FREQ_A4)));
    let sine_b = graph.add_module(Box::new(SineOscillator::new(FREQ_CS5)));
    let mix = graph.add_module(Box::new(Mix::new()));
    let out = graph.add_module(Box::new(AudioOut::new()));
    graph.connect(sine_a, "out", mix, "a", 1.0)?;
    graph.connect(sine_b, "out", mix, "b", 1.0)?;
    graph.connect(mix, "out", out, "left", 1.0)?;
    graph.connect(mix, "out", out, "right", 1.0)?;

    let (plan, _) = build_patch(graph, None, &BufferAllocState::default(), 4096)?;

    let mut engine = SoundEngine::new(plan, 4096)?;
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
