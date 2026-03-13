/// Environmental parameters supplied to modules once when a plan is activated.
///
/// Modules that depend on these parameters (e.g. oscillators that use `sample_rate`)
/// should store them in [`Module::initialise`] and use the stored copies during
/// [`Module::process`] rather than receiving them per sample.
///
/// `poly_voices` is the number of active polyphony voices. Poly cable buffers always
/// hold 16 channels (`[f64; 16]`) regardless of this value; modules should use
/// `poly_voices` to know how many of those channels carry live data.
#[derive(Debug, Clone, Copy)]
pub struct AudioEnvironment {
    pub sample_rate: f64,
    pub poly_voices: usize,
}