use crate::cables::{CableValue, MonoInput, MonoOutput, PolyInput, PolyOutput};

/// Encapsulates the ping-pong cable buffer pool and the current write index,
/// providing typed read/write accessors for use in [`Module::process`].
///
/// `pool[cable_idx][wi]` is the write slot for the current tick;
/// `pool[cable_idx][1 - wi]` is the read slot (the value written last tick).
///
/// [`Module::process`]: crate::Module::process
pub struct CablePool<'a> {
    pool: &'a mut [[CableValue; 2]],
    wi: usize,
}

impl<'a> CablePool<'a> {
    /// Create a new `CablePool` wrapping `pool` with write index `wi`.
    pub fn new(pool: &'a mut [[CableValue; 2]], wi: usize) -> Self {
        Self { pool, wi }
    }

    /// Read a mono value from `input`, applying `input.scale`.
    ///
    /// # Panics
    /// Panics (via `unreachable!`) if the pool slot holds a `Poly` value —
    /// a well-formed graph never produces this.
    pub fn read_mono(&self, input: &MonoInput) -> f32 {
        let ri = 1 - self.wi;
        match self.pool[input.cable_idx][ri] {
            CableValue::Mono(v) => v * input.scale,
            CableValue::Poly(_) => unreachable!(
                "CablePool::read_mono encountered a Poly cable — graph validation should prevent this"
            ),
        }
    }

    /// Read a 16-channel poly value from `input`, applying `input.scale` to
    /// each channel.
    ///
    /// # Panics
    /// Panics (via `unreachable!`) if the pool slot holds a `Mono` value.
    pub fn read_poly(&self, input: &PolyInput) -> [f32; 16] {
        let ri = 1 - self.wi;
        match self.pool[input.cable_idx][ri] {
            CableValue::Poly(channels) => channels.map(|v| v * input.scale),
            CableValue::Mono(_) => unreachable!(
                "CablePool::read_poly encountered a Mono cable — graph validation should prevent this"
            ),
        }
    }

    /// Write a mono `value` to `output`.
    pub fn write_mono(&mut self, output: &MonoOutput, value: f32) {
        self.pool[output.cable_idx][self.wi] = CableValue::Mono(value);
    }

    /// Write a 16-channel poly `value` to `output`.
    pub fn write_poly(&mut self, output: &PolyOutput, value: [f32; 16]) {
        self.pool[output.cable_idx][self.wi] = CableValue::Poly(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pool(values: &[CableValue]) -> Vec<[CableValue; 2]> {
        values.iter().map(|&v| [v, v]).collect()
    }

    #[test]
    fn read_mono_applies_scale() {
        let mut pool = make_pool(&[CableValue::Mono(4.0)]);
        // wi = 0, so ri = 1; both slots seeded with same value
        let cp = CablePool::new(&mut pool, 0);
        let input = MonoInput { cable_idx: 0, scale: 0.5, connected: true };
        assert_eq!(cp.read_mono(&input), 2.0);
    }

    #[test]
    fn read_poly_applies_scale_to_all_channels() {
        let channels: [f32; 16] = std::array::from_fn(|i| i as f32);
        let mut pool = make_pool(&[CableValue::Poly(channels)]);
        let cp = CablePool::new(&mut pool, 0);
        let input = PolyInput { cable_idx: 0, scale: 2.0, connected: true };
        let result = cp.read_poly(&input);
        for (i, &v) in result.iter().enumerate() {
            assert_eq!(v, i as f32 * 2.0, "channel {i} mismatch");
        }
    }

    #[test]
    fn write_mono_stores_at_write_index() {
        let mut pool = vec![[CableValue::Mono(0.0); 2]];
        {
            let mut cp = CablePool::new(&mut pool, 1);
            let output = MonoOutput { cable_idx: 0, connected: true };
            cp.write_mono(&output, 3.14);
        }
        match pool[0][1] {
            CableValue::Mono(v) => assert_eq!(v, 3.14),
            _ => panic!("expected CableValue::Mono at write index"),
        }
    }

    #[test]
    fn write_poly_stores_at_write_index() {
        let mut pool = vec![[CableValue::Poly([0.0; 16]); 2]];
        let data: [f32; 16] = std::array::from_fn(|i| i as f32 * 0.1);
        {
            let mut cp = CablePool::new(&mut pool, 0);
            let output = PolyOutput { cable_idx: 0, connected: true };
            cp.write_poly(&output, data);
        }
        match pool[0][0] {
            CableValue::Poly(channels) => assert_eq!(channels, data),
            _ => panic!("expected CableValue::Poly at write index"),
        }
    }
}
