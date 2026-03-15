use std::time::Instant;

use super::clock::ClockAnchor;

/// Converts a `(ClockAnchor, event_wall_time)` pair into a target sample
/// position, adding a fixed lookahead to absorb OS scheduling jitter.
///
/// This is pure logic with no side effects.
pub struct EventScheduler {
    sample_rate: f32,
    lookahead_samples: u64,
}

impl EventScheduler {
    pub fn new(sample_rate: f32, lookahead_samples: u64) -> Self {
        Self {
            sample_rate,
            lookahead_samples,
        }
    }

    /// Compute the target sample position for an event.
    ///
    /// `target = anchor.sample_count + elapsed_samples + lookahead_samples`
    ///
    /// where `elapsed_samples` is the number of samples between
    /// `anchor.playback_wall_time` and `event_wall_time`, rounded to the
    /// nearest integer. If `event_wall_time` is before the anchor's reference
    /// point the result is clamped to `anchor.sample_count`.
    pub fn stamp(&self, anchor: &ClockAnchor, event_wall_time: Instant) -> u64 {
        let elapsed_secs = if event_wall_time >= anchor.playback_wall_time {
            (event_wall_time - anchor.playback_wall_time).as_secs_f32()
        } else {
            // Negative: event arrived before the anchor's reference point.
            -((anchor.playback_wall_time - event_wall_time).as_secs_f32())
        };

        let elapsed_samples = (elapsed_secs * self.sample_rate).round() as i64;
        let target = anchor.sample_count as i64 + elapsed_samples + self.lookahead_samples as i64;

        // Clamp: never return a position before the anchor.
        target.max(anchor.sample_count as i64) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn anchor_at(base: Instant, sample_count: u64) -> ClockAnchor {
        ClockAnchor {
            sample_count,
            playback_wall_time: base,
        }
    }

    #[test]
    fn event_at_anchor_time_returns_lookahead() {
        let base = Instant::now();
        let anchor = anchor_at(base, 1000);
        let scheduler = EventScheduler::new(48_000.0, 128);

        // event_wall_time == anchor.playback_wall_time → elapsed = 0
        let result = scheduler.stamp(&anchor, base);
        assert_eq!(result, 1000 + 128);
    }

    #[test]
    fn event_ahead_of_anchor() {
        let base = Instant::now();
        let anchor = anchor_at(base, 0);
        let scheduler = EventScheduler::new(48_000.0, 0);

        // 1 second ahead → 48_000 elapsed samples
        let event_time = base + Duration::from_secs(1);
        let result = scheduler.stamp(&anchor, event_time);
        assert_eq!(result, 48_000);
    }

    #[test]
    fn event_behind_anchor_is_clamped() {
        let base = Instant::now();
        let anchor = anchor_at(base, 5000);
        let scheduler = EventScheduler::new(48_000.0, 0);

        // event before anchor → clamped to anchor.sample_count
        let event_time = base - Duration::from_millis(100);
        let result = scheduler.stamp(&anchor, event_time);
        assert_eq!(result, 5000);
    }

    #[test]
    fn non_trivial_lookahead_offset() {
        let base = Instant::now();
        let anchor = anchor_at(base, 0);
        let scheduler = EventScheduler::new(48_000.0, 256);

        // 0.5 s ahead → 24_000 elapsed + 256 lookahead
        let event_time = base + Duration::from_millis(500);
        let result = scheduler.stamp(&anchor, event_time);
        assert_eq!(result, 24_000 + 256);
    }
}
