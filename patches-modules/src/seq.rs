use patches_core::{
    AudioEnvironment, CableValue, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, ModuleShape, OutputPort, ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::build_error::BuildError;
use patches_core::parameter_map::{ParameterMap, ParameterValue};

/// A pre-parsed step in the sequencer pattern.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    /// A named note with a V/OCT pitch value (relative to C0=0.0).
    Note { voct: f64 },
    /// A rest: gate=0, trigger=0; pitch holds previous value.
    Rest,
    /// A tie: gate=1, trigger=0; pitch holds the current tied note's value.
    Tie,
}

/// Error returned when a step string cannot be parsed.
#[derive(Debug, PartialEq)]
struct ParseStepError {
    step: String,
}

impl std::fmt::Display for ParseStepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unrecognised step string: {:?}", self.step)
    }
}

impl std::error::Error for ParseStepError {}

/// Parse a step string into a `Step`.
fn parse_step(s: &str) -> Result<Step, ParseStepError> {
    match s {
        "-" => return Ok(Step::Rest),
        "_" => return Ok(Step::Tie),
        _ => {}
    }

    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Err(ParseStepError { step: s.to_string() });
    }

    // Letter
    let letter = bytes[0] as char;
    let semitone_base: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(ParseStepError { step: s.to_string() }),
    };

    let mut pos = 1;

    // Optional accidental
    let accidental: i32 = if pos < bytes.len() {
        match bytes[pos] as char {
            '#' => { pos += 1; 1 }
            'b' => { pos += 1; -1 }
            _ => 0,
        }
    } else {
        0
    };

    // Octave digit
    if pos >= bytes.len() {
        return Err(ParseStepError { step: s.to_string() });
    }
    let octave_char = bytes[pos] as char;
    let octave: i32 = octave_char
        .to_digit(10)
        .map(|d| d as i32)
        .ok_or_else(|| ParseStepError { step: s.to_string() })?;
    pos += 1;

    // No trailing characters allowed
    if pos != bytes.len() {
        return Err(ParseStepError { step: s.to_string() });
    }

    let semitone_index = semitone_base + accidental;
    let voct = octave as f64 + semitone_index as f64 / 12.0;
    Ok(Step::Note { voct })
}

/// A step sequencer that advances one step per rising edge on the `clock` input.
///
/// Input ports (all at index 0 since each name is unique):
///   inputs[0] — clock
///   inputs[1] — start
///   inputs[2] — stop
///   inputs[3] — reset
///
/// Output ports:
///   outputs[0] — pitch  (V/OCT, C0=0.0)
///   outputs[1] — trigger (1.0 on the clock-advance sample, 0.0 otherwise)
///   outputs[2] — gate    (1.0 while a note or tie is active)
pub struct Seq {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    steps: Vec<Step>,
    step_index: usize,
    playing: bool,
    /// Pitch value held until next note step.
    current_pitch: f64,
    /// Whether to emit trigger=1 on this sample.
    trigger_pending: bool,
    /// Previous sample values for rising-edge detection.
    prev_clock: f64,
    prev_start: f64,
    prev_stop: f64,
    prev_reset: f64,
    // Port fields
    in_clock: MonoInput,
    in_start: MonoInput,
    in_stop: MonoInput,
    in_reset: MonoInput,
    out_pitch: MonoOutput,
    out_trigger: MonoOutput,
    out_gate: MonoOutput,
}

impl Seq {
    /// Apply the step at `self.step_index` to the internal pitch/trigger state.
    fn apply_current_step(&mut self) {
        match &self.steps[self.step_index] {
            Step::Note { voct } => {
                self.current_pitch = *voct;
                self.trigger_pending = true;
            }
            Step::Rest | Step::Tie => {
                // pitch holds; trigger stays false
            }
        }
    }
}

impl Module for Seq {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "Seq",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "clock", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "start", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "stop",  index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "reset", index: 0, kind: CableKind::Mono },
            ],
            outputs: vec![
                PortDescriptor { name: "pitch",   index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "trigger", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "gate",    index: 0, kind: CableKind::Mono },
            ],
            parameters: vec![
                ParameterDescriptor {
                    name: "steps",
                    index: 0,
                    parameter_type: ParameterKind::Array { default: &[], length: shape.length },
                },
            ],
            is_sink: false,
        }
    }

    fn prepare(_audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        let capacity = descriptor.shape.length;
        Self {
            instance_id,
            descriptor,
            steps: Vec::with_capacity(capacity),
            step_index: 0,
            playing: true,
            current_pitch: 0.0,
            trigger_pending: false,
            prev_clock: 0.0,
            prev_start: 0.0,
            prev_stop: 0.0,
            prev_reset: 0.0,
            in_clock: MonoInput::default(),
            in_start: MonoInput::default(),
            in_stop: MonoInput::default(),
            in_reset: MonoInput::default(),
            out_pitch: MonoOutput::default(),
            out_trigger: MonoOutput::default(),
            out_gate: MonoOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Array(step_strs)) = params.get("steps") {
            // Steps have already been validated by update_parameters; parse is infallible here.
            let parsed: Vec<Step> = step_strs
                .iter()
                .filter_map(|s| parse_step(s).ok())
                .collect();
            self.steps = parsed;
            // Do not reset step_index: preserve position so that hot-reloading a pattern
            // during playback does not cause an audible jump to step 0.
            // process() uses steps.get(step_index), which returns None (treated as rest)
            // for any out-of-range index until the next clock edge wraps it back in bounds.
        }
    }

    fn update_parameters(&mut self, params: &ParameterMap) -> Result<(), BuildError> {
        patches_core::validate_parameters(params, self.descriptor())?;
        // Validate step patterns before applying — array content is not checked by
        // validate_parameters, so we do it here in the fallible layer.
        if let Some(ParameterValue::Array(step_strs)) = params.get("steps") {
            let _: Vec<Step> = step_strs
                .iter()
                .map(|s| parse_step(s))
                .collect::<Result<Vec<Step>, ParseStepError>>()
                .map_err(|e| BuildError::Custom {
                    module: "StepSequencer",
                    message: format!("invalid step pattern: {e}"),
                })?;
        }
        self.update_validated_parameters(params);
        Ok(())
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_clock = MonoInput::from_ports(inputs, 0);
        self.in_start = MonoInput::from_ports(inputs, 1);
        self.in_stop = MonoInput::from_ports(inputs, 2);
        self.in_reset = MonoInput::from_ports(inputs, 3);
        self.out_pitch = MonoOutput::from_ports(outputs, 0);
        self.out_trigger = MonoOutput::from_ports(outputs, 1);
        self.out_gate = MonoOutput::from_ports(outputs, 2);
    }

    fn process(&mut self, pool: &mut [[CableValue; 2]], wi: usize) {
        let ri = 1 - wi;

        // Guard: when the pattern is empty all outputs hold at rest values.
        if self.steps.is_empty() {
            self.out_pitch.write_to(pool, wi, 0.0);
            self.out_trigger.write_to(pool, wi, 0.0);
            self.out_gate.write_to(pool, wi, 0.0);
            return;
        }

        let clock = self.in_clock.read_from(pool, ri);
        let start = self.in_start.read_from(pool, ri);
        let stop  = self.in_stop.read_from(pool, ri);
        let reset = self.in_reset.read_from(pool, ri);

        let clock_rose = clock >= 0.5 && self.prev_clock < 0.5;
        let start_rose = start >= 0.5 && self.prev_start < 0.5;
        let stop_rose  = stop  >= 0.5 && self.prev_stop  < 0.5;
        let reset_rose = reset >= 0.5 && self.prev_reset < 0.5;

        self.prev_clock = clock;
        self.prev_start = start;
        self.prev_stop  = stop;
        self.prev_reset = reset;

        if reset_rose {
            self.step_index = 0;
            self.trigger_pending = false;
        }

        if stop_rose {
            self.playing = false;
            self.trigger_pending = false;
        }

        if start_rose {
            self.playing = true;
        }

        if clock_rose && self.playing && !self.steps.is_empty() {
            self.step_index = (self.step_index + 1) % self.steps.len();
            self.apply_current_step();
        }

        // Determine outputs from current step
        let (gate, trigger) = if !self.playing {
            (0.0, 0.0)
        } else {
            match self.steps.get(self.step_index) {
                Some(Step::Note { .. }) => {
                    let t = if self.trigger_pending { 1.0 } else { 0.0 };
                    (1.0, t)
                }
                Some(Step::Tie) => (1.0, 0.0),
                Some(Step::Rest) | None => (0.0, 0.0),
            }
        };

        self.out_pitch.write_to(pool, wi, self.current_pitch);
        self.out_trigger.write_to(pool, wi, trigger);
        self.out_gate.write_to(pool, wi, gate);

        // trigger is a one-sample pulse
        self.trigger_pending = false;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patches_core::{AudioEnvironment, Module, ModuleShape, Registry};
    use patches_core::parameter_map::{ParameterMap, ParameterValue};

    fn make_sequencer(steps: &[&str]) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert(
            "steps".into(),
            ParameterValue::Array(steps.iter().map(|s| s.to_string()).collect()),
        );
        let mut r = Registry::new();
        r.register::<Seq>();
        r.create(
            "Seq",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels: 0, length: 32 },
            &params,
            InstanceId::next(),
        ).unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

    fn set_ports_for_test(module: &mut Box<dyn Module>) {
        // 0=clock, 1=start, 2=stop, 3=reset; 4=pitch, 5=trigger, 6=gate
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 2, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 3, scale: 1.0, connected: true }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 4, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 5, connected: true }),
            OutputPort::Mono(MonoOutput { cable_idx: 6, connected: true }),
        ];
        module.set_ports(&inputs, &outputs);
    }

    fn tick(seq: &mut dyn Module, pool: &mut Vec<[CableValue; 2]>, clock: f64, tick_count: usize) -> (f64, f64, f64) {
        let wi = tick_count % 2;
        let ri = 1 - wi;
        pool[0][ri] = CableValue::Mono(clock);
        pool[1][ri] = CableValue::Mono(0.0);
        pool[2][ri] = CableValue::Mono(0.0);
        pool[3][ri] = CableValue::Mono(0.0);
        seq.process(pool, wi);
        let pitch = if let CableValue::Mono(v) = pool[4][wi] { v } else { panic!(); };
        let trigger = if let CableValue::Mono(v) = pool[5][wi] { v } else { panic!(); };
        let gate = if let CableValue::Mono(v) = pool[6][wi] { v } else { panic!(); };
        (pitch, trigger, gate)
    }

    fn tick_ctrl(
        seq: &mut dyn Module,
        pool: &mut Vec<[CableValue; 2]>,
        clock: f64,
        start: f64,
        stop: f64,
        reset: f64,
        tick_count: usize,
    ) -> (f64, f64, f64) {
        let wi = tick_count % 2;
        let ri = 1 - wi;
        pool[0][ri] = CableValue::Mono(clock);
        pool[1][ri] = CableValue::Mono(start);
        pool[2][ri] = CableValue::Mono(stop);
        pool[3][ri] = CableValue::Mono(reset);
        seq.process(pool, wi);
        let pitch = if let CableValue::Mono(v) = pool[4][wi] { v } else { panic!(); };
        let trigger = if let CableValue::Mono(v) = pool[5][wi] { v } else { panic!(); };
        let gate = if let CableValue::Mono(v) = pool[6][wi] { v } else { panic!(); };
        (pitch, trigger, gate)
    }

    #[test]
    fn parse_note_c2() {
        assert_eq!(parse_step("C2"), Ok(Step::Note { voct: 2.0 }));
    }

    #[test]
    fn parse_note_c3() {
        assert_eq!(parse_step("C3"), Ok(Step::Note { voct: 3.0 }));
    }

    #[test]
    fn parse_note_sharp() {
        let expected = 2.0 + 1.0 / 12.0;
        match parse_step("C#2").unwrap() {
            Step::Note { voct } => assert!((voct - expected).abs() < 1e-12),
            _ => panic!("expected Note"),
        }
    }

    #[test]
    fn parse_note_flat() {
        let expected = 3.0 + 10.0 / 12.0;
        match parse_step("Bb3").unwrap() {
            Step::Note { voct } => assert!((voct - expected).abs() < 1e-12),
            _ => panic!("expected Note"),
        }
    }

    #[test]
    fn parse_rest() {
        assert_eq!(parse_step("-"), Ok(Step::Rest));
    }

    #[test]
    fn parse_tie() {
        assert_eq!(parse_step("_"), Ok(Step::Tie));
    }

    #[test]
    fn parse_invalid_returns_error() {
        assert!(parse_step("X9").is_err());
        assert!(parse_step("C").is_err());
        assert!(parse_step("").is_err());
        assert!(parse_step("C##3").is_err());
    }

    #[test]
    fn empty_pattern_succeeds_and_process_does_not_panic() {
        let mut seq = make_sequencer(&[]);
        set_ports_for_test(&mut seq);
        let mut pool = make_pool(7);
        let (pitch, trigger, gate) = tick(seq.as_mut(), &mut pool, 0.0, 0);
        assert_eq!(pitch, 0.0, "pitch should be 0.0 for empty pattern");
        assert_eq!(trigger, 0.0, "trigger should be 0.0 for empty pattern");
        assert_eq!(gate, 0.0, "gate should be 0.0 for empty pattern");
        let (pitch, trigger, gate) = tick(seq.as_mut(), &mut pool, 1.0, 1);
        assert_eq!(pitch, 0.0);
        assert_eq!(trigger, 0.0);
        assert_eq!(gate, 0.0);
    }

    #[test]
    fn invalid_step_string_returns_err_from_create() {
        let mut params = ParameterMap::new();
        params.insert(
            "steps".into(),
            ParameterValue::Array(vec!["Z9".to_string()]),
        );
        let mut r = Registry::new();
        r.register::<Seq>();
        let result = r.create(
            "StepSequencer",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels: 0, length: 32 },
            &params,
            InstanceId::next(),
        );
        assert!(result.is_err(), "expected Err for invalid step string");
    }

    #[test]
    fn basic_sequence_pitch_trigger_gate() {
        let mut seq = make_sequencer(&["C3", "D3", "-", "_"]);
        set_ports_for_test(&mut seq);
        let mut pool = make_pool(7);
        let mut tc = 0usize;

        // Start playing
        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 1.0, 0.0, 0.0, tc); tc += 1;
        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 0.0, 0.0, 0.0, tc); tc += 1;

        let (pitch, trigger, gate) = tick(seq.as_mut(), &mut pool, 0.0, tc); tc += 1;
        assert_eq!(gate, 1.0, "gate at step 0 (C3)");
        assert_eq!(trigger, 0.0, "no trigger before first clock");
        assert_eq!(pitch, 0.0, "pitch is initial current_pitch C0");

        let d3 = 3.0 + 2.0 / 12.0;
        let (pitch, trigger, gate) = tick(seq.as_mut(), &mut pool, 1.0, tc); tc += 1;
        assert_eq!(gate, 1.0, "gate on D3");
        assert_eq!(trigger, 1.0, "trigger on D3 advance");
        assert!((pitch - d3).abs() < 1e-12, "D3 voct = {}, got {}", d3, pitch);

        let (_, trigger, gate) = tick(seq.as_mut(), &mut pool, 1.0, tc); tc += 1;
        assert_eq!(trigger, 0.0);
        assert_eq!(gate, 1.0);

        tick(seq.as_mut(), &mut pool, 0.0, tc); tc += 1;

        let (pitch, trigger, gate) = tick(seq.as_mut(), &mut pool, 1.0, tc); tc += 1;
        assert_eq!(gate, 0.0, "gate on rest");
        assert_eq!(trigger, 0.0, "no trigger on rest");
        assert!((pitch - d3).abs() < 1e-12, "pitch holds D3 on rest");

        tick(seq.as_mut(), &mut pool, 0.0, tc); tc += 1;
        let (_, trigger, gate) = tick(seq.as_mut(), &mut pool, 1.0, tc); tc += 1;
        assert_eq!(gate, 1.0, "gate on tie");
        assert_eq!(trigger, 0.0, "no trigger on tie");

        tick(seq.as_mut(), &mut pool, 0.0, tc); tc += 1;
        let c3 = 3.0;
        let (pitch, trigger, gate) = tick(seq.as_mut(), &mut pool, 1.0, tc);
        assert_eq!(gate, 1.0, "gate on C3 re-entry");
        assert_eq!(trigger, 1.0, "trigger on C3 re-entry");
        assert!((pitch - c3).abs() < 1e-12, "C3 voct = {}, got {}", c3, pitch);
    }

    #[test]
    fn stop_suppresses_gate_and_blocks_clock() {
        let mut seq = make_sequencer(&["C3", "D3"]);
        set_ports_for_test(&mut seq);
        let mut pool = make_pool(7);
        let mut tc = 0usize;

        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 1.0, 0.0, 0.0, tc); tc += 1;
        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 0.0, 0.0, 0.0, tc); tc += 1;

        tick(seq.as_mut(), &mut pool, 1.0, tc); tc += 1;
        tick(seq.as_mut(), &mut pool, 0.0, tc); tc += 1;

        let (_, trigger, gate) = tick_ctrl(seq.as_mut(), &mut pool, 0.0, 0.0, 1.0, 0.0, tc); tc += 1;
        assert_eq!(gate, 0.0, "gate suppressed on stop");
        assert_eq!(trigger, 0.0);

        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 0.0, 0.0, 0.0, tc); tc += 1;
        let (_, _, gate) = tick_ctrl(seq.as_mut(), &mut pool, 1.0, 0.0, 0.0, 0.0, tc); tc += 1;
        assert_eq!(gate, 0.0, "gate stays 0 while stopped");

        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 0.0, 0.0, 0.0, tc); tc += 1;
        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 1.0, 0.0, 0.0, tc); tc += 1;
        let (_, _, gate) = tick_ctrl(seq.as_mut(), &mut pool, 0.0, 0.0, 0.0, 0.0, tc);
        assert_eq!(gate, 1.0, "gate restored after start");
    }

    #[test]
    fn reset_returns_to_step_zero_then_advance() {
        let mut seq = make_sequencer(&["C3", "D3", "E3"]);
        set_ports_for_test(&mut seq);
        let mut pool = make_pool(7);
        let mut tc = 0usize;

        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 1.0, 0.0, 0.0, tc); tc += 1;
        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 0.0, 0.0, 0.0, tc); tc += 1;

        tick(seq.as_mut(), &mut pool, 1.0, tc); tc += 1;
        tick(seq.as_mut(), &mut pool, 0.0, tc); tc += 1;
        tick(seq.as_mut(), &mut pool, 1.0, tc); tc += 1;
        tick(seq.as_mut(), &mut pool, 0.0, tc); tc += 1;

        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 0.0, 0.0, 1.0, tc); tc += 1;
        tick_ctrl(seq.as_mut(), &mut pool, 0.0, 0.0, 0.0, 0.0, tc); tc += 1;

        let (pitch, trigger, gate) = tick(seq.as_mut(), &mut pool, 1.0, tc);
        let d3 = 3.0 + 2.0 / 12.0;
        assert!((pitch - d3).abs() < 1e-12, "D3 voct after reset, got {}", pitch);
        assert_eq!(trigger, 1.0);
        assert_eq!(gate, 1.0);
    }
}
