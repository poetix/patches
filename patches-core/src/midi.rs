/// A raw 3-byte MIDI message.
///
/// The first byte is the status byte; the following two bytes are data bytes.
/// Messages shorter than 3 bytes have their unused data bytes set to zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiEvent {
    pub bytes: [u8; 3],
}

/// Opt-in trait for modules that receive MIDI events.
///
/// Modules that want MIDI delivery implement this trait and override
/// [`Module::as_midi_receiver`](crate::Module::as_midi_receiver) to return
/// `Some(self)`. Modules that do not implement this trait pay zero cost — the
/// planner ignores them and the pool never dispatches events to them.
pub trait ReceivesMidi {
    /// Deliver a MIDI event to this module.
    ///
    /// Called by the audio callback once per sub-block boundary for each event
    /// that falls within the current window. Implementations must not allocate,
    /// block, or perform I/O.
    fn receive_midi(&mut self, event: MidiEvent);
}
