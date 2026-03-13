use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use patches_core::{
    AudioEnvironment, CableKind, CablePool, GraphError, InstanceId, Module, ModuleDescriptor,
    ModuleGraph, ModuleShape, NodeId, PortDescriptor, PortRef, PolyInput, PolyOutput, Registry,
};
use patches_core::cables::{InputPort, OutputPort};
use patches_core::parameter_map::ParameterMap;
use patches_engine::{build_patch, PlannerState};
use patches_modules::{AudioOut, Oscillator};

// ── constants ─────────────────────────────────────────────────────────────────

const POOL_CAP: usize = 256;
const MODULE_CAP: usize = 64;

// ── PolyProbe ─────────────────────────────────────────────────────────────────

/// Shared state extracted from `PolyProbe` for inspection by tests.
struct PolyProbeState {
    set_ports_count: AtomicUsize,
    input_connected: AtomicBool,
    output_connected: AtomicBool,
    last_received: Mutex<Option<[f64; 16]>>,
}

/// A minimal module with one poly input and one poly output.
///
/// Records connectivity and received values so tests can inspect them
/// after plan adoption and after ticking.
struct PolyProbe {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    poly_in: PolyInput,
    poly_out: PolyOutput,
    shared: Arc<PolyProbeState>,
}

impl Module for PolyProbe {
    fn describe(_shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "PolyProbe",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![PortDescriptor { name: "poly_in", index: 0, kind: CableKind::Poly }],
            outputs: vec![PortDescriptor { name: "poly_out", index: 0, kind: CableKind::Poly }],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            poly_in: PolyInput::default(),
            poly_out: PolyOutput::default(),
            shared: Arc::new(PolyProbeState {
                set_ports_count: AtomicUsize::new(0),
                input_connected: AtomicBool::new(false),
                output_connected: AtomicBool::new(false),
                last_received: Mutex::new(None),
            }),
        }
    }

    fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.poly_in = PolyInput::from_ports(inputs, 0);
        self.poly_out = PolyOutput::from_ports(outputs, 0);
        self.shared.set_ports_count.fetch_add(1, Ordering::Relaxed);
        self.shared.input_connected.store(self.poly_in.connected, Ordering::Relaxed);
        self.shared.output_connected.store(self.poly_out.connected, Ordering::Relaxed);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        if self.poly_in.is_connected() {
            let values = pool.read_poly(&self.poly_in);
            *self.shared.last_received.lock().unwrap() = Some(values);
        }
        if self.poly_out.is_connected() {
            pool.write_poly(&self.poly_out, [0.0; 16]);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── PolySource ────────────────────────────────────────────────────────────────

/// A module with one poly output that always writes a known pattern.
struct PolySource {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    poly_out: PolyOutput,
    pattern: [f64; 16],
}

const POLY_PATTERN: [f64; 16] = {
    let mut arr = [0.0f64; 16];
    let mut i = 0;
    while i < 16 {
        arr[i] = (i as f64 + 1.0) * 0.1;
        i += 1;
    }
    arr
};

impl Module for PolySource {
    fn describe(_shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "PolySource",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![],
            outputs: vec![PortDescriptor { name: "poly_out", index: 0, kind: CableKind::Poly }],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            poly_out: PolyOutput::default(),
            pattern: POLY_PATTERN,
        }
    }

    fn update_validated_parameters(&mut self, _params: &ParameterMap) {}

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, _inputs: &[InputPort], outputs: &[OutputPort]) {
        self.poly_out = PolyOutput::from_ports(outputs, 0);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        pool.write_poly(&self.poly_out, self.pattern);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn env() -> AudioEnvironment {
    AudioEnvironment { sample_rate: 44100.0, poly_voices: 16 }
}

fn p(name: &'static str) -> PortRef {
    PortRef { name, index: 0 }
}

fn make_registry() -> Registry {
    let mut r = Registry::new();
    r.register::<PolyProbe>();
    r.register::<PolySource>();
    r.register::<Oscillator>();
    r.register::<AudioOut>();
    r
}

/// Graph: PolySource("src") → PolyProbe("probe"); Osc("osc") → AudioOut("out").
///
/// The mono osc→out path satisfies the sink requirement; the poly src→probe
/// path is the test subject.
fn connected_graph() -> ModuleGraph {
    use patches_core::parameter_map::ParameterValue;
    let mut graph = ModuleGraph::new();
    let mut osc_params = ParameterMap::new();
    osc_params.insert("frequency".to_string(), ParameterValue::Float(440.0));

    graph.add_module("src", PolySource::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new()).unwrap();
    graph.add_module("probe", PolyProbe::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new()).unwrap();
    graph.add_module("osc", Oscillator::describe(&ModuleShape { channels: 0, length: 0 }), &osc_params).unwrap();
    graph.add_module("out", AudioOut::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new()).unwrap();

    graph.connect(&NodeId::from("src"), p("poly_out"), &NodeId::from("probe"), p("poly_in"), 1.0).unwrap();
    graph.connect(&NodeId::from("osc"), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();
    graph.connect(&NodeId::from("osc"), p("sine"), &NodeId::from("out"), p("right"), 1.0).unwrap();
    graph
}

/// Graph: PolyProbe("probe") alone (poly_in unconnected); Osc("osc") → AudioOut("out").
fn disconnected_graph() -> ModuleGraph {
    use patches_core::parameter_map::ParameterValue;
    let mut graph = ModuleGraph::new();
    let mut osc_params = ParameterMap::new();
    osc_params.insert("frequency".to_string(), ParameterValue::Float(440.0));

    graph.add_module("probe", PolyProbe::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new()).unwrap();
    graph.add_module("osc", Oscillator::describe(&ModuleShape { channels: 0, length: 0 }), &osc_params).unwrap();
    graph.add_module("out", AudioOut::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new()).unwrap();

    graph.connect(&NodeId::from("osc"), p("sine"), &NodeId::from("out"), p("left"), 1.0).unwrap();
    graph.connect(&NodeId::from("osc"), p("sine"), &NodeId::from("out"), p("right"), 1.0).unwrap();
    graph
}

fn pool_index_for(state: &PlannerState, node_id: &NodeId) -> usize {
    let ns = &state.nodes[node_id];
    state.module_alloc.pool_map[&ns.instance_id]
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// After plan adoption, `set_ports` has been called on the probe exactly once,
/// `poly_in.connected` is `true` (the source drives it), and `poly_out.connected`
/// is `false` (nothing consumes it in this graph).
///
/// This verifies that the builder calls `set_ports` with correctly-typed
/// `InputPort::Poly` / `OutputPort::Poly` objects and accurate connectivity.
#[test]
fn initial_port_delivery() {
    let registry = make_registry();
    let graph = connected_graph();
    let (plan, _state) =
        build_patch(&graph, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();

    // Extract shared state from the probe before it's moved into the engine.
    let shared = plan
        .new_modules
        .iter()
        .find_map(|(_, m)| m.as_any().downcast_ref::<PolyProbe>().map(|p| Arc::clone(&p.shared)))
        .expect("PolyProbe must be in new_modules");

    let mut engine = patches_integration_tests::HeadlessEngine::new(plan, POOL_CAP, MODULE_CAP);

    // set_ports was called by the builder inline (before pushing to new_modules).
    assert_eq!(shared.set_ports_count.load(Ordering::Relaxed), 1, "set_ports must be called once");
    assert!(shared.input_connected.load(Ordering::Relaxed), "poly_in must be connected (driven by PolySource)");
    assert!(!shared.output_connected.load(Ordering::Relaxed), "poly_out must be disconnected (nothing consumes it)");

    engine.stop();
}

/// A source writing a known `[f64; 16]` pattern; after two ticks (one for
/// PolySource to write, one for PolyProbe to read), the recorded values match.
#[test]
fn poly_cable_propagation() {
    let registry = make_registry();
    let graph = connected_graph();
    let (plan, _state) =
        build_patch(&graph, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();

    let shared = plan
        .new_modules
        .iter()
        .find_map(|(_, m)| m.as_any().downcast_ref::<PolyProbe>().map(|p| Arc::clone(&p.shared)))
        .expect("PolyProbe must be in new_modules");

    let mut engine = patches_integration_tests::HeadlessEngine::new(plan, POOL_CAP, MODULE_CAP);

    // Tick once so PolySource writes to the cable; tick again so PolyProbe
    // reads from the cable (1-sample ping-pong delay).
    engine.tick();
    engine.tick();

    let recorded = shared.last_received.lock().unwrap();
    assert!(recorded.is_some(), "PolyProbe must have recorded a value");
    assert_eq!(
        recorded.unwrap(),
        POLY_PATTERN,
        "recorded values must match the source pattern"
    );

    engine.stop();
}

/// Connecting a mono output to a poly input must return `GraphError::CableKindMismatch`
/// at graph construction time, before any plan is submitted to the engine.
#[test]
fn kind_mismatch_at_connect() {
    use patches_core::parameter_map::ParameterValue;
    let mut graph = ModuleGraph::new();
    let mut osc_params = ParameterMap::new();
    osc_params.insert("frequency".to_string(), ParameterValue::Float(440.0));

    graph.add_module("osc", Oscillator::describe(&ModuleShape { channels: 0, length: 0 }), &osc_params).unwrap();
    graph.add_module("probe", PolyProbe::describe(&ModuleShape { channels: 0, length: 0 }), &ParameterMap::new()).unwrap();

    let result = graph.connect(&NodeId::from("osc"), p("sine"), &NodeId::from("probe"), p("poly_in"), 1.0);
    assert!(
        matches!(result, Err(GraphError::CableKindMismatch { .. })),
        "connecting mono output to poly input must return CableKindMismatch, got: {result:?}"
    );
}

/// After a cable is removed and the new plan adopted, `poly_in.connected`
/// on the surviving probe is `false`.
#[test]
fn connected_false_after_cable_removal() {
    let registry = make_registry();

    // Build with cable.
    let graph_a = connected_graph();
    let (plan_a, state_a) =
        build_patch(&graph_a, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();

    let shared = plan_a
        .new_modules
        .iter()
        .find_map(|(_, m)| m.as_any().downcast_ref::<PolyProbe>().map(|p| Arc::clone(&p.shared)))
        .expect("PolyProbe must be in new_modules on initial build");

    let mut engine = patches_integration_tests::HeadlessEngine::new(plan_a, POOL_CAP, MODULE_CAP);
    assert!(shared.input_connected.load(Ordering::Relaxed), "poly_in must be connected after initial plan");

    // Build without cable.
    let graph_b = disconnected_graph();
    let (plan_b, _state_b) =
        build_patch(&graph_b, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    // Probe must survive (cable removed, not module removed).
    let probe_slot = pool_index_for(&state_a, &NodeId::from("probe"));
    assert!(
        !plan_b.tombstones.contains(&probe_slot),
        "probe must not be tombstoned when only its cable is removed"
    );

    engine.adopt_plan(plan_b);

    assert!(!shared.input_connected.load(Ordering::Relaxed), "poly_in.connected must be false after cable removal");

    engine.stop();
}

/// An identical reload must not call `set_ports` again on the surviving probe.
#[test]
fn no_spurious_set_ports_on_identical_reload() {
    let registry = make_registry();
    let graph = connected_graph();

    let (plan_a, state_a) =
        build_patch(&graph, &registry, &env(), &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();

    let shared = plan_a
        .new_modules
        .iter()
        .find_map(|(_, m)| m.as_any().downcast_ref::<PolyProbe>().map(|p| Arc::clone(&p.shared)))
        .expect("PolyProbe must be in new_modules");

    let mut engine = patches_integration_tests::HeadlessEngine::new(plan_a, POOL_CAP, MODULE_CAP);
    assert_eq!(shared.set_ports_count.load(Ordering::Relaxed), 1, "set_ports called once on initial build");

    // Identical rebuild.
    let (plan_b, _) =
        build_patch(&graph, &registry, &env(), &state_a, POOL_CAP, MODULE_CAP).unwrap();

    assert!(plan_b.new_modules.is_empty(), "no new modules on identical rebuild");
    assert!(plan_b.port_updates.iter().all(|(idx, _, _)| {
        let probe_slot = pool_index_for(&state_a, &NodeId::from("probe"));
        *idx != probe_slot
    }), "probe must not appear in port_updates on identical rebuild");

    engine.adopt_plan(plan_b);

    assert_eq!(
        shared.set_ports_count.load(Ordering::Relaxed),
        1,
        "set_ports must not be called again on an identical reload"
    );

    engine.stop();
}
