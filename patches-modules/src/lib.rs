pub mod audio_out;
pub mod crossfade;
pub mod oscillator;

pub use audio_out::AudioOut;
pub use crossfade::Crossfade;
pub use oscillator::SineOscillator;
pub use patches_core::{Module, ModuleDescriptor, PortDescriptor, PortDirection, SampleBuffer};
