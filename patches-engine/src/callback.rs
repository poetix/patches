use std::time::Instant;

use cpal::traits::DeviceTrait;
use cpal::{Stream, StreamConfig};

use patches_core::Module;

use crate::builder::ExecutionPlan;
use crate::engine::EngineError;
use crate::midi::{AudioClock, EventQueueConsumer};
use crate::pool::ModulePool;

/// Number of samples per MIDI sub-block.
///
/// Every `SUB_BLOCK_SIZE` samples the audio callback drains the MIDI event
/// queue and delivers pending events to all MIDI-receiving modules.
pub(crate) const SUB_BLOCK_SIZE: u64 = 64;

/// All state owned by the audio callback — plan, pools, channels, and
/// control-rate bookkeeping — gathered into one place so the callback
/// closure is a single `callback.fill_buffer(data, info)` call.
pub(crate) struct AudioCallback {
    plan_rx: rtrb::Consumer<ExecutionPlan>,
    current_plan: ExecutionPlan,
    buffer_pool: Box<[[f64; 2]]>,
    module_pool: ModulePool,
    channels: usize,
    /// `channels.trailing_zeros()` — the right-shift to convert a sample count to a frame count.
    channel_shift: u32,
    /// Samples remaining until the next 64-sample MIDI sub-block boundary.
    samples_until_next_midi: usize,
    wi_counter: usize,
    /// Running sample counter, incremented by `SUB_BLOCK_SIZE` after each sub-block.
    /// Used as the `window_start` argument to `EventQueueConsumer::drain_window`
    /// and as the sample-count payload published to `AudioClock`.
    sample_counter: u64,
    /// Consumer end of the MIDI event queue. `None` until a MIDI connector is wired up (T-0110).
    event_queue: Option<EventQueueConsumer>,
    /// Raw pointer to the shared audio clock. The `SoundEngine` owns the `Arc<AudioClock>`
    /// that keeps the allocation alive; the callback holds a raw pointer so that no
    /// Arc refcount operations occur on the audio thread.
    ///
    /// # Safety
    /// Valid for the entire lifetime of the callback: `SoundEngine` drops the stream
    /// (and thus this callback) before releasing its `Arc<AudioClock>`.
    clock: *const AudioClock,
    /// Producer end of the cleanup ring buffer. Tombstoned modules are pushed here
    /// so that deallocation happens on the cleanup thread, not the audio thread.
    /// Dropping the stream drops this producer, signalling the cleanup thread to exit.
    cleanup_tx: rtrb::Producer<Box<dyn Module>>,
}

// SAFETY: `AudioCallback` is sent to the audio thread exactly once (when the
// CPAL stream is built) and never accessed from any other thread after that.
// The raw `*const AudioClock` is read-only on the audio thread and points to
// data owned by `SoundEngine`'s `Arc<AudioClock>`, which outlives the stream.
unsafe impl Send for AudioCallback {}

impl AudioCallback {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        plan_rx: rtrb::Consumer<ExecutionPlan>,
        current_plan: ExecutionPlan,
        buffer_pool: Box<[[f64; 2]]>,
        module_pool: ModulePool,
        channels: usize,
        event_queue: Option<EventQueueConsumer>,
        clock: *const AudioClock,
        cleanup_tx: rtrb::Producer<Box<dyn Module>>,
    ) -> Self {
        Self {
            plan_rx,
            current_plan,
            buffer_pool,
            module_pool,
            channels,
            channel_shift: channels.trailing_zeros(),
            samples_until_next_midi: SUB_BLOCK_SIZE as usize,
            wi_counter: 0,
            sample_counter: 0,
            event_queue,
            clock,
            cleanup_tx,
        }
    }

    fn process_chunk<T: cpal::SizedSample + cpal::FromSample<f32>>(
        &mut self,
        data: &mut [T],
        out_i: &mut usize,
        chunk: usize,
    ) {
        for _ in 0..chunk {
            let wi = self.wi_counter % 2;
            self.current_plan.tick(&mut self.module_pool, &mut self.buffer_pool, wi);
            let left = self.module_pool.read_sink_left() as f32;
            let right = self.module_pool.read_sink_right() as f32;

            if self.channels == 1 {
                data[*out_i] = T::from_sample((left + right) * 0.5_f32);
            } else {
                data[*out_i * self.channels] = T::from_sample(left);
                data[*out_i * self.channels + 1] = T::from_sample(right);
                for c in 2..self.channels {
                    data[*out_i * self.channels + c] = T::from_sample(0.0_f32);
                }
            }
            *out_i += 1;
            self.wi_counter += 1;
        }
    }

    /// Adopt a new plan if one has been published — wait-free, no allocation.
    fn receive_plan(&mut self) {
        if let Ok(mut new_plan) = self.plan_rx.pop() {
            // Tombstone removed modules first: the freelist may have recycled
            // tombstoned slots for new modules, so we must clear old entries
            // before installing new ones.
            for &idx in &new_plan.tombstones {
                if let Some(module) = self.module_pool.tombstone(idx) {
                    if let Err(rtrb::PushError::Full(module)) = self.cleanup_tx.push(module) {
                        eprintln!(
                            "patches: cleanup ring buffer full — dropping module on audio thread (slot {idx})"
                        );
                        drop(module);
                    }
                }
            }
            // Install new modules (already initialised by swap_plan).
            for (idx, module) in new_plan.new_modules.drain(..) {
                self.module_pool.install(idx, module);
            }
            // Apply parameter diffs to surviving modules.
            for (idx, params) in &new_plan.parameter_updates {
                self.module_pool.update_parameters(*idx, params);
            }
            // Apply connectivity updates to surviving modules.
            for (idx, conn) in new_plan.connectivity_updates.drain(..) {
                self.module_pool.set_connectivity(idx, conn);
            }
            // Zero freed/new cable buffer slots.
            for &i in &new_plan.to_zero {
                self.buffer_pool[i] = [0.0; 2];
            }
            self.current_plan = new_plan;
        }
    }

    /// Drain MIDI events for the current sub-block window and deliver them to
    /// all MIDI-receiving modules listed in the current plan.
    ///
    /// Events in `[sample_counter, sample_counter + SUB_BLOCK_SIZE)` are
    /// delivered; late events (target before `sample_counter`) are delivered
    /// with `offset = 0`; future events are left in the queue.
    fn dispatch_midi_events(&mut self) {
        if let Some(eq) = &mut self.event_queue {
            // DrainWindow borrows `eq` (i.e. self.event_queue).
            // self.current_plan and self.module_pool are separate fields —
            // Rust's NLL split-borrow analysis permits accessing them here.
            for (offset, event) in eq.drain_window(self.sample_counter, SUB_BLOCK_SIZE) {
                for i in 0..self.current_plan.midi_receiver_indices.len() {
                    let idx = self.current_plan.midi_receiver_indices[i];
                    self.module_pool.receive_midi(idx, offset, event);
                }
            }
        }
    }

    pub(crate) fn fill_buffer<T: cpal::SizedSample + cpal::FromSample<f32>>(
        &mut self,
        data: &mut [T],
        _info: &cpal::OutputCallbackInfo,
    ) {
        // Capture an Instant at callback entry as an approximation of the
        // playback wall-clock time for the first sample of this buffer.
        let playback_time = Instant::now();

        self.receive_plan();

        let frames = if self.channels > 0 { data.len() >> self.channel_shift } else { 0 };
        let mut remaining = frames;
        let mut out_i: usize = 0;

        while remaining > 0 {
            // Dispatch MIDI events at the start of each new sub-block.
            if self.samples_until_next_midi == SUB_BLOCK_SIZE as usize {
                self.dispatch_midi_events();
            }

            let chunk = self.samples_until_next_midi.min(remaining);

            self.process_chunk(data, &mut out_i, chunk);

            self.samples_until_next_midi -= chunk;
            remaining -= chunk;

            if self.samples_until_next_midi == 0 {
                self.sample_counter += SUB_BLOCK_SIZE;
                self.samples_until_next_midi = SUB_BLOCK_SIZE as usize;
            }
        }

        // Publish the clock anchor after the full buffer so that the MIDI
        // connector thread can map wall-clock timestamps to sample positions.
        //
        // SAFETY: `clock` points to the `AudioClock` owned by `SoundEngine`'s
        // `Arc`. The engine drops the stream (and thus this callback) before
        // releasing its `Arc`, so the pointer is valid here.
        unsafe { &*self.clock }.publish(self.sample_counter, playback_time);
    }
}

pub(crate) fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut callback: AudioCallback,
) -> Result<Stream, EngineError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                callback.fill_buffer(data, info);
            },
            |err| eprintln!("patches audio stream error: {err}"),
            None,
        )
        .map_err(EngineError::BuildStreamError)
}
