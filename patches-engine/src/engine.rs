use std::fmt;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use patches_core::{AudioEnvironment, ControlSignal, InstanceId};

use crate::builder::ExecutionPlan;

/// Pre-start state: the plan channel consumer, the signal consumer, the
/// initial plan, and the buffer pool.
/// Stored in [`SoundEngine`] until [`start`](SoundEngine::start) moves them
/// into the audio closure.
type PendingState = (
    rtrb::Consumer<ExecutionPlan>,
    rtrb::Consumer<(InstanceId, ControlSignal)>,
    ExecutionPlan,
    Box<[[f64; 2]]>,
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
/// The audio callback owns the [`ExecutionPlan`] directly — no `Arc`, no
/// `Mutex`. A new plan can be swapped in at any time via
/// [`swap_plan`](Self::swap_plan), which sends it over a wait-free SPSC
/// channel (`rtrb`).
///
/// Plans are initialised (via [`ExecutionPlan::initialise`]) with the device's
/// sample rate before being sent to the audio callback — either in
/// [`start`](Self::start) for the initial plan, or in
/// [`swap_plan`](Self::swap_plan) for subsequent hot-reloads. Calling
/// `swap_plan` before `start` skips initialisation (sample rate not yet known);
/// the plan will be initialised when adopted by the audio callback if the engine
/// is later started.
///
/// A `SoundEngine` can be started once. After [`stop`](Self::stop) the plan
/// has been moved into (and dropped with) the audio closure; to run again
/// with a new plan, create a fresh `SoundEngine`.
///
/// ```no_run
/// # use patches_engine::{SoundEngine, EngineError};
/// # fn example(plan: patches_engine::ExecutionPlan) -> Result<(), EngineError> {
/// let mut engine = SoundEngine::new(plan, 4096, 64)?;
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
    /// Write end of the lock-free signal channel. Held here so that
    /// [`send_signal`](Self::send_signal) can enqueue signals at any time.
    signal_tx: rtrb::Producer<(InstanceId, ControlSignal)>,
    /// Consumer end, initial plan, and buffer pool — stashed here until
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
    /// Create a new `SoundEngine` owning the given [`ExecutionPlan`] and a
    /// pre-allocated cable buffer pool.
    ///
    /// `pool_capacity` is the number of `[f64; 2]` slots to pre-allocate.
    /// Slot 0 is the permanent-zero slot; slots 1… are for cable buffers.
    /// A capacity of 4096 accommodates up to 4096 concurrent output ports.
    ///
    /// `control_period` is the number of audio samples between control-rate
    /// ticks (signal dispatch). Must be greater than zero; 64 is a sensible
    /// default (~1.3 ms at 48 kHz).
    ///
    /// No audio device is opened until [`start`](Self::start) is called.
    pub fn new(
        plan: ExecutionPlan,
        pool_capacity: usize,
        control_period: usize,
    ) -> Result<Self, EngineError> {
        if control_period == 0 {
            return Err(EngineError::InvalidControlPeriod);
        }
        let pool = vec![[0.0_f64; 2]; pool_capacity].into_boxed_slice();
        // Capacity-1 ring buffer: one slot is sufficient to queue a single
        // in-flight plan swap. Only the latest plan matters for hot-reload.
        let (plan_tx, plan_rx) = rtrb::RingBuffer::new(1);
        // Signal ring buffer: capacity 64. At 64-sample control period and
        // 48 kHz this gives headroom for ~64 messages per ~1.3 ms tick.
        let (signal_tx, signal_rx) = rtrb::RingBuffer::new(64);
        Ok(Self {
            plan_tx,
            signal_tx,
            pending: Some((plan_rx, signal_rx, plan, pool)),
            stream: None,
            sample_rate: None,
            control_period,
        })
    }

    /// Open the default output device and begin audio processing.
    ///
    /// Initialises the pending plan with the device's sample rate before
    /// starting the audio callback.
    ///
    /// Returns [`EngineError::AlreadyConsumed`] if called after the engine has
    /// already been started and stopped. Returns `Ok(())` if the engine is
    /// already running (no-op).
    pub fn start(&mut self) -> Result<(), EngineError> {
        if self.stream.is_some() {
            return Ok(());
        }

        let (consumer, signal_rx, mut initial_plan, pool) = self
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

        // Initialise plan before handing it to the audio callback.
        let env = AudioEnvironment { sample_rate };
        initial_plan.initialise(&env);
        self.sample_rate = Some(sample_rate);

        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(
                &device, &config, channels, consumer, signal_rx, initial_plan, pool,
                self.control_period,
            ),
            SampleFormat::I16 => build_stream::<i16>(
                &device, &config, channels, consumer, signal_rx, initial_plan, pool,
                self.control_period,
            ),
            SampleFormat::U16 => build_stream::<u16>(
                &device, &config, channels, consumer, signal_rx, initial_plan, pool,
                self.control_period,
            ),
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
    /// If the engine has been started, the plan is initialised with the
    /// device's sample rate before being queued. If the engine has not yet
    /// been started, initialisation is skipped (sample rate is unknown).
    ///
    /// The callback will adopt the new plan at the start of its next
    /// invocation. If the single-slot channel is already full (i.e. a
    /// previous plan has been queued but not yet consumed), the push is a
    /// no-op and `new_plan` is returned as `Err`. In practice the audio
    /// callback drains the slot within one buffer period (~10 ms), so
    /// callers may simply retry.
    ///
    /// This method is wait-free and safe to call from any thread.
    pub fn swap_plan(&mut self, mut new_plan: ExecutionPlan) -> Result<(), ExecutionPlan> {
        if let Some(sr) = self.sample_rate {
            new_plan.initialise(&AudioEnvironment { sample_rate: sr });
        }
        self.plan_tx.push(new_plan).map_err(|rtrb::PushError::Full(v)| v)
    }

    /// Enqueue a [`ControlSignal`] for delivery to the module identified by `id`.
    ///
    /// The signal is pushed onto the lock-free ring buffer (capacity 64). The
    /// audio callback drains the buffer at each control-rate tick and dispatches
    /// signals to the appropriate module using a binary search on
    /// `ExecutionPlan::signal_dispatch`.
    ///
    /// Returns `Err(signal)` if the ring buffer is full. The caller may drop
    /// the signal or retry. This method never blocks.
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
    mut pool: Box<[[f64; 2]]>,
    control_period: usize,
) -> Result<Stream, EngineError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    // Both counters are initialised once and persist across every callback
    // invocation for the lifetime of this stream.
    let mut samples_until_next_control: usize = control_period;
    let mut wi_counter: usize = 0;

    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                // Adopt a new plan if one has been published — wait-free, no allocation.
                // wi_counter and samples_until_next_control continue uninterrupted.
                if let Ok(new_plan) = consumer.pop() {
                    // Zero freed/new slots before the first tick with the new plan.
                    // The audio thread is the sole writer of the pool.
                    for &i in &new_plan.to_zero {
                        pool[i] = [0.0; 2];
                    }
                    current_plan = new_plan;
                }
                fill_buffer(
                    data,
                    &mut current_plan,
                    &mut pool,
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

/// Write one full CPAL output callback buffer without allocating or blocking.
///
/// Processes samples in chunks aligned to the control period. At each control
/// tick (when `samples_until_next_control` reaches zero) the signal ring buffer
/// is drained and any queued `(InstanceId, ControlSignal)` pairs are dispatched
/// to the matching module via `ExecutionPlan::signal_dispatch`.
///
/// `wi_counter` increments monotonically; `wi_counter % 2` gives the write slot
/// index for each sample, preserving the ping-pong semantics of the old fixed
/// `[0, 1]` pair loop while allowing arbitrary chunk sizes.
#[allow(clippy::too_many_arguments)]
fn fill_buffer<T: cpal::SizedSample + cpal::FromSample<f32>>(
    data: &mut [T],
    plan: &mut ExecutionPlan,
    pool: &mut [[f64; 2]],
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

        // Tight inner loop: no branches on control state.
        for _ in 0..chunk {
            let wi = *wi_counter % 2;
            plan.tick(pool, wi);
            let left = plan.last_left() as f32;
            let right = plan.last_right() as f32;

            if channels == 1 {
                data[out_i] = T::from_sample((left + right) * 0.5_f32);
            } else {
                data[out_i * channels] = T::from_sample(left);
                data[out_i * channels + 1] = T::from_sample(right);
                // Any additional channels beyond stereo receive silence.
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
            // Control tick: drain the signal ring buffer and dispatch.
            while let Ok((id, signal)) = signal_rx.pop() {
                if let Ok(idx) =
                    plan.signal_dispatch.binary_search_by_key(&id, |(k, _)| *k)
                {
                    let slot_idx = plan.signal_dispatch[idx].1;
                    plan.slots[slot_idx].module.receive_signal(signal);
                }
                // Signals for unknown InstanceIds are silently dropped.
            }
            *samples_until_next_control = control_period;
        }
    }
}
