use patches_core::{Module, PortConnectivity};
use patches_core::parameter_map::ParameterMap;

/// Audio-thread-owned pool of module instances.
///
/// Wraps a pre-allocated `Box<[Option<Box<dyn Module>>]>` together with a
/// cached record of the installed [`Sink`] module. Each operation is named
/// clearly: [`tombstone`](Self::tombstone), [`install`](Self::install),
/// [`process`](Self::process), [`has_sink`](Self::has_sink),
/// [`read_sink_left`](Self::read_sink_left),
/// and [`read_sink_right`](Self::read_sink_right).
///
/// When a module implementing [`Sink`] is installed, the pool records its
/// slot index and begins updating a `last_sink_left / last_sink_right` cache
/// on every [`process`](Self::process) call for that slot.
/// [`read_sink_left`](Self::read_sink_left) and
/// [`read_sink_right`](Self::read_sink_right) return the cached values as
/// plain field reads — no vtable dispatch in the hot audio path.
/// The cache and slot registration are cleared automatically by
/// [`tombstone`](Self::tombstone).
///
/// All operations are index-based and allocation-free.
pub struct ModulePool {
    modules: Box<[Option<Box<dyn Module>>]>,
    /// Pool index of the installed [`Sink`] module, if any.
    sink_slot: Option<usize>,
    /// Cached left-channel output from the most recent [`process`](Self::process)
    /// call on the sink slot. `0.0` when no sink is registered.
    last_sink_left: f64,
    /// Cached right-channel output from the most recent [`process`](Self::process)
    /// call on the sink slot. `0.0` when no sink is registered.
    last_sink_right: f64,
}

impl ModulePool {
    /// Allocate a pool with `capacity` empty slots.
    pub fn new(capacity: usize) -> Self {
        Self {
            modules: (0..capacity).map(|_| None).collect::<Vec<_>>().into_boxed_slice(),
            sink_slot: None,
            last_sink_left: 0.0,
            last_sink_right: 0.0,
        }
    }

    /// Remove the module at `idx`, leaving the slot empty, and return it.
    ///
    /// Returns `None` if the slot was already empty.
    /// If the slot held the registered sink, the sink cache is cleared.
    pub fn tombstone(&mut self, idx: usize) -> Option<Box<dyn Module>> {
        let module = self.modules[idx].take();
        if self.sink_slot == Some(idx) {
            self.sink_slot = None;
            self.last_sink_left = 0.0;
            self.last_sink_right = 0.0;
        }
        module
    }

    /// Install `module` at `idx`, replacing any previous occupant.
    ///
    /// If `module` implements [`Sink`], it is registered as the pool's sink
    /// and will begin supplying cached output values. If a non-sink module
    /// replaces the registered sink slot, the registration is cleared.
    pub fn install(&mut self, idx: usize, module: Box<dyn Module>) {
        if module.as_sink().is_some() {
            self.sink_slot = Some(idx);
        } else if self.sink_slot == Some(idx) {
            self.sink_slot = None;
            self.last_sink_left = 0.0;
            self.last_sink_right = 0.0;
        }
        self.modules[idx] = Some(module);
    }

    /// Call [`Module::process`] on the module at `idx` with the given scratch buffers.
    ///
    /// If `idx` is the sink slot, the sink's `last_left` and `last_right` values
    /// are captured into the pool's cache immediately after processing.
    ///
    /// # Panics
    /// Panics if slot `idx` is empty. Callers must ensure the plan and pool are
    /// consistent (all slots referenced by the active plan are populated).
    pub fn process(&mut self, idx: usize, inputs: &[f64], outputs: &mut [f64]) {
        let m = self.modules[idx].as_mut().unwrap();
        m.process(inputs, outputs);
        if self.sink_slot == Some(idx) {
            if let Some(s) = m.as_sink() {
                self.last_sink_left = s.last_left();
                self.last_sink_right = s.last_right();
            }
        }
    }

    /// Apply pre-validated parameter updates to the module at `idx`.
    ///
    /// Calls [`Module::update_validated_parameters`] on the module at `idx`.
    /// This is infallible — no `Result` is returned. Does nothing if the slot
    /// is empty (the module may have been tombstoned between the plan being built
    /// and adopted).
    pub fn update_parameters(&mut self, idx: usize, params: &ParameterMap) {
        if let Some(m) = self.modules[idx].as_mut() {
            m.update_validated_parameters(params);
        }
    }

    /// Deliver a MIDI event to the module at `idx`.
    ///
    /// `offset` is the sample offset within the current sub-block at which the
    /// event should take effect. No-op stub until T-0111 introduces the
    /// `ReceiveMidi` trait. Does nothing if the slot is empty.
    pub fn receive_midi(&mut self, _idx: usize, _offset: usize, _event: crate::midi::MidiEvent) {}

    /// Deliver a connectivity update to the module at `idx`.
    ///
    /// Calls [`Module::set_connectivity`] on the module at `idx`.
    /// This is infallible — no `Result` is returned. Does nothing if the slot
    /// is empty (the module may have been tombstoned between the plan being built
    /// and adopted).
    pub fn set_connectivity(&mut self, idx: usize, conn: PortConnectivity) {
        if let Some(m) = self.modules[idx].as_mut() {
            m.set_connectivity(conn);
        }
    }

    /// Returns `true` if a [`Sink`] module is currently installed in the pool.
    pub fn has_sink(&self) -> bool {
        self.sink_slot.is_some()
    }

    /// Left-channel output of the registered sink after the most recent tick.
    ///
    /// Returns `0.0` if no sink is registered. This is a plain field read —
    /// no vtable dispatch.
    pub fn read_sink_left(&self) -> f64 {
        self.last_sink_left
    }

    /// Right-channel output of the registered sink after the most recent tick.
    ///
    /// Returns `0.0` if no sink is registered. This is a plain field read —
    /// no vtable dispatch.
    pub fn read_sink_right(&self) -> f64 {
        self.last_sink_right
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;

    use patches_core::{
        AudioEnvironment, InstanceId, Module, ModuleDescriptor, ModuleShape,
        PortDescriptor, Sink,
    };
    use patches_core::parameter_map::ParameterMap;

    use super::*;

    // ── Test-only modules ─────────────────────────────────────────────────────

    /// Outputs a constant value on its single output port.
    struct ConstSource {
        id: InstanceId,
        value: f64,
        desc: ModuleDescriptor,
    }

    impl ConstSource {
        fn new(value: f64) -> Self {
            Self {
                id: InstanceId::next(),
                value,
                desc: ModuleDescriptor {
                    module_name: "ConstSource",
                    shape: ModuleShape { channels: 0, length: 0 },
                    inputs: vec![],
                    outputs: vec![PortDescriptor { name: "out", index: 0 }],
                    parameters: vec![],
                    is_sink: false,
                },
            }
        }
    }

    impl Module for ConstSource {
        fn describe(_shape: &ModuleShape) -> ModuleDescriptor {
            ModuleDescriptor {
                module_name: "ConstSource",
                shape: ModuleShape { channels: 0, length: 0 },
                inputs: vec![],
                outputs: vec![PortDescriptor { name: "out", index: 0 }],
                parameters: vec![],
                is_sink: false,
            }
        }
        fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
            Self { id: instance_id, value: 0.0, desc: descriptor }
        }
        fn update_validated_parameters(&mut self, _params: &ParameterMap) {}
        fn descriptor(&self) -> &ModuleDescriptor {
            &self.desc
        }
        fn instance_id(&self) -> InstanceId {
            self.id
        }
        fn process(&mut self, _inputs: &[f64], outputs: &mut [f64]) {
            outputs[0] = self.value;
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Records the last value it received on its single input port.
    /// Implements [`Sink`] so tests can verify output via
    /// [`ModulePool::read_sink_left`] and [`ModulePool::read_sink_right`].
    struct RecordingSink {
        id: InstanceId,
        last: f64,
        desc: ModuleDescriptor,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                id: InstanceId::next(),
                last: 0.0,
                desc: ModuleDescriptor {
                    module_name: "RecordingSink",
                    shape: ModuleShape { channels: 0, length: 0 },
                    inputs: vec![PortDescriptor { name: "in", index: 0 }],
                    outputs: vec![],
                    parameters: vec![],
                    is_sink: true,
                },
            }
        }
    }

    impl Module for RecordingSink {
        fn describe(_shape: &ModuleShape) -> ModuleDescriptor {
            ModuleDescriptor {
                module_name: "RecordingSink",
                shape: ModuleShape { channels: 0, length: 0 },
                inputs: vec![PortDescriptor { name: "in", index: 0 }],
                outputs: vec![],
                parameters: vec![],
                is_sink: true,
            }
        }
        fn prepare(_env: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
            Self { id: instance_id, last: 0.0, desc: descriptor }
        }
        fn update_validated_parameters(&mut self, _params: &ParameterMap) {}
        fn descriptor(&self) -> &ModuleDescriptor {
            &self.desc
        }
        fn instance_id(&self) -> InstanceId {
            self.id
        }
        fn process(&mut self, inputs: &[f64], _outputs: &mut [f64]) {
            self.last = inputs[0];
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_sink(&self) -> Option<&dyn Sink> {
            Some(self)
        }
    }

    impl Sink for RecordingSink {
        fn last_left(&self) -> f64 {
            self.last
        }
        fn last_right(&self) -> f64 {
            self.last
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn new_pool_has_no_sink() {
        let pool = ModulePool::new(4);
        assert!(!pool.has_sink(), "no sink registered after new()");
        assert_eq!(pool.read_sink_left(), 0.0);
        assert_eq!(pool.read_sink_right(), 0.0);
    }

    #[test]
    fn process_writes_module_output_to_slice() {
        let mut pool = ModulePool::new(4);
        pool.install(2, Box::new(ConstSource::new(0.75)));
        let mut out = [0.0_f64];
        pool.process(2, &[], &mut out);
        assert_eq!(out[0], 0.75);
    }

    #[test]
    fn process_forwards_inputs_to_module() {
        let mut pool = ModulePool::new(4);
        pool.install(0, Box::new(RecordingSink::new()));
        let mut no_out: [f64; 0] = [];
        pool.process(0, &[0.42], &mut no_out);
        assert!((pool.read_sink_left() - 0.42).abs() < 1e-9);
    }

    #[test]
    fn install_replaces_previous_occupant() {
        let mut pool = ModulePool::new(4);
        pool.install(0, Box::new(ConstSource::new(1.0)));
        pool.install(0, Box::new(ConstSource::new(2.0)));
        let mut out = [0.0_f64];
        pool.process(0, &[], &mut out);
        assert_eq!(out[0], 2.0, "slot should hold the most recently installed module");
    }

    #[test]
    fn tombstone_clears_sink() {
        let mut pool = ModulePool::new(4);
        pool.install(1, Box::new(RecordingSink::new()));
        pool.tombstone(1);
        assert!(!pool.has_sink(), "sink must be unregistered after tombstone");
        assert_eq!(pool.read_sink_left(), 0.0);
        assert_eq!(pool.read_sink_right(), 0.0);
    }

    #[test]
    #[should_panic]
    fn process_on_empty_slot_panics() {
        let mut pool = ModulePool::new(4);
        let mut out = [0.0_f64];
        pool.process(0, &[], &mut out);
    }

    #[test]
    fn non_sink_install_does_not_register_sink() {
        let mut pool = ModulePool::new(4);
        pool.install(0, Box::new(ConstSource::new(0.0)));
        assert!(!pool.has_sink(), "non-sink install must not register a sink");
    }

    #[test]
    fn sink_install_registers_sink() {
        let mut pool = ModulePool::new(4);
        pool.install(0, Box::new(RecordingSink::new()));
        assert!(pool.has_sink(), "sink install must register the sink");
    }

    #[test]
    fn read_sink_reflects_last_processed_value() {
        let mut pool = ModulePool::new(4);
        pool.install(0, Box::new(RecordingSink::new()));
        let mut no_out: [f64; 0] = [];
        pool.process(0, &[0.7], &mut no_out);
        assert!((pool.read_sink_left() - 0.7).abs() < 1e-9);
        assert!((pool.read_sink_right() - 0.7).abs() < 1e-9);
    }
}
