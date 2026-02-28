use std::fmt;

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
    /// [`start`](SoundEngine::start) was called a second time after the engine
    /// has already been started and stopped. Create a new [`SoundEngine`] to
    /// restart with a fresh plan.
    AlreadyConsumed,
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
            EngineError::AlreadyConsumed => write!(
                f,
                "engine has already been started and stopped; create a new SoundEngine to restart"
            ),
        }
    }
}

impl std::error::Error for EngineError {}

/// Drives an [`ExecutionPlan`] continuously, writing stereo output to the
/// default hardware audio device via CPAL.
///
/// The audio callback owns the [`ExecutionPlan`] directly — no `Arc`, no
/// `Mutex`. A new plan can be swapped in at any time via
/// [`swap_plan`](Self::swap_plan), which sends it over a wait-free SPSC
/// channel (`rtrb`).
///
/// A `SoundEngine` can be started once. After [`stop`](Self::stop) the plan
/// has been moved into (and dropped with) the audio closure; to run again
/// with a new plan, create a fresh `SoundEngine`.
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
    /// Write end of the lock-free plan channel. Held here so that
    /// [`swap_plan`](Self::swap_plan) can publish new plans at any time.
    plan_tx: rtrb::Producer<ExecutionPlan>,
    /// Consumer end and initial plan, stashed here until [`start`](Self::start)
    /// moves them into the audio closure. `None` after `start()` has been called.
    pending: Option<(rtrb::Consumer<ExecutionPlan>, ExecutionPlan)>,
    /// Live CPAL stream while the engine is running.
    stream: Option<Stream>,
}

impl SoundEngine {
    /// Create a new `SoundEngine` owning the given [`ExecutionPlan`].
    ///
    /// No audio device is opened until [`start`](Self::start) is called.
    pub fn new(plan: ExecutionPlan) -> Result<Self, EngineError> {
        // Capacity-1 ring buffer: one slot is sufficient to queue a single
        // in-flight plan swap. Only the latest plan matters for hot-reload.
        let (plan_tx, plan_rx) = rtrb::RingBuffer::new(1);
        Ok(Self {
            plan_tx,
            pending: Some((plan_rx, plan)),
            stream: None,
        })
    }

    /// Open the default output device and begin audio processing.
    ///
    /// Returns [`EngineError::AlreadyConsumed`] if called after the engine has
    /// already been started and stopped. Returns `Ok(())` if the engine is
    /// already running (no-op).
    pub fn start(&mut self) -> Result<(), EngineError> {
        if self.stream.is_some() {
            return Ok(());
        }

        let (consumer, initial_plan) = self
            .pending
            .take()
            .ok_or(EngineError::AlreadyConsumed)?;

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
                build_stream::<f32>(&device, &config, sample_rate, channels, consumer, initial_plan)
            }
            SampleFormat::I16 => {
                build_stream::<i16>(&device, &config, sample_rate, channels, consumer, initial_plan)
            }
            SampleFormat::U16 => {
                build_stream::<u16>(&device, &config, sample_rate, channels, consumer, initial_plan)
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
    /// returning, so by the time `stop` returns the audio callback has
    /// finished and the [`ExecutionPlan`] it owned has been dropped.
    pub fn stop(&mut self) {
        self.stream.take();
    }

    /// Send a new [`ExecutionPlan`] to the audio callback.
    ///
    /// The callback will adopt the new plan at the start of its next
    /// invocation. If the single-slot channel is already full (i.e. a
    /// previous plan has been queued but not yet consumed), the push is a
    /// no-op and `new_plan` is returned as `Err`. In practice the audio
    /// callback drains the slot within one buffer period (~10 ms), so
    /// callers may simply retry.
    ///
    /// This method is wait-free and safe to call from any thread.
    pub fn swap_plan(&mut self, new_plan: ExecutionPlan) -> Result<(), ExecutionPlan> {
        self.plan_tx.push(new_plan).map_err(|rtrb::PushError::Full(v)| v)
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    sample_rate: f64,
    channels: usize,
    mut consumer: rtrb::Consumer<ExecutionPlan>,
    mut current_plan: ExecutionPlan,
) -> Result<Stream, EngineError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                // Adopt a new plan if one has been published — wait-free, no allocation.
                if let Ok(new_plan) = consumer.pop() {
                    current_plan = new_plan;
                }
                fill_buffer(data, &mut current_plan, sample_rate, channels);
            },
            |err| {
                eprintln!("patches audio stream error: {err}");
            },
            None,
        )
        .map_err(EngineError::BuildStreamError)
}

/// Write one full CPAL output callback buffer without allocating or blocking.
fn fill_buffer<T: cpal::SizedSample + cpal::FromSample<f32>>(
    data: &mut [T],
    plan: &mut ExecutionPlan,
    sample_rate: f64,
    channels: usize,
) {
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
