use std::f64::consts::TAU;

use patches_core::{AudioEnvironment, Module, ModuleGraph, ModuleShape, NodeId, PortRef};
use patches_core::parameter_map::{ParameterMap, ParameterValue};
use patches_engine::{build_patch, ExecutionPlan, ModulePool, PlannerState};
use patches_modules::{AudioOut, Oscillator, StepSequencer, Sum};
use patches_modules::common::frequency::C0_FREQ;

// ── constants ─────────────────────────────────────────────────────────────────

const POOL_CAP: usize = 256;
const MODULE_CAP: usize = 64;
const SAMPLE_RATE: f64 = 44100.0;

// ── helpers ───────────────────────────────────────────────────────────────────

fn env() -> AudioEnvironment {
    AudioEnvironment { sample_rate: SAMPLE_RATE }
}

fn p(name: &'static str) -> PortRef {
    PortRef { name, index: 0 }
}

/// Oscillator → AudioOut (sine output).
fn sine_out_graph(osc_id: &str, freq: f64) -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let mut params = ParameterMap::new();
    params.insert("frequency".to_string(), ParameterValue::Float(freq));
    graph.add_module(osc_id, Oscillator::describe(&ModuleShape { channels: 0, length: 0 }), &params).unwrap();
    graph.add_module("out", AudioOut::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new()).unwrap();
    graph.connect(&NodeId::from(osc_id), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();
    graph.connect(&NodeId::from(osc_id), p("sine"), &NodeId::from("out"), p("right"), 1.0).unwrap();
    graph
}

/// Osc("osc_a") + Osc("osc_b") → Sum(2) → AudioOut.
fn two_osc_graph(freq_a: f64, freq_b: f64) -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let mut pa = ParameterMap::new();
    pa.insert("frequency".to_string(), ParameterValue::Float(freq_a));
    let mut pb = ParameterMap::new();
    pb.insert("frequency".to_string(), ParameterValue::Float(freq_b));
    graph.add_module("osc_a", Oscillator::describe(&ModuleShape { channels: 0, length: 0 }), &pa).unwrap();
    graph.add_module("osc_b", Oscillator::describe(&ModuleShape { channels: 0, length: 0 }), &pb).unwrap();
    graph.add_module("mix", Sum::describe(&ModuleShape { channels: 2, length: 0 }), &ParameterMap::new()).unwrap();
    graph.add_module("out", AudioOut::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new()).unwrap();
    graph.connect(&NodeId::from("osc_a"), p("sine"), &NodeId::from("mix"), PortRef { name: "in", index: 0 }, 1.0).unwrap();
    graph.connect(&NodeId::from("osc_b"), p("sine"), &NodeId::from("mix"), PortRef { name: "in", index: 1 }, 1.0).unwrap();
    graph.connect(&NodeId::from("mix"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
    graph.connect(&NodeId::from("mix"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
    graph
}

/// Sum(1-channel) → AudioOut. Used as a different module type in type-change tests.
fn sum_out_graph() -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    graph.add_module("osc", Sum::describe(&ModuleShape { channels: 1, length: 0 }), &ParameterMap::new()).unwrap();
    graph.add_module("out", AudioOut::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new()).unwrap();
    graph.connect(&NodeId::from("osc"), p("out"), &NodeId::from("out"), p("left"), 1.0).unwrap();
    graph.connect(&NodeId::from("osc"), p("out"), &NodeId::from("out"), p("right"), 1.0).unwrap();
    graph
}

/// Tombstone removed modules, install new ones, apply parameter diffs.
fn adopt_plan(plan: &mut ExecutionPlan, pool: &mut ModulePool) {
    for &idx in &plan.tombstones {
        pool.tombstone(idx);
    }
    for (idx, m) in plan.new_modules.drain(..) {
        pool.install(idx, m);
    }
    for (idx, params) in &plan.parameter_updates {
        pool.update_parameters(*idx, params);
    }
}

fn make_buffer_pool() -> Vec<[f64; 2]> {
    vec![[0.0; 2]; POOL_CAP]
}

// ── tests ──────────────────────────────────────────────────────────────────────

/// Rebuilding an identical graph produces no new modules and preserves InstanceIds.
#[test]
fn surviving_modules_are_not_re_instantiated() {
    let registry = patches_modules::default_registry();
    let graph = sine_out_graph("osc", 440.0);

    let (plan_a, state_a) =
        build_patch(&graph, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP).unwrap();

    // Collect (pool_index → module InstanceId) from the first plan.
    // After the builder fix, state.instance_id equals module.instance_id().
    let ids_a: std::collections::HashMap<usize, patches_core::InstanceId> =
        plan_a.new_modules.iter().map(|(idx, m)| (*idx, m.instance_id())).collect();

    let (plan_b, state_b) =
        build_patch(&graph, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    assert!(plan_b.new_modules.is_empty(), "no new modules for an identical rebuild");
    assert!(plan_b.tombstones.is_empty(), "no tombstones for an identical rebuild");

    // Every node's InstanceId and pool slot must be stable across an identical rebuild.
    for (node_id, ns_a) in &state_a.nodes {
        let ns_b = state_b.nodes.get(node_id).expect("node must still be in state_b");
        assert_eq!(
            ns_a.instance_id, ns_b.instance_id,
            "InstanceId for {node_id:?} must survive an identical rebuild"
        );
        // The state's InstanceId must match the module that was installed.
        assert_eq!(
            ids_a[&ns_a.pool_index], ns_a.instance_id,
            "state InstanceId must equal the installed module's instance_id()"
        );
    }
}

/// Adding a node to the graph causes it to appear in new_modules with a fresh InstanceId.
#[test]
fn new_node_triggers_instantiation() {
    let registry = patches_modules::default_registry();
    let graph_a = sine_out_graph("osc_a", 440.0);

    let (_plan_a, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP).unwrap();

    // graph_b adds "osc_b" and "mix" (two new nodes).
    let graph_b = two_osc_graph(440.0, 880.0);
    let (plan_b, state_b) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    // "osc_b" and "mix" are new; "osc_a" and "out" survive.
    let new_module_slots: std::collections::HashSet<usize> =
        plan_b.new_modules.iter().map(|(idx, _)| *idx).collect();
    assert_eq!(new_module_slots.len(), 2, "exactly two new modules (osc_b and mix)");

    // The new pool slots must match the state assignments for "osc_b" and "mix".
    let osc_b_slot = state_b.nodes.get(&NodeId::from("osc_b")).expect("osc_b in state_b").pool_index;
    let mix_slot = state_b.nodes.get(&NodeId::from("mix")).expect("mix in state_b").pool_index;

    assert!(new_module_slots.contains(&osc_b_slot), "osc_b pool slot must be in new_modules");
    assert!(new_module_slots.contains(&mix_slot), "mix pool slot must be in new_modules");

    // Verify the state InstanceIds match the modules in new_modules (builder fix check).
    for (idx, m) in &plan_b.new_modules {
        let node_state = state_b.nodes.values().find(|ns| ns.pool_index == *idx).unwrap();
        assert_eq!(
            m.instance_id(), node_state.instance_id,
            "module InstanceId at slot {idx} must equal state InstanceId"
        );
    }

    // "osc_a" and "out" must NOT appear in new_modules (they survive).
    assert!(
        !new_module_slots.contains(&state_a.nodes[&NodeId::from("osc_a")].pool_index),
        "osc_a must not be in new_modules"
    );
    assert!(
        !new_module_slots.contains(&state_a.nodes[&NodeId::from("out")].pool_index),
        "out must not be in new_modules"
    );
}

/// Removing a node from the graph causes it to appear in tombstones.
#[test]
fn removed_node_triggers_tombstone() {
    let registry = patches_modules::default_registry();
    let graph_a = two_osc_graph(440.0, 880.0);

    let (_plan_a, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP).unwrap();

    let osc_b_slot = state_a.nodes[&NodeId::from("osc_b")].pool_index;
    let mix_slot = state_a.nodes[&NodeId::from("mix")].pool_index;

    // graph_b removes "osc_b" and "mix" (back to single-osc layout).
    let graph_b = sine_out_graph("osc_a", 440.0);
    let (plan_b, _state_b) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    assert!(
        plan_b.tombstones.contains(&osc_b_slot),
        "osc_b pool slot must be tombstoned on removal"
    );
    assert!(
        plan_b.tombstones.contains(&mix_slot),
        "mix pool slot must be tombstoned on removal"
    );
    assert!(
        plan_b.new_modules.is_empty(),
        "no new modules when only removing nodes"
    );
}

/// Changing a node's module type tombstones the old slot and instantiates a new module.
#[test]
fn type_change_triggers_tombstone_and_new_module() {
    let registry = patches_modules::default_registry();
    let graph_a = sine_out_graph("osc", 440.0);

    let (_plan_a, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP).unwrap();

    let old_osc_slot = state_a.nodes[&NodeId::from("osc")].pool_index;

    // graph_b: same NodeId "osc" but Sum instead of Oscillator (type changed).
    let graph_b = sum_out_graph();
    let (plan_b, state_b) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    assert!(
        plan_b.tombstones.contains(&old_osc_slot),
        "old Oscillator slot must be tombstoned on type change"
    );

    let new_osc_slot = state_b.nodes[&NodeId::from("osc")].pool_index;
    let new_module_slots: Vec<usize> = plan_b.new_modules.iter().map(|(idx, _)| *idx).collect();
    assert!(
        new_module_slots.contains(&new_osc_slot),
        "new Sum must appear in new_modules"
    );

    // Verify the new module is actually a Sum.
    let new_osc = plan_b.new_modules.iter()
        .find(|(idx, _)| *idx == new_osc_slot)
        .map(|(_, m)| m)
        .unwrap();
    assert!(
        new_osc.as_any().downcast_ref::<Sum>().is_some(),
        "new module must be a Sum"
    );
}

/// A parameter-only change produces parameter_updates but no new_modules.
#[test]
fn parameter_only_change_produces_diffs_without_reinstantiation() {
    let registry = patches_modules::default_registry();
    let graph_a = sine_out_graph("osc", 440.0);

    let (_plan_a, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP).unwrap();

    let osc_slot = state_a.nodes[&NodeId::from("osc")].pool_index;

    // Same structure, different frequency.
    let graph_b = sine_out_graph("osc", 880.0);
    let (plan_b, state_b) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    assert!(plan_b.new_modules.is_empty(), "parameter-only change must not instantiate new modules");
    assert!(plan_b.tombstones.is_empty(), "parameter-only change must not tombstone any slots");

    // InstanceId must be preserved.
    assert_eq!(
        state_a.nodes[&NodeId::from("osc")].instance_id,
        state_b.nodes[&NodeId::from("osc")].instance_id,
        "InstanceId must be stable across a parameter-only change"
    );

    // The frequency diff must appear in parameter_updates for the osc slot.
    let update = plan_b.parameter_updates.iter()
        .find(|(idx, _)| *idx == osc_slot)
        .map(|(_, params)| params);
    let update = update.expect("osc slot must have a parameter update");
    assert!(
        matches!(update.get("frequency"), Some(ParameterValue::Float(f)) if (*f - 880.0).abs() < 1e-10),
        "parameter_updates must contain the new frequency (880 Hz)"
    );
}

/// Module state (oscillator phase) is preserved across a parameter-only replan.
///
/// After 100 ticks at 440 Hz, a parameter update to 880 Hz is applied via replan.
/// The surviving oscillator's accumulated phase must not be reset.
#[test]
fn state_preserved_across_parameter_update() {
    let registry = patches_modules::default_registry();

    let graph_a = sine_out_graph("osc", 440.0);
    let (mut plan_a, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP).unwrap();

    let mut pool = ModulePool::new(MODULE_CAP);
    let mut bufs = make_buffer_pool();
    adopt_plan(&mut plan_a, &mut pool);

    // Tick 100 times at 440 Hz.
    const TICKS: usize = 100;
    for i in 0..TICKS {
        plan_a.tick(&mut pool, &mut bufs, i % 2);
    }

    // Replan with freq=880 Hz — parameter-only change, osc survives.
    let graph_b = sine_out_graph("osc", 880.0);
    let (mut plan_b, _) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();
    adopt_plan(&mut plan_b, &mut pool);

    // Tick twice more to propagate the osc output through AudioOut's 1-sample delay.
    // After tick (TICKS+1, wi=TICKS%2): osc outputs sin(phase_after_100), writes to buf[wi].
    // After tick (TICKS+2, wi=(TICKS+1)%2): AudioOut reads buf[(TICKS+1)%2] = sin(phase_after_100).
    for i in TICKS..(TICKS + 2) {
        plan_b.tick(&mut pool, &mut bufs, i % 2);
    }

    let sink_val = pool.read_sink_left();

    // Expected: sin(phase accumulated after TICKS ticks at C0_FREQ+440 Hz).
    // Oscillator uses reference_frequency=C0_FREQ and frequency_offset=440.
    // Simulate the exact phase accumulation to match floating-point behavior.
    let phi_440 = TAU * (C0_FREQ + 440.0) / SAMPLE_RATE;
    let phase_after_100 = {
        let mut p = 0.0f64;
        for _ in 0..TICKS {
            p = (p + phi_440) % TAU;
        }
        p
    };
    let expected = phase_after_100.sin();

    assert!(
        (sink_val - expected).abs() < 1e-5,
        "oscillator phase must be preserved: got {sink_val}, expected {expected}"
    );

    // A re-instantiated oscillator would output 0.0 here; verify the value is non-trivial.
    assert!(
        sink_val.abs() > 1e-3,
        "expected non-zero preserved-phase output; got {sink_val}"
    );
}

/// A plan built with a real (non-zero) sample rate produces modules that use it correctly.
///
/// Verifies that the AudioEnvironment's sample_rate flows into module construction:
/// after 3 ticks, the sink should reflect sin(TAU * freq / sample_rate), which is
/// only finite and correct when sample_rate is properly stored by the oscillator.
#[test]
fn initial_plan_uses_provided_sample_rate() {
    let registry = patches_modules::default_registry();
    const FREQ: f64 = 440.0;
    let graph = sine_out_graph("osc", FREQ);

    let (mut plan, _state) =
        build_patch(&graph, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP).unwrap();

    let mut pool = ModulePool::new(MODULE_CAP);
    let mut bufs = make_buffer_pool();
    adopt_plan(&mut plan, &mut pool);

    // Three ticks propagate osc's first non-zero output through AudioOut's 1-sample delay:
    //   tick 1 (wi=0): osc→sin(0)=0 → buf[0]; AudioOut reads buf[1]=0; sink=0.
    //   tick 2 (wi=1): osc→sin(φ)   → buf[1]; AudioOut reads buf[0]=0; sink=0.
    //   tick 3 (wi=0): osc→sin(2φ)  → buf[0]; AudioOut reads buf[1]=sin(φ); sink=sin(φ).
    for i in 0..3 {
        plan.tick(&mut pool, &mut bufs, i % 2);
    }

    // Oscillator uses reference_frequency=C0_FREQ and frequency_offset=FREQ.
    let phi = TAU * (C0_FREQ + FREQ) / SAMPLE_RATE;
    let expected = phi.sin();
    let sink_val = pool.read_sink_left();

    assert!(
        sink_val.is_finite(),
        "sink output must be finite (NaN/Inf indicates sample_rate=0 was used)"
    );
    assert!(
        (sink_val - expected).abs() < 1e-5,
        "sink must equal sin(TAU * {FREQ} / {SAMPLE_RATE}) ≈ {expected:.6}; got {sink_val}"
    );
}

/// Changing the shape of a sequencer node (e.g. length 4 → 8) forces the old
/// instance to be tombstoned and a fresh one to be created, even though the
/// NodeId and module type are unchanged.
#[test]
fn shape_change_forces_re_instantiation() {
    let registry = patches_modules::default_registry();

    // Build a minimal graph: StepSequencer("seq") → AudioOut("out").
    // First build: length = 4.
    let seq_steps_a: Vec<String> = ["C3", "D3", "E3", "F3"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut seq_params_a = patches_core::parameter_map::ParameterMap::new();
    seq_params_a.insert(
        "steps".to_string(),
        patches_core::parameter_map::ParameterValue::Array(seq_steps_a),
    );

    let mut graph_a = patches_core::ModuleGraph::new();
    graph_a.add_module(
        "seq",
        StepSequencer::describe(&ModuleShape { channels: 0, length: 4 }),
        &seq_params_a,
    ).unwrap();
    graph_a.add_module(
        "out",
        AudioOut::describe(&ModuleShape { channels: 0, length: 0 }),
        &patches_core::parameter_map::ParameterMap::new(),
    ).unwrap();
    graph_a.connect(
        &patches_core::NodeId::from("seq"),
        p("pitch"),
        &patches_core::NodeId::from("out"),
        p("left"),
        1.0,
    ).unwrap();
    graph_a.connect(
        &patches_core::NodeId::from("seq"),
        p("pitch"),
        &patches_core::NodeId::from("out"),
        p("right"),
        1.0,
    ).unwrap();

    let (plan_a, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();

    let seq_id_a = state_a.nodes[&patches_core::NodeId::from("seq")].instance_id;
    assert!(!plan_a.new_modules.is_empty(), "first build must produce new modules");

    // Second build: same NodeId "seq", same module type, but length = 8.
    let seq_steps_b: Vec<String> = ["C3", "D3", "E3", "F3", "G3", "A3", "B3", "C4"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut seq_params_b = patches_core::parameter_map::ParameterMap::new();
    seq_params_b.insert(
        "steps".to_string(),
        patches_core::parameter_map::ParameterValue::Array(seq_steps_b),
    );

    let mut graph_b = patches_core::ModuleGraph::new();
    graph_b.add_module(
        "seq",
        StepSequencer::describe(&ModuleShape { channels: 0, length: 8 }),
        &seq_params_b,
    ).unwrap();
    graph_b.add_module(
        "out",
        AudioOut::describe(&ModuleShape { channels: 0, length: 0 }),
        &patches_core::parameter_map::ParameterMap::new(),
    ).unwrap();
    graph_b.connect(
        &patches_core::NodeId::from("seq"),
        p("pitch"),
        &patches_core::NodeId::from("out"),
        p("left"),
        1.0,
    ).unwrap();
    graph_b.connect(
        &patches_core::NodeId::from("seq"),
        p("pitch"),
        &patches_core::NodeId::from("out"),
        p("right"),
        1.0,
    ).unwrap();

    let (plan_b, state_b) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    let seq_id_b = state_b.nodes[&patches_core::NodeId::from("seq")].instance_id;

    // The shape changed, so the "seq" node must get a brand-new InstanceId.
    assert_ne!(
        seq_id_a, seq_id_b,
        "shape change (length 4 → 8) must produce a new InstanceId for the sequencer"
    );
    // The old instance must be tombstoned.
    assert!(
        !plan_b.tombstones.is_empty(),
        "shape change must tombstone the old sequencer instance"
    );
    // A fresh module must appear in new_modules.
    assert!(
        plan_b.new_modules.iter().any(|(_, m)| m.instance_id() == seq_id_b),
        "shape change must install a new sequencer module"
    );
}
