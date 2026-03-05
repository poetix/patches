use std::time::Duration;

use patches_core::Module;
use patches_engine::{ExecutionPlan, ModulePool};

/// Synchronous, device-free engine fixture that mirrors the audio callback's
/// plan-swap sequence. Useful for integration tests that do not need a real
/// audio device but do need to exercise plan adoption, module tombstoning,
/// and the cleanup-thread ring buffer.
///
/// `adopt_plan` replicates the callback plan-swap sequence:
///   1. Tombstone removed modules and push them to the cleanup ring buffer.
///   2. Install pre-initialised new modules.
///   3. Apply parameter diffs to surviving modules.
///   4. Zero cable buffer slots listed in `to_zero`.
///   5. Replace the current plan.
///
/// `stop` drops the cleanup producer (signalling the cleanup thread to exit)
/// and joins the thread, guaranteeing all tombstoned modules have been dropped
/// before returning.
pub struct HeadlessEngine {
    plan: ExecutionPlan,
    buffer_pool: Box<[[f64; 2]]>,
    module_pool: ModulePool,
    wi: usize,
    cleanup_tx: Option<rtrb::Producer<Box<dyn Module>>>,
    cleanup_thread: Option<std::thread::JoinHandle<()>>,
}

impl HeadlessEngine {
    /// Create a new `HeadlessEngine` with the given initial plan and pool capacities.
    ///
    /// Installs `plan.new_modules` into the module pool (modules are already
    /// initialised by the builder), zeros `plan.to_zero` slots, and spawns
    /// the `"patches-cleanup"` thread.
    ///
    /// # Panics
    ///
    /// Panics if the OS refuses to spawn the cleanup thread.
    pub fn new(mut plan: ExecutionPlan, buffer_capacity: usize, module_capacity: usize) -> Self {
        let mut buffer_pool = vec![[0.0_f64; 2]; buffer_capacity].into_boxed_slice();
        let mut module_pool = ModulePool::new(module_capacity);

        for (idx, m) in plan.new_modules.drain(..) {
            module_pool.install(idx, m);
        }
        for &i in &plan.to_zero {
            buffer_pool[i] = [0.0; 2];
        }

        let (cleanup_tx, mut cleanup_rx) =
            rtrb::RingBuffer::<Box<dyn Module>>::new(module_capacity);
        let cleanup_thread = std::thread::Builder::new()
            .name("patches-cleanup".to_owned())
            .spawn(move || loop {
                while cleanup_rx.pop().is_ok() {}
                if cleanup_rx.is_abandoned() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            })
            .expect("failed to spawn patches-cleanup thread");

        Self {
            plan,
            buffer_pool,
            module_pool,
            wi: 0,
            cleanup_tx: Some(cleanup_tx),
            cleanup_thread: Some(cleanup_thread),
        }
    }

    /// Adopt a new plan, mirroring the audio callback's plan-swap sequence.
    ///
    /// Tombstoned modules are pushed to the cleanup ring buffer. If the ring
    /// buffer is full, the module is dropped inline.
    pub fn adopt_plan(&mut self, mut plan: ExecutionPlan) {
        for &idx in &plan.tombstones {
            if let Some(module) = self.module_pool.tombstone(idx) {
                if let Some(tx) = &mut self.cleanup_tx {
                    if let Err(rtrb::PushError::Full(module)) = tx.push(module) {
                        drop(module);
                    }
                }
            }
        }
        for (idx, m) in plan.new_modules.drain(..) {
            self.module_pool.install(idx, m);
        }
        for (idx, params) in &plan.parameter_updates {
            self.module_pool.update_parameters(*idx, params);
        }
        for &i in &plan.to_zero {
            self.buffer_pool[i] = [0.0; 2];
        }
        self.plan = plan;
    }

    /// Advance the plan by one sample.
    pub fn tick(&mut self) {
        self.plan.tick(&mut self.module_pool, &mut self.buffer_pool, self.wi);
        self.wi = 1 - self.wi;
    }

    /// Left-channel output of the registered sink after the most recent tick.
    pub fn last_left(&self) -> f64 {
        self.module_pool.read_sink_left()
    }

    /// Right-channel output of the registered sink after the most recent tick.
    pub fn last_right(&self) -> f64 {
        self.module_pool.read_sink_right()
    }

    /// Inspect a cable buffer pool slot. Useful for verifying zeroing behaviour.
    pub fn pool_slot(&self, idx: usize) -> [f64; 2] {
        self.buffer_pool[idx]
    }

    /// Drop the cleanup producer and join the cleanup thread.
    ///
    /// After this call, all tombstoned modules are guaranteed to have been
    /// dropped on the `"patches-cleanup"` thread. Idempotent.
    pub fn stop(&mut self) {
        self.cleanup_tx.take();
        if let Some(handle) = self.cleanup_thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for HeadlessEngine {
    fn drop(&mut self) {
        self.stop();
    }
}
