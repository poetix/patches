/// Environmental parameters supplied to modules once when a plan is activated.
///
/// Modules that depend on these parameters (e.g. oscillators that use `sample_rate`)
/// should store them in [`Module::initialise`] and use the stored copies during
/// [`Module::process`] rather than receiving them per sample.
#[derive(Debug, Clone, Copy)]
pub struct AudioEnvironment {
    pub sample_rate: f64,
}