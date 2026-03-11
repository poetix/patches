pub mod audio_environment;
pub mod build_error;
pub mod graph_yaml;
pub mod graphs;
pub mod midi;
pub mod modules;
pub mod registries;

// ── Crate-internal path aliases ───────────────────────────────────────────────
// These make `crate::X::Y` paths work inside this crate; they are not public API.

// ── Public API ────────────────────────────────────────────────────────────────
pub use audio_environment::AudioEnvironment;
pub use graphs::{GraphError, ModuleGraph, NodeId};
pub use midi::{MidiEvent, ReceivesMidi};
pub use modules::{validate_parameters, Module, PortConnectivity, Sink};
pub use modules::{ModuleDescriptor, ModuleShape, ParameterDescriptor, ParameterKind, PortDescriptor, PortRef};
pub use modules::{ParameterMap, ParameterValue};
pub use modules::parameter_map;
pub use modules::InstanceId;
pub use registries::ModuleBuilder;
pub use registries::Registry;
pub use graphs::planner::{
    allocate_buffers, classify_nodes, make_decisions,
    BufferAllocState, BufferAllocation, GraphIndex, ModuleAllocState, NodeDecision, NodeState,
    PlanDecisions, PlanError, PlannerState, ResolvedGraph,
};
