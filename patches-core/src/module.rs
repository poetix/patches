use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Describes a single port on a module by name and index.
///
/// The `index` field is the user-visible number in a multi-port group (e.g.
/// `in/2` has `name = "in"` and `index = 2`). For modules with a single port
/// of a given name, `index` is `0`. The position of a `PortDescriptor` in
/// `ModuleDescriptor::inputs` / `outputs` determines the slice offset passed to
/// `Module::process`; `index` is semantically distinct from that position.
#[derive(Debug, Clone)]
pub struct PortDescriptor {
    pub name: &'static str,
    pub index: u32,
}

/// A reference to a named, indexed port used in `ModuleGraph::connect()`.
///
/// Port names are always `&'static str` (defined by module implementations at
/// compile time), so producing a `PortRef` never allocates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRef {
    pub name: &'static str,
    pub index: u32,
}

/// Describes the full port layout of a module.
///
/// Inputs and outputs are stored in separate vecs. The index of a port in
/// `inputs` corresponds to the index in the `inputs` slice passed to
/// [`Module::process`], and similarly for `outputs`. The graph and patch builder
/// use this to resolve port names to slice indices at build time.
#[derive(Debug, Clone)]
pub struct ModuleDescriptor {
    pub inputs: Vec<PortDescriptor>,
    pub outputs: Vec<PortDescriptor>,
}

/// Environmental parameters supplied to modules once when a plan is activated.
///
/// Modules that depend on these parameters (e.g. oscillators that use `sample_rate`)
/// should store them in [`Module::initialise`] and use the stored copies during
/// [`Module::process`] rather than receiving them per sample.
#[derive(Debug, Clone, Copy)]
pub struct AudioEnvironment {
    pub sample_rate: f64,
}

/// A stable, unique identifier assigned to a module instance at construction time.
///
/// `InstanceId` is immutable for the lifetime of the module and survives across
/// plan rebuilds, enabling the [`ModuleInstanceRegistry`](crate::ModuleInstanceRegistry)
/// to match old instances to their counterparts in a new plan.
///
/// IDs are generated from a global atomic counter; no two independently constructed
/// modules will share an `InstanceId` within a single process run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId(u64);

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InstanceId({})", self.0)
    }
}

static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(0);

impl InstanceId {
    /// Allocate a fresh `InstanceId`. Each call returns a distinct value.
    pub fn next() -> Self {
        Self(NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// A non-audio-rate parameter update delivered to a module via [`Module::receive_signal`].
///
/// Signals are passed by value; modules that need to store the payload can do so directly.
/// New variants may be added in future; module implementations should use a wildcard arm
/// (`_ => {}`) to stay forward-compatible.
///
/// `ControlSignal` implements [`Send`] so it can be queued in the engine's ring buffer
/// (see T-0038). All current variants use only `Send`-safe types (`&'static str`, `f64`).
#[derive(Debug, Clone)]
pub enum ControlSignal {
    /// A single named float parameter update (e.g. frequency, gain).
    Float { name: &'static str, value: f64 },
}

/// The core trait all audio modules implement.
///
/// `initialise` is called once when a plan is activated (or re-activated after a
/// hot-reload). `process` is then called once per sample. Both `inputs` and
/// `outputs` are indexed according to the module's [`ModuleDescriptor`].
///
/// `as_any` enables downcasting from `&dyn Module` to a concrete type.
/// `as_sink` lets the patch builder detect a sink node without knowing its
/// concrete type — see [`Sink`].
///
/// There is deliberately no `as_any_mut`: no production code needs mutable
/// downcasting, and test code that inspects module state can use `as_any` +
/// `downcast_ref` instead.
pub trait Module: Send {
    fn descriptor(&self) -> &ModuleDescriptor;
    /// The stable identity of this module instance.
    ///
    /// Must be assigned at construction time (e.g. via [`InstanceId::next()`]) and
    /// return the same value for the lifetime of the instance.
    fn instance_id(&self) -> InstanceId;
    /// Called once when the plan containing this module is activated.
    ///
    /// Modules that depend on environment parameters (e.g. sample rate) should
    /// store what they need here. The default implementation is a no-op.
    fn initialise(&mut self, _env: &AudioEnvironment) {}
    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]);
    /// Deliver a non-audio-rate control signal to this module.
    ///
    /// The default implementation is a no-op; modules that respond to control signals
    /// override this method. Unknown signal variants or parameter names should be
    /// silently ignored.
    fn receive_signal(&mut self, _signal: ControlSignal) {}
    fn as_any(&self) -> &dyn std::any::Any;
    /// Returns `Some(self)` if this module is a [`Sink`], `None` otherwise.
    fn as_sink(&self) -> Option<&dyn Sink> {
        None
    }
}

/// Marker trait for modules that act as the final audio output in a patch.
///
/// The patch builder uses this to locate the sink node without knowledge of
/// any concrete type. Implement this alongside [`Module`] and override
/// [`Module::as_sink`] to return `Some(self)`.
pub trait Sink: Module {
    /// Left-channel sample stored during the most recent [`Module::process`] call.
    fn last_left(&self) -> f64;
    /// Right-channel sample stored during the most recent [`Module::process`] call.
    fn last_right(&self) -> f64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_descriptor_fields() {
        let p = PortDescriptor { name: "freq", index: 0 };
        assert_eq!(p.name, "freq");
        assert_eq!(p.index, 0);
    }

    #[test]
    fn module_descriptor_port_counts() {
        let desc = ModuleDescriptor {
            inputs: vec![PortDescriptor { name: "in", index: 0 }],
            outputs: vec![
                PortDescriptor { name: "out_l", index: 0 },
                PortDescriptor { name: "out_r", index: 0 },
            ],
        };
        assert_eq!(desc.inputs.len(), 1);
        assert_eq!(desc.outputs.len(), 2);
        assert_eq!(desc.inputs[0].name, "in");
        assert_eq!(desc.outputs[1].name, "out_r");
    }

    #[test]
    fn instance_ids_are_unique() {
        let a = InstanceId::next();
        let b = InstanceId::next();
        assert_ne!(a, b);
    }
}
