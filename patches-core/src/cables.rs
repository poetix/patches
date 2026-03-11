/// The arity of a cable: mono (single f64) or poly (16-channel f64 array).
#[derive(Clone, Debug)]
pub enum CableKind {
    Mono,
    Poly,
}

/// A value carried by a cable. `Poly` holds exactly 16 channels; no heap
/// allocation is required.
#[derive(Clone, Debug)]
pub enum CableValue {
    Mono(f64),
    Poly([f64; 16]),
}

// ── Concrete port structs ──────────────────────────────────────────────────

/// A mono input port. `cable_idx` indexes the shared cable pool; `scale` is
/// applied on read; `connected` tracks whether a cable is attached.
#[derive(Clone, Debug)]
pub struct MonoInput {
    pub cable_idx: usize,
    pub scale: f64,
    pub connected: bool,
}

impl Default for MonoInput {
    fn default() -> Self {
        Self { cable_idx: 0, scale: 1.0, connected: false }
    }
}

impl MonoInput {
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Read the current value from `pool`, applying `self.scale`.
    ///
    /// # Panics
    /// Panics (via `unreachable!`) in debug builds if the pool slot holds a
    /// `CableValue::Poly` value — a well-formed graph never produces this.
    pub fn read(&self, pool: &[CableValue]) -> f64 {
        match pool[self.cable_idx] {
            CableValue::Mono(v) => v * self.scale,
            CableValue::Poly(_) => unreachable!(
                "MonoInput::read encountered a Poly cable — graph validation should prevent this"
            ),
        }
    }
}

/// A poly input port (16-channel).
#[derive(Clone, Debug)]
pub struct PolyInput {
    pub cable_idx: usize,
    pub scale: f64,
    pub connected: bool,
}

impl Default for PolyInput {
    fn default() -> Self {
        Self { cable_idx: 0, scale: 1.0, connected: false }
    }
}

impl PolyInput {
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Read all 16 channels from `pool`, applying `self.scale` to each.
    ///
    /// Returns `[f64; 16]` by value (stack-allocated, no heap allocation).
    ///
    /// # Panics
    /// Panics (via `unreachable!`) in debug builds if the pool slot holds a
    /// `CableValue::Mono` value — a well-formed graph never produces this.
    pub fn read(&self, pool: &[CableValue]) -> [f64; 16] {
        match pool[self.cable_idx] {
            CableValue::Poly(channels) => channels.map(|v| v * self.scale),
            CableValue::Mono(_) => unreachable!(
                "PolyInput::read encountered a Mono cable — graph validation should prevent this"
            ),
        }
    }
}

/// A mono output port.
#[derive(Clone, Debug, Default)]
pub struct MonoOutput {
    pub cable_idx: usize,
    pub connected: bool,
}

impl MonoOutput {
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Write `value` into `pool` at `self.cable_idx`.
    pub fn write(&self, pool: &mut [CableValue], value: f64) {
        pool[self.cable_idx] = CableValue::Mono(value);
    }
}

/// A poly output port (16-channel).
#[derive(Clone, Debug, Default)]
pub struct PolyOutput {
    pub cable_idx: usize,
    pub connected: bool,
}

impl PolyOutput {
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Write a 16-channel `value` into `pool` at `self.cable_idx`.
    pub fn write(&self, pool: &mut [CableValue], value: [f64; 16]) {
        pool[self.cable_idx] = CableValue::Poly(value);
    }
}

// ── Enum wrappers for heterogeneous port delivery ─────────────────────────

/// Heterogeneous input-port wrapper used by the planner to deliver ports to
/// `Module::set_ports` without boxing.
#[derive(Clone, Debug)]
pub enum InputPort {
    Mono(MonoInput),
    Poly(PolyInput),
}

/// Heterogeneous output-port wrapper used by the planner to deliver ports to
/// `Module::set_ports` without boxing.
#[derive(Clone, Debug)]
pub enum OutputPort {
    Mono(MonoOutput),
    Poly(PolyOutput),
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mono_pool(value: f64) -> Vec<CableValue> {
        vec![CableValue::Mono(value)]
    }

    fn poly_pool(channels: [f64; 16]) -> Vec<CableValue> {
        vec![CableValue::Poly(channels)]
    }

    // MonoInput::read --------------------------------------------------------

    #[test]
    fn mono_input_read_scale_one() {
        let pool = mono_pool(2.5);
        let port = MonoInput { cable_idx: 0, scale: 1.0, connected: true };
        assert_eq!(port.read(&pool), 2.5);
    }

    #[test]
    fn mono_input_read_with_scale() {
        let pool = mono_pool(2.0);
        let port = MonoInput { cable_idx: 0, scale: 0.5, connected: true };
        assert_eq!(port.read(&pool), 1.0);
    }

    // PolyInput::read --------------------------------------------------------

    #[test]
    fn poly_input_read_applies_scale_to_all_channels() {
        let channels: [f64; 16] = std::array::from_fn(|i| i as f64);
        let pool = poly_pool(channels);
        let port = PolyInput { cable_idx: 0, scale: 2.0, connected: true };
        let result = port.read(&pool);
        for (i, &v) in result.iter().enumerate() {
            assert_eq!(v, i as f64 * 2.0, "channel {i} mismatch");
        }
    }

    // is_connected -----------------------------------------------------------

    #[test]
    fn is_connected_mono_input() {
        assert!(!MonoInput::default().is_connected());
        assert!(MonoInput { cable_idx: 0, scale: 1.0, connected: true }.is_connected());
    }

    #[test]
    fn is_connected_poly_input() {
        assert!(!PolyInput::default().is_connected());
        assert!(PolyInput { cable_idx: 0, scale: 1.0, connected: true }.is_connected());
    }

    #[test]
    fn is_connected_mono_output() {
        assert!(!MonoOutput::default().is_connected());
        assert!(MonoOutput { cable_idx: 0, connected: true }.is_connected());
    }

    #[test]
    fn is_connected_poly_output() {
        assert!(!PolyOutput::default().is_connected());
        assert!(PolyOutput { cable_idx: 0, connected: true }.is_connected());
    }

    // MonoOutput::write / PolyOutput::write round-trips ---------------------

    #[test]
    fn mono_output_write_round_trip() {
        let mut pool = vec![CableValue::Mono(0.0)];
        let port = MonoOutput { cable_idx: 0, connected: true };
        port.write(&mut pool, 3.14);
        match pool[0] {
            CableValue::Mono(v) => assert_eq!(v, 3.14),
            _ => panic!("expected CableValue::Mono"),
        }
    }

    #[test]
    fn poly_output_write_round_trip() {
        let mut pool = vec![CableValue::Poly([0.0; 16])];
        let port = PolyOutput { cable_idx: 0, connected: true };
        let data: [f64; 16] = std::array::from_fn(|i| i as f64 * 0.1);
        port.write(&mut pool, data);
        match pool[0] {
            CableValue::Poly(channels) => assert_eq!(channels, data),
            _ => panic!("expected CableValue::Poly"),
        }
    }
}
