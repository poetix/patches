
#[derive(PartialEq)]
pub enum FMMode {
    Linear,
    Exponential,
}

pub struct UnitPhaseAccumulator {
    pub phase: f64,
    phase_increment: f64,
    sample_rate_reciprocal: f64,
    frequency_control: FrequencyControl,
    pub is_modulating: bool,
}

impl UnitPhaseAccumulator {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            phase: 0.0,
            phase_increment: 0.0,
            sample_rate_reciprocal: 1.0 / sample_rate,
            frequency_control: FrequencyControl::new(),
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

    pub fn set_base_frequency(&mut self, frequency: f64) {
        self.frequency_control.base_frequency = frequency;
        self.update_phase_increment(frequency);
    }

    fn update_phase_increment(&mut self, frequency: f64) {
        self.phase_increment = frequency * self.sample_rate_reciprocal;
    }

    pub fn advance(&mut self) {
        self.phase += self.phase_increment;
        self.phase -= self.phase.floor(); // Wrap phase to [0.0, 1.0)
    }

    pub fn advance_modulating(&mut self, voct_input: f64, fm_input: f64) {
        let modulated_frequency = self.frequency_control.compute(voct_input, fm_input);
        self.update_phase_increment(modulated_frequency);
        self.advance();
    }
}

struct FrequencyControl {
    pub base_frequency: f64,
    pub voct_modulating: bool,
    pub fm_modulating: bool,
    pub fm_mode: FMMode,
}

impl FrequencyControl {

    fn new() -> Self {
        Self {
            base_frequency: 440.0,
            voct_modulating: false,
            fm_modulating: false,
            fm_mode: FMMode::Linear,
        }
    }

    fn compute(&self, voct_input: f64, fm_input: f64) -> f64 {
        let mut frequency = self.base_frequency;
        let mut exp_mod = 0.0;
        if self.voct_modulating {
            exp_mod = voct_input;
        }
        if self.fm_modulating && self.fm_mode == FMMode::Exponential {
            exp_mod += fm_input;
        }
        if exp_mod != 0.0 {
            frequency *= exp_mod.exp2();
        }
        if self.fm_modulating && self.fm_mode == FMMode::Linear {
            frequency += fm_input * 10.0;
        }
        frequency
    }
}