//! `patch_player` — load a patches YAML file, play it, and hot-reload on change.
//!
//! Usage:
//!   patch_player <path-to-yaml>

use std::env;
use std::fs;
use std::process;
use std::thread;
use std::time::{Duration, SystemTime};

use patches_core::graph_yaml::yaml_to_graph;
use patches_engine::{new_event_queue, EventScheduler, MidiConnector, PatchEngine, PatchEngineError};

fn mtime(path: &str) -> std::io::Result<SystemTime> {
    fs::metadata(path)?.modified()
}

fn load_graph(
    path: &str,
    registry: &patches_core::Registry,
) -> Result<patches_core::ModuleGraph, Box<dyn std::error::Error>> {
    let yaml = fs::read_to_string(path)?;
    Ok(yaml_to_graph(&yaml, registry)?)
}

/// Push `graph` to `engine`, retrying if the plan channel is full.
fn push_graph(engine: &mut PatchEngine, graph: &patches_core::ModuleGraph) {
    loop {
        match engine.update(graph) {
            Ok(()) => {
                println!("Reloaded.");
                return;
            }
            Err(PatchEngineError::ChannelFull) => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                eprintln!("reload error: {e}");
                return;
            }
        }
    }
}

fn run(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Two registries: one to rehydrate YAML graphs, one owned by the engine.
    let load_registry = patches_modules::default_registry();
    let graph = load_graph(path, &load_registry)?;

    let mut engine = PatchEngine::new(patches_modules::default_registry())?;

    // Create the MIDI event queue. The consumer goes into the audio callback;
    // the producer goes to the MidiConnector which is opened after start().
    let (midi_producer, midi_consumer) = new_event_queue(256);
    engine.start(&graph, Some(midi_consumer))?;

    // Open all available MIDI input ports. The connector must stay alive for
    // the duration of playback; dropping it disconnects all ports.
    let sample_rate = engine.sample_rate().unwrap_or(44_100.0);
    let scheduler = EventScheduler::new(sample_rate, 128);
    let _midi_connector = match MidiConnector::open(engine.clock(), midi_producer, scheduler) {
        Ok(c) => {
            println!("MIDI input open.");
            Some(c)
        }
        Err(e) => {
            eprintln!("warn: could not open MIDI input: {e}");
            None
        }
    };

    println!("Loaded {path}");
    println!("Watching for changes… (Ctrl-C to stop)");

    let mut last_mtime = mtime(path)?;

    loop {
        thread::sleep(Duration::from_millis(500));

        let current_mtime = match mtime(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("warn: could not stat {path}: {e}");
                continue;
            }
        };

        if current_mtime != last_mtime {
            last_mtime = current_mtime;
            match load_graph(path, &load_registry) {
                Ok(graph) => push_graph(&mut engine, &graph),
                Err(e) => eprintln!("parse error (keeping current patch): {e}"),
            }
        }
    }
}

fn main() {
    let path = match env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: patch_player <path-to-yaml>");
            process::exit(1);
        }
    };

    if let Err(e) = run(&path) {
        eprintln!("error: {e}");
        process::exit(1);
    }
}
