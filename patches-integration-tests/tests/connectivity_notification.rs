use std::any::Any;

use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleGraph, ModuleShape, NodeId,
    PortConnectivity, PortDescriptor, PortRef, Registry,
};
use patches_core::parameter_map::{ParameterMap, ParameterValue};
use patches_engine::{build_patch, PlannerState};
use patches_modules::{AudioOut, Oscillator};

// ── constants ─────────────────────────────────────────────────────────────────

const POOL_CAP: usize = 256;
const MODULE_CAP: usize = 64;

// ── Probe module ──────────────────────────────────────────────────────────────

/// A minimal module with one input and one output that records every
/// [`PortConnectivity`] it receives via [`Module::set_connectivity`].
///
/// Local to this test file; never published.
struct Probe {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    connectivity_history: Vec<PortConnectivity>,
}

impl Module for Probe {
    fn describe(_shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Probe",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![PortDescriptor { name: "in", index: 0 }],
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            connectivity_history: Vec::new(),
        }
    }

    fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        outputs[0] = inputs[0];
    }

    fn set_connectivity(&mut self, connectivity: PortConnectivity) {
        self.connectivity_history.push(connectivity);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn env() -> AudioEnvironment {
    AudioEnvironment { sample_rate: 44100.0 }
}

fn p(name: &'static str) -> PortRef {
    PortRef { name, index: 0 }
}

fn make_registry() -> Registry {
    let mut r = Registry::new();
    r.register::<Probe>();
    r.register::<Oscillator>();
    r.register::<AudioOut>();
    r
}

/// Probe("probe") → AudioOut("out").
/// probe.in is unconnected; probe.out feeds both left and right.
fn probe_to_out_graph() -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    graph
        .add_module("probe", Probe::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new())
        .unwrap();
    graph
        .add_module("out", AudioOut::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new())
        .unwrap();
    graph
        .connect(&NodeId::from("probe"), p("out"), &NodeId::from("out"), p("left"), 1.0)
        .unwrap();
    graph
        .connect(&NodeId::from("probe"), p("out"), &NodeId::from("out"), p("right"), 1.0)
        .unwrap();
    graph
}

/// Osc("osc") → probe.in, Probe("probe") → AudioOut("out").
/// Both probe.in and probe.out are connected.
fn probe_with_input_graph() -> ModuleGraph {
    let mut graph = ModuleGraph::new();
    let mut params = ParameterMap::new();
    params.insert("frequency".to_string(), ParameterValue::Float(440.0));
    graph
        .add_module("osc", Oscillator::describe(&ModuleShape { channels: 0, length: 0 }), &params)
        .unwrap();
    graph
        .add_module("probe", Probe::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new())
        .unwrap();
    graph
        .add_module("out", AudioOut::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new())
        .unwrap();
    graph
        .connect(&NodeId::from("osc"), p("sine"), &NodeId::from("probe"), p("in"), 1.0)
        .unwrap();
    graph
        .connect(&NodeId::from("probe"), p("out"), &NodeId::from("out"), p("left"), 1.0)
        .unwrap();
    graph
        .connect(&NodeId::from("probe"), p("out"), &NodeId::from("out"), p("right"), 1.0)
        .unwrap();
    graph
}

fn pool_index_for(state: &PlannerState, node_id: &NodeId) -> usize {
    let ns = &state.nodes[node_id];
    state.module_alloc.pool_map[&ns.instance_id]
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// On the initial plan, the builder calls `set_connectivity` on each fresh module.
/// With probe.in unconnected and probe.out connected to AudioOut, the module
/// should record inputs=[false], outputs=[true].
#[test]
fn initial_connectivity_set_on_fresh_module() {
    let registry = make_registry();
    let graph = probe_to_out_graph();

    let (plan, state) =
        build_patch(&graph, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();

    let probe_slot = pool_index_for(&state, &NodeId::from("probe"));
    let probe = plan
        .new_modules
        .iter()
        .find(|(idx, _)| *idx == probe_slot)
        .map(|(_, m)| m.as_any().downcast_ref::<Probe>().unwrap())
        .expect("Probe must be in new_modules");

    assert_eq!(
        probe.connectivity_history.len(),
        1,
        "set_connectivity must be called exactly once during the initial build"
    );
    let conn = &probe.connectivity_history[0];
    assert!(!conn.inputs[0], "probe.in must be reported as unconnected");
    assert!(conn.outputs[0], "probe.out must be reported as connected");
}

/// Replanning with a new cable feeding probe.in must emit a `connectivity_updates`
/// entry for probe's pool slot, indicating its input is now connected.
#[test]
fn added_cable_produces_connectivity_update() {
    let registry = make_registry();

    // plan_a: probe.in disconnected.
    let graph_a = probe_to_out_graph();
    let (_, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();
    let probe_slot = pool_index_for(&state_a, &NodeId::from("probe"));

    // plan_b: osc → probe.in added; probe survives with changed connectivity.
    let graph_b = probe_with_input_graph();
    let (plan_b, _) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    let update = plan_b
        .connectivity_updates
        .iter()
        .find(|(idx, _)| *idx == probe_slot);
    assert!(
        update.is_some(),
        "connectivity_updates must contain an entry for probe when a cable is added"
    );
    let (_, conn) = update.unwrap();
    assert!(conn.inputs[0], "probe.in must now be connected after adding a cable");
    assert!(conn.outputs[0], "probe.out must remain connected");
}

/// Replanning with the cable to probe.in removed must emit a `connectivity_updates`
/// entry for probe's pool slot, indicating its input is now disconnected.
#[test]
fn removed_cable_produces_connectivity_update() {
    let registry = make_registry();

    // plan_a: osc → probe.in connected.
    let graph_a = probe_with_input_graph();
    let (_, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();
    let probe_slot = pool_index_for(&state_a, &NodeId::from("probe"));

    // plan_b: osc removed; probe.in is now disconnected.
    let graph_b = probe_to_out_graph();
    let (plan_b, _) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    let update = plan_b
        .connectivity_updates
        .iter()
        .find(|(idx, _)| *idx == probe_slot);
    assert!(
        update.is_some(),
        "connectivity_updates must contain an entry for probe when a cable is removed"
    );
    let (_, conn) = update.unwrap();
    assert!(!conn.inputs[0], "probe.in must now be disconnected after removing the cable");
    assert!(conn.outputs[0], "probe.out must remain connected");
}

/// Replanning with no topology change must produce no `connectivity_updates` entry
/// for probe — connectivity diffing must suppress spurious notifications.
#[test]
fn no_spurious_update_when_topology_unchanged() {
    let registry = make_registry();

    let graph = probe_to_out_graph();
    let (_, state_a) =
        build_patch(&graph, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();
    let probe_slot = pool_index_for(&state_a, &NodeId::from("probe"));

    let (plan_b, _) =
        build_patch(&graph, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    assert!(
        !plan_b.connectivity_updates.iter().any(|(idx, _)| *idx == probe_slot),
        "connectivity_updates must be empty for probe when the topology is unchanged"
    );
}
