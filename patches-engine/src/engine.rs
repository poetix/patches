use std::fmt;
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};

use patches_core::{AudioEnvironment, ControlSignal, InstanceId, Module};

use crate::builder::ExecutionPlan;
use crate::callback::{build_stream, AudioCallback};
use crate::pool::ModulePool;

/// Default module pool capacity: number of `Option<Box<dyn Module>>` slots
/// to pre-allocate on the audio thread.  1024 slots accommodates far more
/// modules than any realistic patch.
pub const DEFAULT_MODULE_POOL_CAPACITY: usize = 1024;

/// Pre-start state: the plan channel consumer, the signal consumer, the
/// cable buffer pool, and the module pool.
/// Stored in [`SoundEngine`] until [`start`](SoundEngine::start) moves them
/// into the audio closure.
struct PendingState {
    plan_rx: rtrb::Consumer<ExecutionPlan>,
    signal_rx: rtrb::Consumer<(InstanceId, ControlSignal)>,
    buffer_pool: Box<[[f64; 2]]>,
    module_pool: ModulePool,
}

/// State captured by [`SoundEngine::open`]: the audio device, its stream
/// configuration, sample format, and channel count. Held until
/// [`start`](SoundEngine::start) uses them to build the output stream.
struct OpenedDevice {
    device: Device,
    config: StreamConfig,
    sample_format: SampleFormat,
    channels: usize,
}

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
    /// The OS refused to spawn the cleanup thread.
    ThreadSpawnError(std::io::Error),
    /// [`start`](SoundEngine::start) was called before [`open`](SoundEngine::open).
    NotOpened,
    /// [`open`](SoundEngine::open) was called after the device has already been opened.
    AlreadyOpened,
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
            EngineError::ThreadSpawnError(e) => write!(f, "failed to spawn cleanup thread: {e}"),
            EngineError::NotOpened => {
                write!(f, "start() called before open(); call open() first")
            }
            EngineError::AlreadyOpened => {
                write!(f, "open() called after the device has already been opened")
            }
        }
    }
}

impl std::error::Error for EngineError {}

/// Drives an [`ExecutionPlan`] continuously, writing stereo output to the
/// default hardware audio device via CPAL.
///
/// The audio callback owns the module pool and cable buffer pool directly -- no
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
/// Modules must be fully constructed and initialised before being sent to the
/// engine. The engine does **not** call [`Module::initialise`]; callers are
/// responsible for initialising modules with the [`AudioEnvironment`] returned
/// by [`open`](Self::open) before passing plans to [`start`](Self::start) or
/// [`swap_plan`](Self::swap_plan).
///
/// ## Lifecycle
///
/// 1. [`new`](Self::new) -- create the engine (no plan, no device).
/// 2. [`open`](Self::open) -- open the audio device, query the sample rate,
///    return an [`AudioEnvironment`]. Does **not** start audio.
/// 3. [`start`](Self::start) -- take a fully-constructed [`ExecutionPlan`]
///    and begin audio processing.
/// 4. [`swap_plan`](Self::swap_plan) -- hot-swap to a new plan at any time.
/// 5. [`stop`](Self::stop) -- stop audio and release the device.
///
/// A `SoundEngine` can be started once. After [`stop`](Self::stop) the plan
/// has been moved into (and dropped with) the audio closure; to run again
/// with a new plan, create a fresh `SoundEngine`.
pub struct SoundEngine {
    /// Write end of the lock-free plan channel.
    plan_tx: rtrb::Producer<ExecutionPlan>,
    /// Write end of the lock-free signal channel.
    signal_tx: rtrb::Producer<(InstanceId, ControlSignal)>,
    /// Consumer end and pools -- stashed here until
    /// [`start`](Self::start) moves them into the audio closure.
    /// `None` after `start()` has been called.
    pending: Option<PendingState>,
    /// Device state captured by [`open`](Self::open), consumed by
    /// [`start`](Self::start).
    opened_device: Option<OpenedDevice>,
    /// Live CPAL stream while the engine is running.
    stream: Option<Stream>,
    /// Number of samples between control-rate ticks (signal dispatch).
    control_period: usize,
    /// Capacity of the module pool; used to size the cleanup ring buffer.
    module_pool_capacity: usize,
    /// Join handle for the cleanup thread spawned in [`start`](Self::start).
    cleanup_thread: Option<thread::JoinHandle<()>>,
}

impl SoundEngine {
    /// Create a new `SoundEngine` with pre-allocated pools but no plan and no
    /// audio device.
    ///
    /// `buffer_pool_capacity` is the number of `[f64; 2]` cable buffer slots.
    /// Slot 0 is the permanent-zero slot; slots 1... are for cable buffers.
    ///
    /// `module_pool_capacity` is the number of `Option<Box<dyn Module>>` slots
    /// in the audio-thread module pool. Must be at least as large as the value
    /// used when building plans via [`build_patch`](crate::build_patch).
    ///
    /// `control_period` is the number of audio samples between control-rate
    /// ticks (signal dispatch). Must be greater than zero; 64 is a sensible
    /// default (~1.3 ms at 48 kHz).
    ///
    /// No audio device is opened until [`open`](Self::open) is called.
    pub fn new(
        buffer_pool_capacity: usize,
        module_pool_capacity: usize,
        control_period: usize,
    ) -> Result<Self, EngineError> {
        if control_period == 0 {
            return Err(EngineError::InvalidControlPeriod);
        }
        let buffer_pool = vec![[0.0_f64; 2]; buffer_pool_capacity].into_boxed_slice();
        let module_pool = ModulePool::new(module_pool_capacity);
        let (plan_tx, plan_rx) = rtrb::RingBuffer::new(1);
        let (signal_tx, signal_rx) = rtrb::RingBuffer::new(64);
        Ok(Self {
            plan_tx,
            signal_tx,
            pending: Some(PendingState { plan_rx, signal_rx, buffer_pool, module_pool }),
            opened_device: None,
            stream: None,
            control_period,
            module_pool_capacity,
            cleanup_thread: None,
        })
    }

    /// Open the default output device and query its configuration.
    ///
    /// Returns an [`AudioEnvironment`] containing the device's sample rate.
    /// The device and configuration are stored internally for use by
    /// [`start`](Self::start). Does **not** start the audio thread.
    ///
    /// Returns [`EngineError::AlreadyOpened`] if called a second time.
    pub fn open(&mut self) -> Result<AudioEnvironment, EngineError> {
        if self.opened_device.is_some() {
            return Err(EngineError::AlreadyOpened);
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

        self.opened_device = Some(OpenedDevice {
            device,
            config,
            sample_format,
            channels,
        });

        Ok(AudioEnvironment { sample_rate })
    }

    /// Begin audio processing with the given [`ExecutionPlan`].
    ///
    /// The plan's modules must already be fully constructed and initialised
    /// (e.g. via [`Module::initialise`] with the [`AudioEnvironment`] returned
    /// by [`open`](Self::open)). The engine installs `new_modules` into the
    /// module pool and starts the audio callback.
    ///
    /// Returns [`EngineError::NotOpened`] if [`open`](Self::open) has not been
    /// called. Returns [`EngineError::AlreadyConsumed`] if the engine has
    /// already been started and stopped.
    pub fn start(&mut self, mut plan: ExecutionPlan) -> Result<(), EngineError> {
        if self.stream.is_some() {
            return Ok(());
        }

        let PendingState { plan_rx, signal_rx, buffer_pool, mut module_pool } =
            self.pending.take().ok_or(EngineError::AlreadyConsumed)?;

        let OpenedDevice { device, config, sample_format, channels } =
            self.opened_device.take().ok_or(EngineError::NotOpened)?;

        // Install the plan's new modules into the pool (already initialised).
        for (idx, module) in plan.new_modules.drain(..) {
            module_pool.install(idx, module);
        }

        let (cleanup_tx, mut cleanup_rx) =
            rtrb::RingBuffer::<Box<dyn Module>>::new(self.module_pool_capacity);

        let cleanup_handle = thread::Builder::new()
            .name("patches-cleanup".to_owned())
            .spawn(move || loop {
                while cleanup_rx.pop().is_ok() {
                    // Module is dropped here, off the audio thread.
                }
                if cleanup_rx.is_abandoned() {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            })
            .map_err(EngineError::ThreadSpawnError)?;

        self.cleanup_thread = Some(cleanup_handle);

        let callback = AudioCallback::new(
            plan_rx, signal_rx, plan, buffer_pool, module_pool, channels,
            self.control_period, cleanup_tx,
        );
        let stream = match sample_format {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, callback),
            SampleFormat::I16 => build_stream::<i16>(&device, &config, callback),
            SampleFormat::U16 => build_stream::<u16>(&device, &config, callback),
            other => return Err(EngineError::UnsupportedSampleFormat(other)),
        }?;

        stream.play().map_err(EngineError::PlayStreamError)?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Stop audio processing and close the device.
    ///
    /// Drops the CPAL stream first (which drops the audio callback and its
    /// `cleanup_tx` producer, signalling the cleanup thread to exit), then
    /// joins the cleanup thread so all tombstoned modules are guaranteed to
    /// have been dropped before this method returns.
    ///
    /// Idempotent: safe to call multiple times or if the engine was never started.
    pub fn stop(&mut self) {
        self.stream.take();
        if let Some(handle) = self.cleanup_thread.take() {
            let _ = handle.join();
        }
    }

    /// Send a new [`ExecutionPlan`] to the audio callback.
    ///
    /// The plan's modules must already be fully constructed and initialised.
    /// The engine does **not** call [`Module::initialise`] on incoming modules.
    ///
    /// The callback will adopt the new plan at the start of its next invocation,
    /// installing new modules and tombstoning removed ones. If the single-slot
    /// channel is already full, the push is a no-op and `new_plan` is returned
    /// as `Err`. Callers may retry; the audio callback drains the slot within
    /// one buffer period (~10 ms).
    ///
    /// This method is wait-free and safe to call from any thread.
    pub fn swap_plan(&mut self, new_plan: ExecutionPlan) -> Result<(), ExecutionPlan> {
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
