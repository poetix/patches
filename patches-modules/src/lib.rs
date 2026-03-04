pub mod adsr_envelope;
pub mod audio_out;
pub mod clock_sequencer;
pub mod oscillator;
pub mod step_sequencer;
pub mod sum;
pub mod vca;
pub mod waveforms;
pub mod glide;

pub use adsr_envelope::AdsrEnvelope;
pub use audio_out::AudioOut;
pub use clock_sequencer::ClockSequencer;
pub use oscillator::SineOscillator;
pub use step_sequencer::StepSequencer;
pub use sum::Sum;
pub use vca::Vca;
pub use waveforms::{SawtoothOscillator, SquareOscillator};
pub use glide::Glide;

pub fn default_registry() -> patches_core::Registry {
    let mut r = patches_core::Registry::new();
    r.register::<SineOscillator>();
    r.register::<Sum>();
    r.register::<Vca>();
    r.register::<AudioOut>();
    r.register::<SawtoothOscillator>();
    r.register::<SquareOscillator>();
    r.register::<AdsrEnvelope>();
    r.register::<ClockSequencer>();
    r.register::<StepSequencer>();
    r.register::<Glide>();
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
        let shape = ModuleShape { channels: 2 };
        let params = ParameterMap::new();

        for name in &[
            "SineOscillator",
            "Sum",
            "Vca",
            "AudioOut",
            "SawtoothOscillator",
            "SquareOscillator",
            "AdsrEnvelope",
            "ClockSequencer",
            "StepSequencer",
            "Glide",
        ] {
            assert!(
                r.create(name, &env, &shape, &params).is_ok(),
                "default_registry() missing module: {name}",
            );
        }
    }
}
