pub mod adsr;
pub mod audio_out;
pub mod clock;
pub mod filter;
pub mod midi_in;
pub mod oscillator;
pub mod seq;
pub mod sum;
pub mod vca;
pub mod glide;
pub mod lfo;
pub mod tuner;
pub mod common;

pub use adsr::Adsr;
pub use audio_out::AudioOut;
pub use clock::Clock;
pub use filter::ResonantLowpass;
pub use midi_in::MonoMidiIn;
pub use oscillator::Oscillator;
pub use seq::Seq;
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
    r.register::<Adsr>();
    r.register::<Clock>();
    r.register::<Seq>();
    r.register::<Glide>();
    r.register::<Lfo>();
    r.register::<ResonantLowpass>();
    r.register::<Tuner>();
    r.register::<MonoMidiIn>();
    r
}

#[cfg(test)]
mod tests {
    use patches_core::{AudioEnvironment, InstanceId, ModuleShape};
    use patches_core::parameter_map::ParameterMap;

    #[test]
    fn default_registry_contains_all_modules() {
        let r = super::default_registry();
        let env = AudioEnvironment { sample_rate: 44100.0 };
        let shape = ModuleShape { channels: 2, length: 0 };
        let params = ParameterMap::new();

        for name in &[
            "Osc",
            "Sum",
            "Vca",
            "AudioOut",
            "Adsr",
            "Clock",
            "Seq",
            "Glide",
            "Lfo",
            "Filter",
            "Tuner",
            "MidiIn",
        ] {
            assert!(
                r.create(name, &env, &shape, &params, InstanceId::next()).is_ok(),
                "default_registry() missing module: {name}",
            );
        }
    }
}
