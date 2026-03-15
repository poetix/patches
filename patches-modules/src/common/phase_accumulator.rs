use std::simd::{Simd, Mask, Select};
use std::simd::cmp::SimdPartialOrd;

type Vf = Simd<f32, 16>;
type Vi = Simd<u32, 16>;

pub struct PolyPhaseAccumulator {
    sample_rate_reciprocal: f32,
    phases: Vf,
    increments: Vf,
}

impl PolyPhaseAccumulator {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate_reciprocal: 1.0 / sample_rate,
            phases: Vf::splat(0.0),
            increments: Vf::splat(0.0),
        }
    }

    pub fn reset(&mut self) {
        self.phases = Vf::splat(0.0);
    }

    pub fn sync_on_zero_crossing(&mut self, previous: Vf, current: Vf) {
        let voice_mask = previous.simd_lt(Vf::splat(0.0)) & current.simd_ge(Vf::splat(0.0));
        // Reset phase to 0.0 for voices where voice_mask is true.
        let reset_phases = Vf::splat(0.0);
        self.phases = voice_mask.select(reset_phases, self.phases);
    }

    pub fn set_frequencies(&mut self, frequencies: Vf) {
        self.increments = frequencies * self.sample_rate_reciprocal;
    }

    pub fn advance(&mut self) {
        let mut p = self.phases + self.increments;
        let one = Vf::splat(1.0);

        p -= p.simd_ge(one).select(one, Vf::splat(0.0));

        self.phases = p;
    }
}

