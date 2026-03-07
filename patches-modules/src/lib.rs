pub mod adsr_envelope;
pub mod audio_out;
pub mod clock_sequencer;
pub mod filter;
pub mod oscillator;
pub mod step_sequencer;
pub mod sum;
pub mod vca;
pub mod glide;
pub mod lfo;
pub mod tuner;
pub mod common;

pub use adsr_envelope::AdsrEnvelope;
pub use audio_out::AudioOut;
pub use clock_sequencer::ClockSequencer;
pub use filter::ResonantLowpass;
pub use oscillator::Oscillator;
pub use step_sequencer::StepSequencer;
pub use sum::Sum;
pub use vca::Vca;
pub use glide::Glide;
pub use lfo::Lfo;
pub use tuner::Tuner;

pub fn default_registry() -> patches_core::Registry {
    let mut r = patches_core::Registry::new();
    r.register::<Oscillator>();
    r.register::<Sum>();
    r.register::<Vca>();
    r.register::<AudioOut>();
    r.register::<AdsrEnvelope>();
    r.register::<ClockSequencer>();
    r.register::<StepSequencer>();
    r.register::<Glide>();
    r.register::<Lfo>();
    r.register::<ResonantLowpass>();
    r.register::<Tuner>();
    r
}

#[cfg(test)]
mod tests {
    use patches_core::{AudioEnvironment, ModuleShape};
    use patches_core::parameter_map::ParameterMap;

    #[test]
    fn default_registry_contains_all_modules() {
        let r = super::default_registry();
        let env = AudioEnvironment { sample_rate: 44100.0 };
        let shape = ModuleShape { channels: 2, length: 0 };
        let params = ParameterMap::new();

        for name in &[
            "Oscillator",
            "Sum",
            "Vca",
            "AudioOut",
            "AdsrEnvelope",
            "ClockSequencer",
            "StepSequencer",
            "Glide",
            "Lfo",
            "ResonantLowpass",
            "Tuner",
        ] {
            assert!(
                r.create(name, &env, &shape, &params).is_ok(),
                "default_registry() missing module: {name}",
            );
        }
    }
}
