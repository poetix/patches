pub use patches_core::MidiEvent;
use rtrb::{Consumer, Producer, RingBuffer};

/// Item stored in the ring buffer: a target sample position paired with the
/// event to deliver at that position.
#[derive(Debug, Clone, Copy)]
struct TimedEvent {
    target_sample: u64,
    event: MidiEvent,
}

/// Returned by [`EventQueueProducer::push`] when the ring buffer is full.
#[derive(Debug)]
pub struct QueueFull;

/// Producer half of the [`EventQueue`].  Lives on the MIDI connector thread.
pub struct EventQueueProducer {
    inner: Producer<TimedEvent>,
}

impl EventQueueProducer {
    /// Push an event without blocking or allocating.
    ///
    /// Returns `Err(QueueFull)` if the ring buffer has no room.
    pub fn push(&mut self, target_sample: u64, event: MidiEvent) -> Result<(), QueueFull> {
        self.inner.push(TimedEvent { target_sample, event }).map_err(|_| QueueFull)
    }
}

/// Consumer half of the [`EventQueue`].  Lives on the audio thread.
pub struct EventQueueConsumer {
    inner: Consumer<TimedEvent>,
    /// One-slot look-ahead: holds an event that was peeked but belongs to a
    /// future window.
    peeked: Option<TimedEvent>,
}

impl EventQueueConsumer {
    /// Returns an iterator of `(offset, MidiEvent)` for all events that fall
    /// within `[window_start, window_start + sub_block_size)`.
    ///
    /// - Late events (`target_sample < window_start`) are yielded with
    ///   `offset = 0`.
    /// - Future events remain in the buffer and are **not** yielded.
    pub fn drain_window(
        &mut self,
        window_start: u64,
        sub_block_size: u64,
    ) -> DrainWindow<'_> {
        DrainWindow {
            consumer: self,
            window_start,
            window_end: window_start.saturating_add(sub_block_size),
            sub_block_size,
            done: false,
        }
    }
}

/// Iterator returned by [`EventQueueConsumer::drain_window`].
pub struct DrainWindow<'a> {
    consumer: &'a mut EventQueueConsumer,
    window_start: u64,
    window_end: u64,
    sub_block_size: u64,
    done: bool,
}

impl Iterator for DrainWindow<'_> {
    type Item = (usize, MidiEvent);

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        // Take from the look-ahead slot first, then from the ring buffer.
        let timed = if let Some(ev) = self.consumer.peeked.take() {
            ev
        } else {
            match self.consumer.inner.pop() {
                Ok(ev) => ev,
                Err(_) => return None, // queue empty
            }
        };

        if timed.target_sample >= self.window_end {
            // Future event — put it back in the look-ahead slot and stop.
            self.consumer.peeked = Some(timed);
            self.done = true;
            return None;
        }

        // Late or in-window: compute clamped offset.
        let offset = if timed.target_sample < self.window_start {
            0
        } else {
            let raw = (timed.target_sample - self.window_start) as usize;
            raw.min(self.sub_block_size.saturating_sub(1) as usize)
        };

        Some((offset, timed.event))
    }
}

/// Create a new `EventQueue` with the given ring-buffer capacity.
pub fn new(capacity: usize) -> (EventQueueProducer, EventQueueConsumer) {
    let (producer, consumer) = RingBuffer::new(capacity);
    (
        EventQueueProducer { inner: producer },
        EventQueueConsumer {
            inner: consumer,
            peeked: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(b: u8) -> MidiEvent {
        MidiEvent { bytes: [b, 0, 0] }
    }

    #[test]
    fn empty_queue_returns_no_items() {
        let (_tx, mut rx) = new(16);
        let items: Vec<_> = rx.drain_window(0, 64).collect();
        assert!(items.is_empty());
    }

    #[test]
    fn in_window_events_have_correct_offset() {
        let (mut tx, mut rx) = new(16);
        // window [100, 164)
        tx.push(100, event(1)).unwrap();
        tx.push(110, event(2)).unwrap();
        tx.push(163, event(3)).unwrap();

        let items: Vec<_> = rx.drain_window(100, 64).collect();
        assert_eq!(items, vec![(0, event(1)), (10, event(2)), (63, event(3))]);
    }

    #[test]
    fn late_events_yield_offset_zero() {
        let (mut tx, mut rx) = new(16);
        tx.push(50, event(9)).unwrap(); // before window [100, 164)

        let items: Vec<_> = rx.drain_window(100, 64).collect();
        assert_eq!(items, vec![(0, event(9))]);
    }

    #[test]
    fn future_events_not_consumed() {
        let (mut tx, mut rx) = new(16);
        // window [0, 64)
        tx.push(64, event(7)).unwrap(); // exactly at window end → future
        tx.push(100, event(8)).unwrap();

        let first: Vec<_> = rx.drain_window(0, 64).collect();
        assert!(first.is_empty(), "no events should be in [0, 64)");

        // Both should appear in a later window.
        let second: Vec<_> = rx.drain_window(64, 64).collect();
        assert_eq!(second, vec![(0, event(7)), (36, event(8))]);
    }

    #[test]
    fn mix_of_past_current_and_future() {
        let (mut tx, mut rx) = new(16);
        // window [200, 264)
        tx.push(150, event(1)).unwrap(); // late → offset 0
        tx.push(200, event(2)).unwrap(); // at start → offset 0
        tx.push(230, event(3)).unwrap(); // in window → offset 30
        tx.push(264, event(4)).unwrap(); // future → stays

        let items: Vec<_> = rx.drain_window(200, 64).collect();
        assert_eq!(
            items,
            vec![(0, event(1)), (0, event(2)), (30, event(3))]
        );

        // Future event still available in next window.
        let next: Vec<_> = rx.drain_window(264, 64).collect();
        assert_eq!(next, vec![(0, event(4))]);
    }
}
