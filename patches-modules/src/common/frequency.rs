
/// Middle C0 frequency in Hz (MIDI note 0), used as the V/OCT reference pitch.
/// V/OCT oscillators add the user-supplied `frequency_offset` to this value,
/// so a `frequency_offset` of `0.0` places the oscillator at C0 (≈ 16.35 Hz)
/// before any V/OCT input is applied.
pub const C0_FREQ: f32 = 16.351_598;

#[derive(PartialEq)]
pub enum FMMode {
    Linear,
    Exponential,
}

pub struct UnitPhaseAccumulator {
    pub phase: f32,
    pub phase_increment: f32,
    sample_rate_reciprocal: f32,
    frequency_control: FrequencyControl,
    pub is_modulating: bool,
}

impl UnitPhaseAccumulator {
    pub fn new(sample_rate: f32, reference_frequency: f32) -> Self {
        Self {
            phase: 0.0,
            phase_increment: 0.0,
            sample_rate_reciprocal: 1.0 / sample_rate,
            frequency_control: FrequencyControl::new(reference_frequency),
            is_modulating: false,
        }
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    pub fn set_fm_mode(&mut self, fm_mode: FMMode) {
        self.frequency_control.fm_mode = fm_mode;
    }

    pub fn set_modulation(&mut self, voct_modulating: bool, fm_modulating: bool) {
        self.frequency_control.voct_modulating = voct_modulating;
        self.frequency_control.fm_modulating = fm_modulating;
        self.is_modulating = voct_modulating || fm_modulating;
    }

    /// Set the frequency offset (Hz) added to the reference frequency.
    /// Recomputes the static phase increment.
    pub fn set_frequency_offset(&mut self, frequency_offset: f32) {
        self.frequency_control.frequency_offset = frequency_offset;
        let base = self.frequency_control.base_pitch();
        self.update_phase_increment(base);
    }

    fn update_phase_increment(&mut self, frequency: f32) {
        self.phase_increment = frequency * self.sample_rate_reciprocal;
    }

    pub fn advance(&mut self) {
        self.phase += self.phase_increment;
        self.phase -= self.phase.floor(); // Wrap phase to [0.0, 1.0)
    }

    pub fn advance_modulating(&mut self, voct_input: f32, fm_input: f32) {
        let modulated_frequency = self.frequency_control.compute(voct_input, fm_input);
        self.update_phase_increment(modulated_frequency);
        self.advance();
    }
}

struct FrequencyControl {
    reference_frequency: f32,
    frequency_offset: f32,
    pub voct_modulating: bool,
    pub fm_modulating: bool,
    pub fm_mode: FMMode,
    // Cache for the exp2 result: only recomputed when exp_mod changes.
    last_exp_mod: f32,
    cached_exp2: f32,
}

impl FrequencyControl {

    fn new(reference_frequency: f32) -> Self {
        Self {
            reference_frequency,
            frequency_offset: 0.0,
            voct_modulating: false,
            fm_modulating: false,
            fm_mode: FMMode::Linear,
            last_exp_mod: f32::NAN, // sentinel: forces first computation
            cached_exp2: 1.0,
        }
    }

    fn base_pitch(&self) -> f32 {
        self.reference_frequency + self.frequency_offset
    }

    fn compute(&mut self, voct_input: f32, fm_input: f32) -> f32 {
        let mut frequency = self.base_pitch();
        let mut exp_mod = 0.0;
        if self.voct_modulating {
            exp_mod = voct_input;
        }
        if self.fm_modulating && self.fm_mode == FMMode::Exponential {
            exp_mod += fm_input;
        }
        if exp_mod != 0.0 {
            if exp_mod != self.last_exp_mod {
                self.cached_exp2 = exp_mod.exp2();
                self.last_exp_mod = exp_mod;
            }
            frequency *= self.cached_exp2;
        }
        if self.fm_modulating && self.fm_mode == FMMode::Linear {
            frequency += fm_input * 10.0;
        }
        frequency
    }
}
