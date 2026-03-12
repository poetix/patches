use std::any::Any;
use std::sync::{Arc, Mutex};

use patches_core::{AudioEnvironment, CableValue, InstanceId, Module, ModuleDescriptor, ModuleShape};
use patches_core::parameter_map::ParameterMap;
use patches_engine::{build_patch, PlannerState};
use patches_modules::{AudioOut, Oscillator};
use patches_integration_tests::HeadlessEngine;

// ── ThreadIdDropSpy ───────────────────────────────────────────────────────────

struct ThreadIdDropSpy {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    drop_thread: Arc<Mutex<Option<String>>>,
}

impl ThreadIdDropSpy {
    fn new(drop_thread: Arc<Mutex<Option<String>>>) -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                module_name: "ThreadIdDropSpy",
                shape: ModuleShape { channels: 0, length: 0 },
                inputs: vec![],
                outputs: vec![],
                parameters: vec![],
                is_sink: false,
            },
            drop_thread,
        }
    }
}

impl Drop for ThreadIdDropSpy {
    fn drop(&mut self) {
        let name = std::thread::current().name().map(str::to_owned);
        *self.drop_thread.lock().unwrap() = name;
    }
}

impl Module for ThreadIdDropSpy {
    fn describe(_shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "ThreadIdDropSpy",
            shape: ModuleShape { channels: 0, length: 0 },
            inputs: vec![],
            outputs: vec![],
            parameters: vec![],
            is_sink: false,
        }
    }

    fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            drop_thread: Arc::new(Mutex::new(None)),
        }
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

// ── Helpers ───────────────────────────────────────────────────────────────────

const POOL_CAP: usize = 256;
const MODULE_CAP: usize = 64;
const ENV: AudioEnvironment = AudioEnvironment { sample_rate: 48_000.0 };

fn sine_out_graph() -> patches_core::ModuleGraph {
    use patches_core::{ModuleGraph, NodeId, PortRef};
    use patches_core::parameter_map::ParameterValue;

    let mut graph = ModuleGraph::new();
    let mut params = ParameterMap::new();
    params.insert("frequency".to_string(), ParameterValue::Float(440.0));
    graph
        .add_module(
            "osc",
            Oscillator::describe(&ModuleShape { channels: 0, length: 0 }),
            &params,
        )
        .unwrap();
    graph
        .add_module(
            "out",
            AudioOut::describe(&ModuleShape { channels: 0, length: 0 }),
            &ParameterMap::new(),
        )
        .unwrap();
    graph
        .connect(
            &NodeId::from("osc"),
            PortRef { name: "sine", index: 0 },
            &NodeId::from("out"),
            PortRef { name: "left", index: 0 },
            1.0,
        )
        .unwrap();
    graph
        .connect(
            &NodeId::from("osc"),
            PortRef { name: "sine", index: 0 },
            &NodeId::from("out"),
            PortRef { name: "right", index: 0 },
            1.0,
        )
        .unwrap();
    graph
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// A module tombstoned during a plan swap must be dropped on the
/// `"patches-cleanup"` thread, not on the calling thread.
///
/// Uses `HeadlessEngine` so no audio hardware is required.
#[test]
fn tombstoned_module_dropped_on_cleanup_thread() {
    let registry = patches_modules::default_registry();
    let graph = sine_out_graph();

    // Build the initial plan (Oscillator → AudioOut).
    let (plan_1, state_1) =
        build_patch(&graph, &registry, &ENV, &PlannerState::empty(), POOL_CAP, MODULE_CAP)
            .unwrap();

    let mut engine = HeadlessEngine::new(plan_1, POOL_CAP, MODULE_CAP);

    // Choose a free module slot for the spy: the next unused index.
    let spy_slot = state_1.module_alloc.next_hwm;

    // Plan 2: same execution order, plus spy installed at spy_slot.
    // The spy is not referenced in the execution order but will be installed
    // in the pool when adopt_plan processes new_modules.
    let drop_thread: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let spy: Box<dyn Module> = Box::new(ThreadIdDropSpy::new(Arc::clone(&drop_thread)));
    let (mut plan_2, _) =
        build_patch(&graph, &registry, &ENV, &state_1, POOL_CAP, MODULE_CAP).unwrap();
    plan_2.new_modules.push((spy_slot, spy));
    engine.adopt_plan(plan_2);

    // Plan 3: same execution order, spy tombstoned.
    let (mut plan_3, _) =
        build_patch(&graph, &registry, &ENV, &state_1, POOL_CAP, MODULE_CAP).unwrap();
    plan_3.tombstones.push(spy_slot);
    engine.adopt_plan(plan_3);

    // stop() drops cleanup_tx (signalling the thread) and joins it,
    // guaranteeing the spy has been dropped before we check.
    engine.stop();

    let recorded = drop_thread.lock().unwrap().clone();
    assert_eq!(
        recorded,
        Some("patches-cleanup".to_owned()),
        "spy must be dropped on the patches-cleanup thread"
    );
}
