use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use midir::{MidiInput, MidiInputConnection};

use super::clock::AudioClock;
use super::event_queue::{EventQueueProducer, MidiEvent};
use super::scheduler::EventScheduler;

/// Errors that can occur when opening a [`MidiConnector`].
#[derive(Debug)]
pub enum MidiError {
    /// Failed to initialise the MIDI subsystem.
    Init(midir::InitError),
}

impl std::fmt::Display for MidiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MidiError::Init(e) => write!(f, "MIDI init failed: {e}"),
        }
    }
}

impl std::error::Error for MidiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MidiError::Init(e) => Some(e),
        }
    }
}

/// State shared across all port callback closures via `Arc`.
struct ConnectorState {
    /// Protected by a mutex because midir may invoke each port's callback on
    /// a separate thread; the lock is held only for one non-blocking push.
    producer: Mutex<EventQueueProducer>,
    clock: Arc<AudioClock>,
    scheduler: EventScheduler,
    /// Count of events dropped because the [`EventQueue`] was full, or because
    /// the mutex was poisoned.  Incremented atomically; never blocks.
    dropped_count: AtomicU64,
}

/// Opens all available MIDI input ports and forwards incoming events into an
/// [`EventQueueProducer`] with sample-accurate target positions computed via
/// [`AudioClock`] and [`EventScheduler`].
///
/// Dropping the `MidiConnector` disconnects all ports and frees the
/// associated resources.
pub struct MidiConnector {
    // Kept alive so that the connections remain open; dropped on `close()` or
    // when `MidiConnector` is dropped.
    _connections: Vec<MidiInputConnection<Arc<ConnectorState>>>,
    state: Arc<ConnectorState>,
}

impl MidiConnector {
    /// Open all currently available MIDI input ports.
    ///
    /// On each port a callback is registered that:
    /// 1. Records `Instant::now()` as close to receipt as possible.
    /// 2. Calls [`EventScheduler::stamp`] to compute a target sample position.
    /// 3. Pushes the event into `queue`.
    ///
    /// If the queue is full the event is silently dropped and
    /// [`MidiConnector::dropped_count`] is incremented.
    pub fn open(
        clock: Arc<AudioClock>,
        queue: EventQueueProducer,
        scheduler: EventScheduler,
    ) -> Result<Self, MidiError> {
        let state = Arc::new(ConnectorState {
            producer: Mutex::new(queue),
            clock,
            scheduler,
            dropped_count: AtomicU64::new(0),
        });

        let mut connections: Vec<MidiInputConnection<Arc<ConnectorState>>> = Vec::new();

        // Each `MidiInput::connect` consumes its `MidiInput`, so we create a
        // fresh one per port.  We iterate by index; if the port list shrinks
        // between iterations we simply stop.
        let mut port_idx: usize = 0;
        loop {
            let midi_in = MidiInput::new(&format!("patches-midi-{port_idx}"))
                .map_err(MidiError::Init)?;
            let ports = midi_in.ports();
            if port_idx >= ports.len() {
                break;
            }
            let port = &ports[port_idx];
            let port_name = midi_in
                .port_name(port)
                .unwrap_or_else(|_| format!("port-{port_idx}"));

            let state_clone = Arc::clone(&state);
            match midi_in.connect(
                port,
                "patches-input",
                move |_midir_ts, message, data| {
                    // Timestamp as close to receipt as possible.
                    let now = Instant::now();
                    handle_midi_message(now, message, data);
                },
                state_clone,
            ) {
                Ok(conn) => {
                    connections.push(conn);
                }
                Err(e) => {
                    eprintln!("patches-midi: could not connect to '{port_name}': {e}");
                }
            }

            port_idx += 1;
        }

        if connections.is_empty() {
            eprintln!("patches-midi: no MIDI input ports available or all connections failed");
        }

        Ok(Self {
            _connections: connections,
            state,
        })
    }

    /// Number of events dropped due to a full queue (or a poisoned mutex)
    /// since this connector was opened.
    ///
    /// This counter is updated atomically and never blocks.
    pub fn dropped_count(&self) -> u64 {
        self.state.dropped_count.load(Ordering::Relaxed)
    }

    /// Disconnect all ports and free resources.
    ///
    /// Equivalent to dropping the `MidiConnector`.
    pub fn close(self) {
        // Dropping `self` disconnects all connections via their `Drop` impls.
    }
}

/// Called from within a midir callback thread for each incoming message.
fn handle_midi_message(now: Instant, message: &[u8], state: &mut Arc<ConnectorState>) {
    if message.is_empty() {
        return;
    }

    // SysEx and other variable-length messages are not supported; ignore them.
    if message[0] == 0xF0 {
        return;
    }

    let bytes = match message.len() {
        1 => [message[0], 0, 0],
        2 => [message[0], message[1], 0],
        _ => [message[0], message[1], message[2]],
    };

    let anchor = state.clock.read();
    let target_sample = state.scheduler.stamp(&anchor, now);
    let event = MidiEvent { bytes };

    // The mutex is held only for one non-blocking ring-buffer push.
    let push_result = state
        .producer
        .lock()
        .map(|mut p| p.push(target_sample, event));

    match push_result {
        Ok(Ok(())) => {}
        Ok(Err(_)) | Err(_) => {
            // Queue full or mutex poisoned — count the drop without allocating
            // or blocking.
            state.dropped_count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::{new_event_queue, AudioClock, EventScheduler};
    use std::sync::Arc;

    /// Smoke test: open and immediately close the connector.
    ///
    /// No MIDI hardware is required; the test passes even when no ports are
    /// available (the connector opens successfully with zero connections).
    #[test]
    fn open_and_close_without_panic() {
        let clock = Arc::new(AudioClock::new());
        let (producer, _consumer) = new_event_queue(64);
        let scheduler = EventScheduler::new(48_000.0, 128);

        match MidiConnector::open(clock, producer, scheduler) {
            Ok(connector) => {
                assert_eq!(connector.dropped_count(), 0);
                connector.close();
            }
            Err(e) => {
                // Acceptable in environments without MIDI subsystem support.
                eprintln!("MIDI init not available (expected in some CI): {e}");
            }
        }
    }
}
