use std::simd::Simd;
use std::simd::num::{SimdFloat, SimdUint};
use std::sync::LazyLock;

type Vf = Simd<f32, 16>;
type Vi = Simd<u32, 16>;

const TABLE_SIZE: usize = 2048;
const TABLE_SIZE_F: f32 = TABLE_SIZE as f32;
const TABLE_SIZE_MASK: usize = TABLE_SIZE - 1;

pub struct SineTable {
    table: [f32; TABLE_SIZE],
}

impl Default for SineTable {
    fn default() -> Self {
        let mut table = [0.0; TABLE_SIZE];
        for i in 0..TABLE_SIZE {
            table[i as usize] = (i as f32 / TABLE_SIZE_F * std::f32::consts::TAU).sin();
        }
        Self { table }
    }
}

impl SineTable {
    pub fn mono_lookup(&self, phase: f32) -> f32 {
        // phase ∈ [0,1)
        let idx_f = phase * TABLE_SIZE_F;   // scaled index
        let idx_int = idx_f as usize;
        let frac = idx_f - (idx_int as f32);

        let next = (idx_int + 1) & TABLE_SIZE_MASK;

        // linear interpolation
        let a = self.table[idx_int];
        let b = self.table[next];
        a + frac * (b - a)
    }

    pub fn poly_lookup(&self, phases: Vf) -> Vf {
        // phases ∈ [0,1)
        let idx_f = phases * Vf::splat(TABLE_SIZE as f32);   // scaled index
        let idx_int = idx_f.cast::<u32>();
        let frac = idx_f - idx_int.cast::<f32>();

        let next = idx_int + Vi::splat(1);
        // wrap around table
        let next = next & Vi::splat(TABLE_SIZE_MASK as u32);

        // gather_or_default requires usize indices
        let idx_usize = idx_int.cast::<usize>();
        let next_usize = next.cast::<usize>();

        // gather from table (linear interpolation)
        let a = Vf::gather_or_default(&self.table, idx_usize);
        let b = Vf::gather_or_default(&self.table, next_usize);

        a + frac * (b - a)
    }
}

pub static SINE_TABLE: LazyLock<SineTable> = LazyLock::new(SineTable::default);

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> SineTable {
        SineTable::default()
    }

    // --- mono_lookup tests ---

    #[test]
    fn mono_zero_is_zero() {
        let t = table();
        let v = t.mono_lookup(0.0);
        assert!(v.abs() < 1e-4, "sin(0) ≈ 0, got {v}");
    }

    #[test]
    fn mono_quarter_is_one() {
        let t = table();
        let v = t.mono_lookup(0.25);
        assert!((v - 1.0).abs() < 1e-3, "sin(π/2) ≈ 1, got {v}");
    }

    #[test]
    fn mono_half_is_zero() {
        let t = table();
        let v = t.mono_lookup(0.5);
        assert!(v.abs() < 1e-3, "sin(π) ≈ 0, got {v}");
    }

    #[test]
    fn mono_three_quarters_is_minus_one() {
        let t = table();
        let v = t.mono_lookup(0.75);
        assert!((v + 1.0).abs() < 1e-3, "sin(3π/2) ≈ -1, got {v}");
    }

    #[test]
    fn mono_interpolates_smoothly() {
        // The lookup is linearly interpolated; compare against f32::sin directly.
        let t = table();
        for i in 0..=100u32 {
            let phase = i as f32 / 100.0;
            let expected = (phase * std::f32::consts::TAU).sin();
            let got = t.mono_lookup(phase);
            assert!(
                (got - expected).abs() < 2e-4,
                "phase={phase:.3}: expected {expected:.6}, got {got:.6}"
            );
        }
    }

    // --- poly_lookup tests ---

    #[test]
    fn poly_matches_mono_for_each_lane() {
        let t = table();
        // 16 evenly-spaced phases covering [0, 1)
        let phases: [f32; 16] = std::array::from_fn(|i| i as f32 / 16.0);
        let poly_out = t.poly_lookup(Vf::from_array(phases)).to_array();
        for (i, (&ph, &pv)) in phases.iter().zip(poly_out.iter()).enumerate() {
            let mono = t.mono_lookup(ph);
            assert!(
                (pv - mono).abs() < 1e-5,
                "lane {i} (phase={ph:.4}): poly={pv:.6} mono={mono:.6}"
            );
        }
    }

    #[test]
    fn poly_known_phases() {
        let t = table();
        let mut phases = [0.0_f32; 16];
        phases[0] = 0.0;    // sin ≈ 0
        phases[4] = 0.25;   // sin ≈ +1
        phases[8] = 0.5;    // sin ≈ 0
        phases[12] = 0.75;  // sin ≈ -1
        let out = t.poly_lookup(Vf::from_array(phases)).to_array();

        assert!(out[0].abs() < 1e-4,         "lane 0 (phase=0.00): got {}", out[0]);
        assert!((out[4] - 1.0).abs() < 1e-3, "lane 4 (phase=0.25): got {}", out[4]);
        assert!(out[8].abs() < 1e-3,         "lane 8 (phase=0.50): got {}", out[8]);
        assert!((out[12] + 1.0).abs() < 1e-3,"lane 12 (phase=0.75): got {}", out[12]);
    }

    #[test]
    fn poly_wrap_at_table_boundary() {
        // Phase very close to 1.0 — the next-index wrap must not go out of bounds.
        let t = table();
        let phase = 1.0_f32 - 1.0 / TABLE_SIZE as f32;
        let mut phases = [0.0_f32; 16];
        phases[0] = phase;
        let out = t.poly_lookup(Vf::from_array(phases)).to_array();
        // Just confirm it doesn't panic and is a reasonable sine value.
        assert!(out[0].abs() <= 1.0, "sine must be in [-1,1], got {}", out[0]);
    }
}
