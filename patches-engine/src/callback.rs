use cpal::traits::DeviceTrait;
use cpal::{Stream, StreamConfig};

use patches_core::{ControlSignal, InstanceId, Module};

use crate::builder::ExecutionPlan;
use crate::engine::EngineError;
use crate::pool::ModulePool;

/// All state owned by the audio callback — plan, pools, channels, and
/// control-rate bookkeeping — gathered into one place so the callback
/// closure is a single `callback.fill_buffer(data)` call.
pub(crate) struct AudioCallback {
    plan_rx: rtrb::Consumer<ExecutionPlan>,
    signal_rx: rtrb::Consumer<(InstanceId, ControlSignal)>,
    current_plan: ExecutionPlan,
    buffer_pool: Box<[[f64; 2]]>,
    module_pool: ModulePool,
    channels: usize,
    /// `channels.trailing_zeros()` — the right-shift to convert a sample count to a frame count.
    channel_shift: u32,
    control_period: usize,
    samples_until_next_control: usize,
    wi_counter: usize,
    /// Producer end of the cleanup ring buffer. Tombstoned modules are pushed here
    /// so that deallocation happens on the cleanup thread, not the audio thread.
    /// Dropping the stream drops this producer, signalling the cleanup thread to exit.
    cleanup_tx: rtrb::Producer<Box<dyn Module>>,
}

impl AudioCallback {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        plan_rx: rtrb::Consumer<ExecutionPlan>,
        signal_rx: rtrb::Consumer<(InstanceId, ControlSignal)>,
        current_plan: ExecutionPlan,
        buffer_pool: Box<[[f64; 2]]>,
        module_pool: ModulePool,
        channels: usize,
        control_period: usize,
        cleanup_tx: rtrb::Producer<Box<dyn Module>>,
    ) -> Self {
        Self {
            plan_rx,
            signal_rx,
            current_plan,
            buffer_pool,
            module_pool,
            channels,
            channel_shift: channels.trailing_zeros(),
            control_period,
            samples_until_next_control: control_period,
            wi_counter: 0,
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

    /// Drain the signal ring buffer, delivering each signal to the appropriate module.
    fn dispatch_signals(&mut self) {
        while let Ok((id, signal)) = self.signal_rx.pop() {
            self.current_plan.dispatch_signal(id, signal, &mut self.module_pool);
        }
    }

    pub(crate) fn fill_buffer<T: cpal::SizedSample + cpal::FromSample<f32>>(
        &mut self,
        data: &mut [T],
    ) {
        self.receive_plan();

        let frames = if self.channels > 0 { data.len() >> self.channel_shift } else { 0 };
        let mut remaining = frames;
        let mut out_i: usize = 0;

        while remaining > 0 {
            let chunk = self.samples_until_next_control.min(remaining);

            self.process_chunk(data, &mut out_i, chunk);

            self.samples_until_next_control -= chunk;
            remaining -= chunk;

            if self.samples_until_next_control == 0 {
                self.dispatch_signals();
                self.samples_until_next_control = self.control_period;
            }
        }
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
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| callback.fill_buffer(data),
            |err| eprintln!("patches audio stream error: {err}"),
            None,
        )
        .map_err(EngineError::BuildStreamError)
}
