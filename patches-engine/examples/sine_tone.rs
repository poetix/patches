use std::process;
use std::thread;
use std::time::Duration;

use patches_core::ModuleGraph;
use patches_engine::{build_patch, EngineError, SoundEngine};
use patches_modules::{AudioOut, Crossfade, SineOscillator};

// A major third above 440 Hz: 440 * 2^(4/12)
const FREQ_A4: f64 = 440.0;
const FREQ_CS5: f64 = 554.365_226_444;

fn run() -> Result<(), EngineError> {
    let mut graph = ModuleGraph::new();
    let sine_a = graph.add_module(Box::new(SineOscillator::new(FREQ_A4)));
    let sine_b = graph.add_module(Box::new(SineOscillator::new(FREQ_CS5)));
    let mix = graph.add_module(Box::new(Crossfade::new()));
    let out = graph.add_module(Box::new(AudioOut::new()));
    graph.connect(sine_a, "out", mix, "a").unwrap();
    graph.connect(sine_b, "out", mix, "b").unwrap();
    graph.connect(mix, "out", out, "left").unwrap();
    graph.connect(mix, "out", out, "right").unwrap();

    let plan = build_patch(graph).map_err(|e| {
        eprintln!("failed to build patch: {e}");
        // BuildError is distinct from EngineError; map to a sentinel by printing
        // and exiting here so the ? operator works with a single error type.
        process::exit(1);
    })?;

    let mut engine = SoundEngine::new(plan)?;
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
