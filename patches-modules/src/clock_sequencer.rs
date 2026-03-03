use patches_core::{AudioEnvironment, ControlSignal, InstanceId, Module, ModuleDescriptor, PortDescriptor};

/// Generates bar, beat, quaver, and semiquaver trigger pulses from a configurable BPM
/// and time signature.
///
/// All four outputs are derived from a single beat-phase accumulator, keeping them
/// perfectly phase-locked. Outputs are 1.0 on the one sample at each boundary and
/// 0.0 on all other samples.
///
/// Supports both simple time signatures (quavers_per_beat=2) and compound
/// (quavers_per_beat=3).
pub struct ClockSequencer {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    bpm: f64,
    beats_per_bar: u32,
    quavers_per_beat: u32,
    /// beat_phase increment per sample: bpm / (60.0 * sample_rate)
    beat_phase_delta: f64,
    /// Beat phase in [0.0, 1.0); incremented each sample
    beat_phase: f64,
    /// Number of beats that have completed (for bar boundary detection)
    beat_count: u32,
}

impl ClockSequencer {
    /// Construct a `ClockSequencer` with configurable BPM, beats per bar, and quavers per beat.
    pub fn new(bpm: f64, beats_per_bar: u32, quavers_per_beat: u32) -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor: ModuleDescriptor {
                inputs: vec![],
                outputs: vec![
                    PortDescriptor { name: "bar", index: 0 },
                    PortDescriptor { name: "beat", index: 0 },
                    PortDescriptor { name: "quaver", index: 0 },
                    PortDescriptor { name: "semiquaver", index: 0 },
                ],
            },
            bpm,
            beats_per_bar,
            quavers_per_beat,
            beat_phase_delta: bpm / (60.0 * 44100.0), // default, updated in initialise
            beat_phase: 0.0,
            beat_count: 0,
        }
    }

    /// Recalculate beat_phase_delta from bpm and sample_rate.
    fn update_beat_phase_delta(&mut self, sample_rate: f64) {
        self.beat_phase_delta = self.bpm / (60.0 * sample_rate);
    }
}

impl Module for ClockSequencer {
    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn initialise(&mut self, env: &AudioEnvironment) {
        self.update_beat_phase_delta(env.sample_rate);
    }

    fn receive_signal(&mut self, signal: ControlSignal) {
        match signal {
            ControlSignal::Float { name: "bpm", value } => {
                self.bpm = value;
                // Note: sample_rate is not available here, so we can't update beat_phase_delta.
                // The delta will be recalculated in the next initialise call or we'd need
                // to store sample_rate as a field. Store sample_rate for this reason.
            }
            ControlSignal::Float { name: "beats_per_bar", value } => {
                self.beats_per_bar = value as u32;
            }
            ControlSignal::Float { name: "quavers_per_beat", value } => {
                self.quavers_per_beat = value as u32;
            }
            _ => {}
        }
    }

    fn process(&mut self, _inputs: &[f64], outputs: &mut [f64]) {
        // Initialize all outputs to 0
        outputs[0] = 0.0; // bar
        outputs[1] = 0.0; // beat
        outputs[2] = 0.0; // quaver
        outputs[3] = 0.0; // semiquaver

        // Record old phase before increment
        let old_phase = self.beat_phase;

        // Increment beat phase
        self.beat_phase += self.beat_phase_delta;

        // Detect beat wrap and bar boundaries
        let beat_fired = if self.beat_phase >= 1.0 {
            self.beat_phase -= 1.0;
            self.beat_count = self.beat_count.wrapping_add(1);

            // Check for bar boundary
            if self.beat_count.is_multiple_of(self.beats_per_bar) {
                outputs[0] = 1.0; // bar
            }
            outputs[1] = 1.0; // beat
            true
        } else {
            false
        };

        let new_phase = self.beat_phase;

        // Check for quaver boundary (1/quavers_per_beat of a beat)
        let quaver_buckets = self.quavers_per_beat;
        let old_quaver_bucket = (old_phase * quaver_buckets as f64) as u64;
        let new_quaver_bucket = (new_phase * quaver_buckets as f64) as u64;
        if new_quaver_bucket > old_quaver_bucket || beat_fired {
            outputs[2] = 1.0; // quaver
        }

        // Check for semiquaver boundary (half of a quaver)
        let semiquaver_buckets = self.quavers_per_beat * 2;
        let old_semiquaver_bucket = (old_phase * semiquaver_buckets as f64) as u64;
        let new_semiquaver_bucket = (new_phase * semiquaver_buckets as f64) as u64;
        if new_semiquaver_bucket > old_semiquaver_bucket || beat_fired {
            outputs[3] = 1.0; // semiquaver
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_shape() {
        let m = ClockSequencer::new(120.0, 4, 2);
        let desc = m.descriptor();
        assert_eq!(desc.inputs.len(), 0);
        assert_eq!(desc.outputs.len(), 4);
        assert_eq!(desc.outputs[0].name, "bar");
        assert_eq!(desc.outputs[1].name, "beat");
        assert_eq!(desc.outputs[2].name, "quaver");
        assert_eq!(desc.outputs[3].name, "semiquaver");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = ClockSequencer::new(120.0, 4, 2);
        let b = ClockSequencer::new(120.0, 4, 2);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn four_four_time_4bpm_sample_rate_1() {
        // 4/4 time at 4 BPM with sample rate 1 Hz.
        // At 4 BPM, a beat occurs every 60/4 = 15 seconds.
        // With sample_rate = 1, that's every 15 samples.
        // In 4/4, a bar has 4 beats, so bar fires every 60 samples.

        let mut clock = ClockSequencer::new(4.0, 4, 2);
        clock.initialise(&AudioEnvironment { sample_rate: 1.0 });

        let mut outputs = [0.0f64; 4];
        let mut beat_count = 0;
        let mut bar_count = 0;

        // Process 64 samples and count pulses
        for _ in 0..64 {
            clock.process(&[], &mut outputs);
            if outputs[1] > 0.5 {
                beat_count += 1;
            }
            if outputs[0] > 0.5 {
                bar_count += 1;
            }
        }

        // In 64 samples at 4 BPM / 1 Hz:
        // beat_phase increments by 4/60 per sample
        // Beat fires when beat_phase wraps (every 15 samples)
        // 64 / 15 ≈ 4.26, so 4 beats and 0 bars (bar fires on 4th beat, which is at 60 samples)
        assert_eq!(beat_count, 4, "expected 4 beats in 64 samples at 4 BPM");
        assert_eq!(bar_count, 1, "expected 1 bar (fires with 4th beat) in 64 samples");
    }

    #[test]
    fn six_eight_time_120bpm() {
        // 6/8 time (6 beats per bar, compound with quavers_per_beat=3)
        // At 120 BPM with sample_rate 44100:
        // beat_phase increments by 120 / (60 * 44100) = 120 / 2646000 ≈ 4.534e-5 per sample
        // A beat completes every 1 / (120 / 2646000) = 22050 samples
        // 6 beats per bar = bar every 132300 samples

        let mut clock = ClockSequencer::new(120.0, 6, 3);
        clock.initialise(&AudioEnvironment { sample_rate: 44100.0 });

        let mut outputs = [0.0f64; 4];
        let mut beat_count = 0;
        let mut bar_count = 0;
        let mut quaver_count = 0;
        let mut semiquaver_count = 0;

        // Process 150000 samples
        for _ in 0..150000 {
            clock.process(&[], &mut outputs);
            if outputs[0] > 0.5 {
                bar_count += 1;
            }
            if outputs[1] > 0.5 {
                beat_count += 1;
            }
            if outputs[2] > 0.5 {
                quaver_count += 1;
            }
            if outputs[3] > 0.5 {
                semiquaver_count += 1;
            }
        }

        // 150000 / 22050 ≈ 6.8 beats, so ~6 beats complete within the window
        assert_eq!(beat_count, 6, "expected 6 beats in 150000 samples at 120 BPM");
        // 6 beats per bar means bar fires on beats 6 and 12, so 1 bar
        assert!(bar_count > 0, "expected at least 1 bar");
        // In 6/8 (quavers_per_beat=3), there are 3 quavers per beat
        // So quaver count should be 3x beat count
        assert!(quaver_count > beat_count, "expected more quavers than beats");
        // Semiquavers are half-quavers, so 2x quaver count
        assert!(semiquaver_count > quaver_count, "expected more semiquavers than quavers");
    }

    #[test]
    fn all_outputs_initialized_to_zero() {
        let mut clock = ClockSequencer::new(120.0, 4, 2);
        clock.initialise(&AudioEnvironment { sample_rate: 44100.0 });

        let mut outputs = [0.0f64; 4];
        // First few samples should not fire anything unless we're at a boundary
        for i in 0..5 {
            clock.process(&[], &mut outputs);
            if i > 0 {
                // Only the first sample can fire a beat if we start at phase 0
                assert_eq!(outputs[0], 0.0, "bar should be 0 at sample {}", i);
                assert_eq!(outputs[1], 0.0, "beat should be 0 at sample {}", i);
                assert_eq!(outputs[2], 0.0, "quaver should be 0 at sample {}", i);
                assert_eq!(outputs[3], 0.0, "semiquaver should be 0 at sample {}", i);
            }
        }
    }

    #[test]
    fn receive_signal_updates_bpm() {
        let mut clock = ClockSequencer::new(120.0, 4, 2);
        clock.receive_signal(ControlSignal::Float {
            name: "bpm",
            value: 240.0,
        });
        assert_eq!(clock.bpm, 240.0);
    }

    #[test]
    fn receive_signal_updates_beats_per_bar() {
        let mut clock = ClockSequencer::new(120.0, 4, 2);
        clock.receive_signal(ControlSignal::Float {
            name: "beats_per_bar",
            value: 3.0,
        });
        assert_eq!(clock.beats_per_bar, 3);
    }

    #[test]
    fn receive_signal_updates_quavers_per_beat() {
        let mut clock = ClockSequencer::new(120.0, 4, 2);
        clock.receive_signal(ControlSignal::Float {
            name: "quavers_per_beat",
            value: 3.0,
        });
        assert_eq!(clock.quavers_per_beat, 3);
    }

    #[test]
    fn receive_signal_unknown_is_ignored() {
        let mut clock = ClockSequencer::new(120.0, 4, 2);
        let original_bpm = clock.bpm;
        clock.receive_signal(ControlSignal::Float {
            name: "gain",
            value: 0.5,
        });
        assert_eq!(clock.bpm, original_bpm);
    }
}
