use std::any::Any;

use patches_core::{
    AudioEnvironment, CableKind, CableValue, InstanceId, Module, ModuleDescriptor, ModuleGraph,
    ModuleShape, NodeId, PortDescriptor, PortRef, Registry,
};
use patches_core::parameter_map::{ParameterMap, ParameterValue};
use patches_engine::{build_patch, PlannerState};
use patches_modules::{AudioOut, Oscillator};

// ── constants ─────────────────────────────────────────────────────────────────

const POOL_CAP: usize = 256;
const MODULE_CAP: usize = 64;

// ── Probe module ──────────────────────────────────────────────────────────────

/// A minimal module with one input and one output.
///
/// Local to this test file; never published.
struct Probe {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
}

impl Module for Probe {
    fn describe(_shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Probe",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![PortDescriptor { name: "in", index: 0, kind: CableKind::Mono }],
            outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Mono }],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self { instance_id, descriptor }
    }

    fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn process(&mut self, _pool: &mut [[CableValue; 2]], _wi: usize) {}

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

/// Connectivity notification tests are superseded by the port-objects mechanism
/// (T-0116). The `connectivity_updates` field has been removed from `ExecutionPlan`;
/// connectivity is now delivered via `Module::set_ports`. These tests are retained
/// as stubs — they verify the builder succeeds but do not assert connectivity delivery.

#[test]
fn initial_build_succeeds() {
    let registry = make_registry();
    let graph = probe_to_out_graph();
    let (plan, state) =
        build_patch(&graph, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();
    let probe_slot = pool_index_for(&state, &NodeId::from("probe"));
    assert!(
        plan.new_modules.iter().any(|(idx, _)| *idx == probe_slot),
        "Probe must be in new_modules on initial build"
    );
}

#[test]
fn added_cable_produces_surviving_module() {
    let registry = make_registry();
    let graph_a = probe_to_out_graph();
    let (_, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();
    let graph_b = probe_with_input_graph();
    let (plan_b, _) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();
    // Probe survives (no tombstone for it).
    let probe_slot = pool_index_for(&state_a, &NodeId::from("probe"));
    assert!(
        !plan_b.tombstones.contains(&probe_slot),
        "probe must not be tombstoned when a cable is added"
    );
}

#[test]
fn removed_cable_leaves_probe_surviving() {
    let registry = make_registry();
    let graph_a = probe_with_input_graph();
    let (_, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();
    let probe_slot = pool_index_for(&state_a, &NodeId::from("probe"));
    let graph_b = probe_to_out_graph();
    let (plan_b, _) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();
    assert!(
        !plan_b.tombstones.contains(&probe_slot),
        "probe must not be tombstoned when a cable is removed"
    );
}

#[test]
fn no_new_modules_on_identical_rebuild() {
    let registry = make_registry();
    let graph = probe_to_out_graph();
    let (_, state_a) =
        build_patch(&graph, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();
    let (plan_b, _) =
        build_patch(&graph, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();
    assert!(
        plan_b.new_modules.is_empty(),
        "no new modules on identical rebuild"
    );
}
