use std::fmt;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use crate::builder::ExecutionPlan;

/// Errors returned by [`SoundEngine`] operations.
#[derive(Debug)]
pub enum EngineError {
    /// No default output device is available on this system.
    NoOutputDevice,
    /// Failed to query the device's default stream configuration.
    DefaultConfigError(cpal::DefaultStreamConfigError),
    /// Failed to build the output stream.
    BuildStreamError(cpal::BuildStreamError),
    /// Failed to start stream playback.
    PlayStreamError(cpal::PlayStreamError),
    /// The device's native sample format is not supported by this engine.
    UnsupportedSampleFormat(SampleFormat),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::NoOutputDevice => write!(f, "no default output device available"),
            EngineError::DefaultConfigError(e) => {
                write!(f, "failed to get device config: {e}")
            }
            EngineError::BuildStreamError(e) => write!(f, "failed to build stream: {e}"),
            EngineError::PlayStreamError(e) => write!(f, "failed to play stream: {e}"),
            EngineError::UnsupportedSampleFormat(fmt) => {
                write!(f, "unsupported sample format: {fmt:?}")
            }
        }
    }
}

impl std::error::Error for EngineError {}

/// Drives an [`ExecutionPlan`] continuously, writing stereo output to the
/// default hardware audio device via CPAL.
///
/// ```no_run
/// # use patches_engine::{SoundEngine, EngineError};
/// # fn example(plan: patches_engine::ExecutionPlan) -> Result<(), EngineError> {
/// let mut engine = SoundEngine::new(plan)?;
/// engine.start()?;
/// // … patch runs until stop() is called …
/// engine.stop();
/// # Ok(())
/// # }
/// ```
pub struct SoundEngine {
    /// Shared with the audio callback; protected by a Mutex so `stop` can
    /// reclaim the plan and the engine is restartable.
    plan: Arc<Mutex<ExecutionPlan>>,
    /// Holds the live CPAL stream while the engine is running.
    stream: Option<Stream>,
}

impl SoundEngine {
    /// Create a new `SoundEngine` owning the given [`ExecutionPlan`].
    ///
    /// No audio device is opened until [`start`](Self::start) is called.
    pub fn new(plan: ExecutionPlan) -> Result<Self, EngineError> {
        Ok(Self {
            plan: Arc::new(Mutex::new(plan)),
            stream: None,
        })
    }

    /// Open the default output device and begin audio processing.
    ///
    /// If the engine is already running this is a no-op.
    pub fn start(&mut self) -> Result<(), EngineError> {
        if self.stream.is_some() {
            return Ok(());
        }

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(EngineError::NoOutputDevice)?;

        let supported = device
            .default_output_config()
            .map_err(EngineError::DefaultConfigError)?;

        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let sample_rate = f64::from(config.sample_rate.0);
        let channels = usize::from(config.channels);

        let stream = match sample_format {
            SampleFormat::F32 => {
                self.build_stream::<f32>(&device, &config, sample_rate, channels)
            }
            SampleFormat::I16 => {
                self.build_stream::<i16>(&device, &config, sample_rate, channels)
            }
            SampleFormat::U16 => {
                self.build_stream::<u16>(&device, &config, sample_rate, channels)
            }
            other => return Err(EngineError::UnsupportedSampleFormat(other)),
        }?;

        stream.play().map_err(EngineError::PlayStreamError)?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Stop audio processing and close the device.
    ///
    /// Dropping the [`Stream`] causes CPAL to join the audio thread before
    /// returning, so by the time `stop` returns the audio callback is finished.
    /// The engine can be restarted by calling [`start`](Self::start) again.
    pub fn stop(&mut self) {
        self.stream.take();
    }

    fn build_stream<T>(
        &self,
        device: &cpal::Device,
        config: &StreamConfig,
        sample_rate: f64,
        channels: usize,
    ) -> Result<Stream, EngineError>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let plan = Arc::clone(&self.plan);

        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    fill_buffer(data, &plan, sample_rate, channels);
                },
                |err| {
                    eprintln!("patches audio stream error: {err}");
                },
                None,
            )
            .map_err(EngineError::BuildStreamError)
    }
}

/// Write one full CPAL output callback buffer without allocating or blocking.
///
/// Uses [`try_lock`](Mutex::try_lock) so the audio thread never sleeps waiting
/// for the mutex. In the unlikely event the lock is held (e.g. during a
/// future hot-reload plan swap), the buffer is filled with silence.
fn fill_buffer<T: cpal::SizedSample + cpal::FromSample<f32>>(
    data: &mut [T],
    plan: &Arc<Mutex<ExecutionPlan>>,
    sample_rate: f64,
    channels: usize,
) {
    let Ok(mut plan) = plan.try_lock() else {
        for s in data.iter_mut() {
            *s = T::from_sample(0.0_f32);
        }
        return;
    };

    let frames = if channels > 0 {
        data.len() / channels
    } else {
        0
    };

    for i in 0..frames {
        plan.tick(sample_rate);
        let left = plan.last_left() as f32;
        let right = plan.last_right() as f32;

        if channels == 1 {
            data[i] = T::from_sample((left + right) * 0.5_f32);
        } else {
            data[i * channels] = T::from_sample(left);
            data[i * channels + 1] = T::from_sample(right);
            // Any additional channels beyond stereo receive silence.
            for c in 2..channels {
                data[i * channels + c] = T::from_sample(0.0_f32);
            }
        }
    }
}
