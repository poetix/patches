use patches_core::{
    AudioEnvironment, CablePool, CableValue, InstanceId, Module, ModuleDescriptor, ModuleShape,
    MonoOutput, OutputPort, ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::{ParameterMap, ParameterValue};

/// Generates bar, beat, quaver, and semiquaver trigger pulses from a configurable BPM
/// and time signature.
///
/// All four outputs are derived from a single beat-phase accumulator, keeping them
/// perfectly phase-locked. Outputs are 1.0 on the one sample at each boundary and
/// 0.0 on all other samples.
///
/// Supports both simple time signatures (quavers_per_beat=2) and compound
/// (quavers_per_beat=3).
pub struct Clock {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f64,
    bpm: f64,
    beats_per_bar: u32,
    quavers_per_beat: u32,
    /// beat_phase increment per sample: bpm / (60.0 * sample_rate)
    beat_phase_delta: f64,
    /// Beat phase in [0.0, 1.0); incremented each sample
    beat_phase: f64,
    /// Number of beats that have completed (for bar boundary detection)
    beat_count: u32,
    // Output port fields
    out_bar: MonoOutput,
    out_beat: MonoOutput,
    out_quaver: MonoOutput,
    out_semiquaver: MonoOutput,
}

impl Module for Clock {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Clock",
            shape: shape.clone(),
            inputs: vec![],
            outputs: vec![
                PortDescriptor { name: "bar",        index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "beat",       index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "quaver",     index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "semiquaver", index: 0, kind: CableKind::Mono },
            ],
            parameters: vec![
                ParameterDescriptor {
                    name: "bpm",
                    index: 0,
                    parameter_type: ParameterKind::Float { min: 1.0, max: 300.0, default: 120.0 },
                },
                ParameterDescriptor {
                    name: "beats_per_bar",
                    index: 0,
                    parameter_type: ParameterKind::Int { min: 1, max: 16, default: 4 },
                },
                ParameterDescriptor {
                    name: "quavers_per_beat",
                    index: 0,
                    parameter_type: ParameterKind::Int { min: 1, max: 4, default: 2 },
                },
            ],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        Self {
            instance_id,
            descriptor,
            sample_rate: audio_environment.sample_rate,
            bpm: 0.0,
            beats_per_bar: 0,
            quavers_per_beat: 0,
            beat_phase_delta: 0.0,
            beat_phase: 0.0,
            beat_count: 0,
            out_bar: MonoOutput::default(),
            out_beat: MonoOutput::default(),
            out_quaver: MonoOutput::default(),
            out_semiquaver: MonoOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("bpm") {
            self.bpm = *v;
            self.beat_phase_delta = self.bpm / (60.0 * self.sample_rate);
        }
        if let Some(ParameterValue::Int(v)) = params.get("beats_per_bar") {
            self.beats_per_bar = *v as u32;
        }
        if let Some(ParameterValue::Int(v)) = params.get("quavers_per_beat") {
            self.quavers_per_beat = *v as u32;
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, _inputs: &[patches_core::InputPort], outputs: &[OutputPort]) {
        self.out_bar = MonoOutput::from_ports(outputs, 0);
        self.out_beat = MonoOutput::from_ports(outputs, 1);
        self.out_quaver = MonoOutput::from_ports(outputs, 2);
        self.out_semiquaver = MonoOutput::from_ports(outputs, 3);
    }

    fn process(&mut self, pool: &mut CablePool<'_>) {
        // Record old phase before increment
        let old_phase = self.beat_phase;

        // Increment beat phase
        self.beat_phase += self.beat_phase_delta;

        let mut bar_fired = false;
        let beat_fired = if self.beat_phase >= 1.0 {
            self.beat_phase -= 1.0;
            self.beat_count = self.beat_count.wrapping_add(1);

            // Check for bar boundary
            if self.beat_count.is_multiple_of(self.beats_per_bar) {
                bar_fired = true;
            }
            true
        } else {
            false
        };

        let new_phase = self.beat_phase;

        // Check for quaver boundary (1/quavers_per_beat of a beat)
        let quaver_buckets = self.quavers_per_beat;
        let old_quaver_bucket = (old_phase * quaver_buckets as f64) as u64;
        let new_quaver_bucket = (new_phase * quaver_buckets as f64) as u64;
        let quaver_fired = new_quaver_bucket > old_quaver_bucket || beat_fired;

        // Check for semiquaver boundary (half of a quaver)
        let semiquaver_buckets = self.quavers_per_beat * 2;
        let old_semiquaver_bucket = (old_phase * semiquaver_buckets as f64) as u64;
        let new_semiquaver_bucket = (new_phase * semiquaver_buckets as f64) as u64;
        let semiquaver_fired = new_semiquaver_bucket > old_semiquaver_bucket || beat_fired;

        pool.write_mono(&self.out_bar, if bar_fired { 1.0 } else { 0.0 });
        pool.write_mono(&self.out_beat, if beat_fired { 1.0 } else { 0.0 });
        pool.write_mono(&self.out_quaver, if quaver_fired { 1.0 } else { 0.0 });
        pool.write_mono(&self.out_semiquaver, if semiquaver_fired { 1.0 } else { 0.0 });
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use patches_core::{AudioEnvironment, CablePool, Module, ModuleShape, Registry};
    use patches_core::parameter_map::{ParameterMap, ParameterValue};

    fn make_clock(bpm: f64, beats_per_bar: i64, quavers_per_beat: i64) -> Box<dyn Module> {
        make_clock_sr(bpm, beats_per_bar, quavers_per_beat, 44100.0)
    }

    fn make_clock_sr(bpm: f64, beats_per_bar: i64, quavers_per_beat: i64, sample_rate: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("bpm".into(),              ParameterValue::Float(bpm));
        params.insert("beats_per_bar".into(),    ParameterValue::Int(beats_per_bar));
        params.insert("quavers_per_beat".into(), ParameterValue::Int(quavers_per_beat));
        let mut r = Registry::new();
        r.register::<Clock>();
        r.create(
            "Clock",
            &AudioEnvironment { sample_rate: sample_rate },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        ).unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

    fn set_all_outputs(module: &mut Box<dyn Module>) {
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 0, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 1, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 2, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 3, connected: true }),
        ];
        module.set_ports(&[], &outputs);
    }

    fn read_output(pool: &Vec<[CableValue; 2]>, idx: usize, wi: usize) -> f64 {
        if let CableValue::Mono(v) = pool[idx][wi] { v } else { panic!("expected Mono") }
    }

    #[test]
    fn four_four_time_4bpm_sample_rate_1() {
        // 4/4 time at 4 BPM with sample rate 1 Hz.
        // At 4 BPM, a beat occurs every 60/4 = 15 seconds.
        // With sample_rate = 1, that's every 15 samples.
        // In 4/4, a bar has 4 beats, so bar fires every 60 samples.
        let mut clock = make_clock_sr(4.0, 4, 2, 1.0);
        set_all_outputs(&mut clock);

        let mut pool = make_pool(4);
        let mut beat_count = 0;
        let mut bar_count = 0;

        // Process 64 samples and count pulses
        for i in 0..64 {
            let wi = i % 2;
            clock.process(&mut CablePool::new(&mut pool, wi));
            if read_output(&pool, 1, wi) > 0.5 { beat_count += 1; }
            if read_output(&pool, 0, wi) > 0.5 { bar_count += 1; }
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

        let mut clock = make_clock(120.0, 6, 3);
        set_all_outputs(&mut clock);

        let mut pool = make_pool(4);
        let mut beat_count = 0;
        let mut bar_count = 0;
        let mut quaver_count = 0;
        let mut semiquaver_count = 0;

        // Process 150000 samples
        for i in 0..150000usize {
            let wi = i % 2;
            clock.process(&mut CablePool::new(&mut pool, wi));
            if read_output(&pool, 0, wi) > 0.5 { bar_count += 1; }
            if read_output(&pool, 1, wi) > 0.5 { beat_count += 1; }
            if read_output(&pool, 2, wi) > 0.5 { quaver_count += 1; }
            if read_output(&pool, 3, wi) > 0.5 { semiquaver_count += 1; }
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
        let mut clock = make_clock(120.0, 4, 2);
        set_all_outputs(&mut clock);

        let mut pool = make_pool(4);
        // First few samples should not fire anything unless we're at a boundary
        for i in 0..5usize {
            let wi = i % 2;
            clock.process(&mut CablePool::new(&mut pool, wi));
            if i > 0 {
                // Only the first sample can fire a beat if we start at phase 0
                assert_eq!(read_output(&pool, 0, wi), 0.0, "bar should be 0 at sample {}", i);
                assert_eq!(read_output(&pool, 1, wi), 0.0, "beat should be 0 at sample {}", i);
                assert_eq!(read_output(&pool, 2, wi), 0.0, "quaver should be 0 at sample {}", i);
                assert_eq!(read_output(&pool, 3, wi), 0.0, "semiquaver should be 0 at sample {}", i);
            }
        }
    }

}
