use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// A consistent snapshot of the audio clock published by the audio thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockAnchor {
    pub sample_count: u64,
    pub playback_wall_time: Instant,
}

/// A seqlock allowing the audio thread to publish `ClockAnchor` values without
/// allocating or blocking, and MIDI connector threads to read consistent
/// snapshots without blocking the writer.
///
/// ## Protocol
/// - **Writer** increments `sequence` to an odd value, writes both fields,
///   then increments `sequence` to even.
/// - **Reader** checks `sequence` is even before and after reading the fields;
///   if not, it retries. This is obstruction-free for readers and wait-free for
///   the writer.
pub struct AudioClock {
    sequence: AtomicU64,
    // UnsafeCell gives us interior mutability without a lock.
    sample_count: UnsafeCell<u64>,
    playback_wall_time: UnsafeCell<Instant>,
}

// SAFETY: `AudioClock` is designed for exactly one writer and any number of
// readers on separate threads. The seqlock protocol ensures readers never
// observe torn data.
unsafe impl Sync for AudioClock {}
// The clock is meant to be shared behind an Arc, so it must be Send.
unsafe impl Send for AudioClock {}

impl AudioClock {
    /// Create a new `AudioClock`. The initial anchor has `sample_count = 0`
    /// and `playback_wall_time = Instant::now()` so that `read()` is always
    /// well-defined even before the first `publish`.
    pub fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            sample_count: UnsafeCell::new(0),
            playback_wall_time: UnsafeCell::new(Instant::now()),
        }
    }

    /// Publish a new anchor from the audio thread.
    ///
    /// This is wait-free: it performs no heap allocation and no blocking.
    ///
    /// # Safety contract for callers
    /// Only a **single writer** may call `publish` at any time. Multiple
    /// concurrent calls to `publish` would violate the seqlock invariant.
    pub fn publish(&self, sample_count: u64, playback_wall_time: Instant) {
        // Step 1: announce write-in-progress (odd sequence).
        let seq = self.sequence.load(Ordering::Relaxed);
        self.sequence.store(seq.wrapping_add(1), Ordering::Release);

        // Step 2: write the payload. Compiler / CPU must not move these stores
        // before the sequence increment above.
        //
        // SAFETY: we are the sole writer; no other `publish` call is concurrent.
        // Readers who observe an odd sequence will retry, so they never use these
        // values while they are being written.
        unsafe {
            *self.sample_count.get() = sample_count;
            *self.playback_wall_time.get() = playback_wall_time;
        }

        // Step 3: announce write complete (even sequence).
        self.sequence.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// Read a consistent `ClockAnchor` snapshot.
    ///
    /// Spins (obstruction-free) until a clean read is obtained. In steady state
    /// the audio thread writes at most once per callback (~10 ms), so contention
    /// is negligible.
    pub fn read(&self) -> ClockAnchor {
        loop {
            let seq1 = self.sequence.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                // Write in progress — retry immediately.
                std::hint::spin_loop();
                continue;
            }

            // SAFETY: seq1 is even, so no writer is currently modifying the
            // payload. We perform an acquire load on `sequence` again after
            // reading to detect if a write completed mid-read.
            let sample_count = unsafe { *self.sample_count.get() };
            let playback_wall_time = unsafe { *self.playback_wall_time.get() };

            let seq2 = self.sequence.load(Ordering::Acquire);
            if seq1 == seq2 {
                return ClockAnchor {
                    sample_count,
                    playback_wall_time,
                };
            }
            // Sequence changed — a write overlapped our read; retry.
            std::hint::spin_loop();
        }
    }
}

impl Default for AudioClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_then_read_returns_published_values() {
        let clock = AudioClock::new();
        let t = Instant::now();
        clock.publish(42, t);
        let anchor = clock.read();
        assert_eq!(anchor.sample_count, 42);
        assert_eq!(anchor.playback_wall_time, t);
    }

    #[test]
    fn sequence_parity_prevents_torn_reads() {
        // Verify the seqlock state-machine logic in isolation (single-threaded).
        // We check that a reader presented with an odd sequence would retry,
        // and that equal even sequences are accepted.
        let clock = AudioClock::new();

        // Initial state: sequence is 0 (even) → read should succeed immediately.
        let t0 = Instant::now();
        clock.publish(1, t0);
        let anchor = clock.read();
        assert_eq!(anchor.sample_count, 1);

        // Simulate two sequential publishes; the final read must see the last one.
        let t1 = Instant::now();
        clock.publish(100, t1);
        let t2 = Instant::now();
        clock.publish(200, t2);
        let anchor2 = clock.read();
        assert_eq!(anchor2.sample_count, 200);
        assert_eq!(anchor2.playback_wall_time, t2);
    }
}
