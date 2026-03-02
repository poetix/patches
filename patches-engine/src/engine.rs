use std::fmt;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use patches_core::{AudioEnvironment, ControlSignal, InstanceId, Module};

use crate::builder::ExecutionPlan;

/// Default module pool capacity: number of `Option<Box<dyn Module>>` slots
/// to pre-allocate on the audio thread.  1024 slots accommodates far more
/// modules than any realistic patch.
pub const DEFAULT_MODULE_POOL_CAPACITY: usize = 1024;

/// Pre-start state: the plan channel consumer, the signal consumer, the
/// initial plan, the cable buffer pool, and the module pool.
/// Stored in [`SoundEngine`] until [`start`](SoundEngine::start) moves them
/// into the audio closure.
type PendingState = (
    rtrb::Consumer<ExecutionPlan>,
    rtrb::Consumer<(InstanceId, ControlSignal)>,
    ExecutionPlan,
    Box<[[f64; 2]]>,
    Box<[Option<Box<dyn Module>>]>,
);

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
    /// `control_period` of zero was passed to [`SoundEngine::new`].
    InvalidControlPeriod,
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
            EngineError::InvalidControlPeriod => {
                write!(f, "control_period must be greater than zero")
            }
        }
    }
}

impl std::error::Error for EngineError {}

/// Drives an [`ExecutionPlan`] continuously, writing stereo output to the
/// default hardware audio device via CPAL.
///
/// The audio callback owns the module pool and cable buffer pool directly — no
/// `Arc`, no `Mutex`. A new plan can be swapped in at any time via
/// [`swap_plan`](Self::swap_plan), which sends it over a wait-free SPSC
/// channel (`rtrb`).
///
/// When the audio callback adopts a new plan it:
///   1. Installs `new_modules` into the module pool.
///   2. Takes tombstoned modules out of the pool (dropping them).
///   3. Zeros `to_zero` cable buffer slots.
///   4. Replaces `current_plan`.
///
/// New modules are initialised (via [`Module::initialise`]) with the device's
/// sample rate in [`swap_plan`](Self::swap_plan) before being sent to the
/// audio callback. The initial plan's modules are initialised in
/// [`start`](Self::start).
///
/// A `SoundEngine` can be started once. After [`stop`](Self::stop) the plan
/// has been moved into (and dropped with) the audio closure; to run again
/// with a new plan, create a fresh `SoundEngine`.
pub struct SoundEngine {
    /// Write end of the lock-free plan channel.
    plan_tx: rtrb::Producer<ExecutionPlan>,
    /// Write end of the lock-free signal channel.
    signal_tx: rtrb::Producer<(InstanceId, ControlSignal)>,
    /// Consumer end, initial plan, and pools — stashed here until
    /// [`start`](Self::start) moves them into the audio closure.
    /// `None` after `start()` has been called.
    pending: Option<PendingState>,
    /// Live CPAL stream while the engine is running.
    stream: Option<Stream>,
    /// Sample rate of the open audio device. Set in [`start`](Self::start);
    /// used by [`swap_plan`](Self::swap_plan) to initialise incoming plans.
    sample_rate: Option<f64>,
    /// Number of samples between control-rate ticks (signal dispatch).
    control_period: usize,
}

impl SoundEngine {
    /// Create a new `SoundEngine` owning the given [`ExecutionPlan`] and
    /// pre-allocated cable buffer and module pools.
    ///
    /// `buffer_pool_capacity` is the number of `[f64; 2]` cable buffer slots.
    /// Slot 0 is the permanent-zero slot; slots 1… are for cable buffers.
    ///
    /// `module_pool_capacity` is the number of `Option<Box<dyn Module>>` slots
    /// in the audio-thread module pool. Must be at least as large as the value
    /// used when building plans via [`build_patch`](crate::build_patch).
    ///
    /// `control_period` is the number of audio samples between control-rate
    /// ticks (signal dispatch). Must be greater than zero; 64 is a sensible
    /// default (~1.3 ms at 48 kHz).
    ///
    /// No audio device is opened until [`start`](Self::start) is called.
    pub fn new(
        plan: ExecutionPlan,
        buffer_pool_capacity: usize,
        module_pool_capacity: usize,
        control_period: usize,
    ) -> Result<Self, EngineError> {
        if control_period == 0 {
            return Err(EngineError::InvalidControlPeriod);
        }
        let buffer_pool = vec![[0.0_f64; 2]; buffer_pool_capacity].into_boxed_slice();
        let module_pool: Box<[Option<Box<dyn Module>>]> =
            (0..module_pool_capacity).map(|_| None).collect::<Vec<_>>().into_boxed_slice();
        let (plan_tx, plan_rx) = rtrb::RingBuffer::new(1);
        let (signal_tx, signal_rx) = rtrb::RingBuffer::new(64);
        Ok(Self {
            plan_tx,
            signal_tx,
            pending: Some((plan_rx, signal_rx, plan, buffer_pool, module_pool)),
            stream: None,
            sample_rate: None,
            control_period,
        })
    }

    /// Open the default output device and begin audio processing.
    ///
    /// Initialises the initial plan's new modules with the device's sample rate,
    /// installs them into the module pool, and starts the audio callback.
    ///
    /// Returns [`EngineError::AlreadyConsumed`] if called after the engine has
    /// already been started and stopped. Returns `Ok(())` if the engine is
    /// already running (no-op).
    pub fn start(&mut self) -> Result<(), EngineError> {
        if self.stream.is_some() {
            return Ok(());
        }

        let (consumer, signal_rx, mut initial_plan, buffer_pool, mut module_pool) = self
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

        // Initialise and install the initial plan's new modules into the pool.
        let env = AudioEnvironment { sample_rate };
        for (idx, mut module) in initial_plan.new_modules.drain(..) {
            module.initialise(&env);
            module_pool[idx] = Some(module);
        }
        self.sample_rate = Some(sample_rate);

        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(
                &device, &config, channels, consumer, signal_rx, initial_plan, buffer_pool,
                module_pool, self.control_period,
            ),
            SampleFormat::I16 => build_stream::<i16>(
                &device, &config, channels, consumer, signal_rx, initial_plan, buffer_pool,
                module_pool, self.control_period,
            ),
            SampleFormat::U16 => build_stream::<u16>(
                &device, &config, channels, consumer, signal_rx, initial_plan, buffer_pool,
                module_pool, self.control_period,
            ),
            other => return Err(EngineError::UnsupportedSampleFormat(other)),
        }?;

        stream.play().map_err(EngineError::PlayStreamError)?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Stop audio processing and close the device.
    pub fn stop(&mut self) {
        self.stream.take();
    }

    /// Send a new [`ExecutionPlan`] to the audio callback.
    ///
    /// If the engine has been started, the plan's new modules are initialised
    /// with the device's sample rate before being queued. If the engine has not
    /// yet been started, initialisation is skipped (sample rate is unknown) —
    /// the modules will be initialised when `start` is called.
    ///
    /// The callback will adopt the new plan at the start of its next invocation,
    /// installing new modules and tombstoning removed ones. If the single-slot
    /// channel is already full, the push is a no-op and `new_plan` is returned
    /// as `Err`. Callers may retry; the audio callback drains the slot within
    /// one buffer period (~10 ms).
    ///
    /// This method is wait-free and safe to call from any thread.
    pub fn swap_plan(&mut self, mut new_plan: ExecutionPlan) -> Result<(), ExecutionPlan> {
        if let Some(sr) = self.sample_rate {
            let env = AudioEnvironment { sample_rate: sr };
            for (_, module) in &mut new_plan.new_modules {
                module.initialise(&env);
            }
        }
        self.plan_tx.push(new_plan).map_err(|rtrb::PushError::Full(v)| v)
    }

    /// Enqueue a [`ControlSignal`] for delivery to the module identified by `id`.
    ///
    /// Returns `Err(signal)` if the ring buffer is full. Never blocks.
    pub fn send_signal(
        &mut self,
        id: InstanceId,
        signal: ControlSignal,
    ) -> Result<(), ControlSignal> {
        self.signal_tx
            .push((id, signal))
            .map_err(|rtrb::PushError::Full((_, s))| s)
    }
}

#[allow(clippy::too_many_arguments)]
fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    mut consumer: rtrb::Consumer<ExecutionPlan>,
    mut signal_rx: rtrb::Consumer<(InstanceId, ControlSignal)>,
    mut current_plan: ExecutionPlan,
    mut buffer_pool: Box<[[f64; 2]]>,
    mut module_pool: Box<[Option<Box<dyn Module>>]>,
    control_period: usize,
) -> Result<Stream, EngineError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let mut samples_until_next_control: usize = control_period;
    let mut wi_counter: usize = 0;

    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                // Adopt a new plan if one has been published — wait-free, no allocation.
                if let Ok(mut new_plan) = consumer.pop() {
                    // Tombstone removed modules first: the freelist may have recycled
                    // tombstoned slots for new modules, so we must clear old entries
                    // before installing new ones.
                    for &idx in &new_plan.tombstones {
                        module_pool[idx].take();
                    }
                    // Install new modules (already initialised by swap_plan).
                    for (idx, module) in new_plan.new_modules.drain(..) {
                        module_pool[idx] = Some(module);
                    }
                    // Zero freed/new cable buffer slots.
                    for &i in &new_plan.to_zero {
                        buffer_pool[i] = [0.0; 2];
                    }
                    current_plan = new_plan;
                }
                fill_buffer(
                    data,
                    &mut current_plan,
                    &mut module_pool,
                    &mut buffer_pool,
                    channels,
                    &mut signal_rx,
                    control_period,
                    &mut samples_until_next_control,
                    &mut wi_counter,
                );
            },
            |err| {
                eprintln!("patches audio stream error: {err}");
            },
            None,
        )
        .map_err(EngineError::BuildStreamError)
}

#[allow(clippy::too_many_arguments)]
fn fill_buffer<T: cpal::SizedSample + cpal::FromSample<f32>>(
    data: &mut [T],
    plan: &mut ExecutionPlan,
    module_pool: &mut [Option<Box<dyn Module>>],
    buffer_pool: &mut [[f64; 2]],
    channels: usize,
    signal_rx: &mut rtrb::Consumer<(InstanceId, ControlSignal)>,
    control_period: usize,
    samples_until_next_control: &mut usize,
    wi_counter: &mut usize,
) {
    let frames = if channels > 0 { data.len() / channels } else { 0 };
    let mut remaining = frames;
    let mut out_i: usize = 0;

    while remaining > 0 {
        let chunk = (*samples_until_next_control).min(remaining);

        for _ in 0..chunk {
            let wi = *wi_counter % 2;
            plan.tick(module_pool, buffer_pool, wi);
            let left = plan.last_left(module_pool) as f32;
            let right = plan.last_right(module_pool) as f32;

            if channels == 1 {
                data[out_i] = T::from_sample((left + right) * 0.5_f32);
            } else {
                data[out_i * channels] = T::from_sample(left);
                data[out_i * channels + 1] = T::from_sample(right);
                for c in 2..channels {
                    data[out_i * channels + c] = T::from_sample(0.0_f32);
                }
            }
            out_i += 1;
            *wi_counter += 1;
        }

        *samples_until_next_control -= chunk;
        remaining -= chunk;

        if *samples_until_next_control == 0 {
            while let Ok((id, signal)) = signal_rx.pop() {
                if let Ok(idx) =
                    plan.signal_dispatch.binary_search_by_key(&id, |(k, _)| *k)
                {
                    let pool_idx = plan.signal_dispatch[idx].1;
                    if let Some(module) = module_pool[pool_idx].as_mut() {
                        module.receive_signal(signal);
                    }
                }
            }
            *samples_until_next_control = control_period;
        }
    }
}
