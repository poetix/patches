pub mod audio_environment;
pub mod build_error;
pub mod graph_yaml;
pub mod graphs;
pub mod modules;
pub mod registries;

// ── Crate-internal path aliases ───────────────────────────────────────────────
// These make `crate::X::Y` paths work inside this crate; they are not public API.
pub(crate) use graphs::planner;
pub(crate) use modules::instance_id;
pub(crate) use modules::module_descriptor;
pub(crate) use registries::module_builder;

// ── Public API ────────────────────────────────────────────────────────────────
pub use audio_environment::AudioEnvironment;
pub use graphs::{GraphError, ModuleGraph, NodeId};
pub use modules::{validate_parameters, ControlSignal, Module, PortConnectivity, Sink};
pub use modules::{ModuleDescriptor, ModuleShape, ParameterDescriptor, ParameterKind, PortDescriptor, PortRef};
pub use modules::{ParameterMap, ParameterValue};
pub use modules::parameter_map;
pub use modules::InstanceId;
pub use registries::ModuleBuilder;
pub use registries::Registry;
pub use graphs::planner::{
    BufferAllocState, GraphIndex, ModuleAllocState, NodeState, PlanError,
    PlannerState, ResolvedGraph,
};
