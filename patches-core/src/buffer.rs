/// A single-cable 2-element ring buffer.
///
/// Each connection between modules is represented by one `SampleBuffer`. During
/// each engine tick:
/// 1. The writing module calls [`write`](SampleBuffer::write) with its output value.
/// 2. The reading module calls [`read`](SampleBuffer::read) to obtain the value
///    from the *previous* tick (1-sample cable delay).
/// 3. After all modules have processed, the engine calls [`advance`](SampleBuffer::advance)
///    on every buffer to rotate the write slot.
///
/// The 1-sample delay makes feedback cycles in the module graph safe regardless
/// of execution order.
pub struct SampleBuffer {
    data: [f64; 2],
    write_index: usize,
}

impl SampleBuffer {
    pub fn new() -> Self {
        Self {
            data: [0.0; 2],
            write_index: 0,
        }
    }

    /// Store `value` into the current write slot (this tick).
    pub fn write(&mut self, value: f64) {
        self.data[self.write_index] = value;
    }

    /// Return the value written in the *previous* tick.
    pub fn read(&self) -> f64 {
        self.data[1 - self.write_index]
    }

    /// Rotate the write slot. Called by the engine after all modules have processed.
    pub fn advance(&mut self) {
        self.write_index = 1 - self.write_index;
    }
}

impl Default for SampleBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_read_is_zero() {
        let buf = SampleBuffer::new();
        assert_eq!(buf.read(), 0.0);
    }

    #[test]
    fn write_then_read_returns_previous_tick_value() {
        let mut buf = SampleBuffer::new();

        // Tick 1: write 1.0; read still returns the initial 0.0.
        buf.write(1.0);
        assert_eq!(buf.read(), 0.0);
        buf.advance();

        // Tick 2: write 2.0; read returns 1.0 (written last tick).
        buf.write(2.0);
        assert_eq!(buf.read(), 1.0);
        buf.advance();

        // Tick 3: read returns 2.0.
        assert_eq!(buf.read(), 2.0);
    }

    #[test]
    fn advance_toggles_write_slot() {
        let mut buf = SampleBuffer::new();

        buf.write(10.0);
        buf.advance();
        buf.write(20.0);
        buf.advance();

        // After two advances the slot has toggled twice (back to original),
        // so read returns what was written in the immediately preceding tick: 20.0.
        assert_eq!(buf.read(), 20.0);
    }
}
