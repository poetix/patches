use patches_core::{
    AudioEnvironment, ControlSignal, InstanceId, Module, ModuleDescriptor, ModuleShape,
    ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::build_error::BuildError;
use patches_core::parameter_map::{ParameterMap, ParameterValue};

/// A pre-parsed step in the sequencer pattern.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    /// A named note with a V/OCT pitch value (relative to C2=0.0).
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
    let voct = (octave as f64 - 2.0) + semitone_index as f64 / 12.0;
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
///   outputs[0] — pitch  (V/OCT, C2=0.0)
///   outputs[1] — trigger (1.0 on the clock-advance sample, 0.0 otherwise)
///   outputs[2] — gate    (1.0 while a note or tie is active)
pub struct StepSequencer {
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
}

impl StepSequencer {
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

impl Module for StepSequencer {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "StepSequencer",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "clock", index: 0 },
                PortDescriptor { name: "start", index: 0 },
                PortDescriptor { name: "stop",  index: 0 },
                PortDescriptor { name: "reset", index: 0 },
            ],
            outputs: vec![
                PortDescriptor { name: "pitch",   index: 0 },
                PortDescriptor { name: "trigger", index: 0 },
                PortDescriptor { name: "gate",    index: 0 },
            ],
            parameters: vec![
                ParameterDescriptor {
                    name: "steps",
                    index: 0,
                    parameter_type: ParameterKind::Array { default: &[] },
                },
            ],
            is_sink: false,
        }
    }

    fn prepare(_audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor) -> Self {
        Self {
            instance_id: InstanceId::next(),
            descriptor,
            steps: Vec::new(),
            step_index: 0,
            playing: true,
            current_pitch: 0.0,
            trigger_pending: false,
            prev_clock: 0.0,
            prev_start: 0.0,
            prev_stop: 0.0,
            prev_reset: 0.0,
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
            self.step_index = 0;
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

    fn receive_signal(&mut self, _signal: ControlSignal) {}

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        // Guard: when the pattern is empty all outputs hold at rest values.
        if self.steps.is_empty() {
            outputs[0] = 0.0;
            outputs[1] = 0.0;
            outputs[2] = 0.0;
            return;
        }

        let clock = inputs[0];
        let start = inputs[1];
        let stop  = inputs[2];
        let reset = inputs[3];

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

        outputs[0] = self.current_pitch;
        outputs[1] = trigger;
        outputs[2] = gate;

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
        r.register::<StepSequencer>();
        r.create(
            "StepSequencer",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels: 0 },
            &params,
        ).unwrap()
    }

    fn tick(seq: &mut dyn Module, clock: f64) -> (f64, f64, f64) {
        let inputs = [clock, 0.0, 0.0, 0.0];
        let mut outputs = [0.0f64; 3];
        seq.process(&inputs, &mut outputs);
        (outputs[0], outputs[1], outputs[2]) // pitch, trigger, gate
    }

    fn tick_ctrl(
        seq: &mut dyn Module,
        clock: f64,
        start: f64,
        stop: f64,
        reset: f64,
    ) -> (f64, f64, f64) {
        let inputs = [clock, start, stop, reset];
        let mut outputs = [0.0f64; 3];
        seq.process(&inputs, &mut outputs);
        (outputs[0], outputs[1], outputs[2])
    }

    #[test]
    fn parse_note_c2() {
        assert_eq!(parse_step("C2"), Ok(Step::Note { voct: 0.0 }));
    }

    #[test]
    fn parse_note_c3() {
        assert_eq!(parse_step("C3"), Ok(Step::Note { voct: 1.0 }));
    }

    #[test]
    fn parse_note_sharp() {
        // C#2: semitone 1, octave 2 → voct = 1/12
        let expected = 1.0 / 12.0;
        match parse_step("C#2").unwrap() {
            Step::Note { voct } => assert!((voct - expected).abs() < 1e-12),
            _ => panic!("expected Note"),
        }
    }

    #[test]
    fn parse_note_flat() {
        // Bb3: B(11) + b(-1) = 10, octave 3 → voct = 1.0 + 10/12
        let expected = 1.0 + 10.0 / 12.0;
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
    fn descriptor_shape() {
        let m = make_sequencer(&["C3", "D3"]);
        let desc = m.descriptor();
        assert_eq!(desc.inputs.len(), 4);
        assert_eq!(desc.outputs.len(), 3);
        assert_eq!(desc.inputs[0].name, "clock");
        assert_eq!(desc.inputs[1].name, "start");
        assert_eq!(desc.inputs[2].name, "stop");
        assert_eq!(desc.inputs[3].name, "reset");
        assert_eq!(desc.outputs[0].name, "pitch");
        assert_eq!(desc.outputs[1].name, "trigger");
        assert_eq!(desc.outputs[2].name, "gate");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_sequencer(&["C3"]);
        let b = make_sequencer(&["C3"]);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn empty_pattern_succeeds_and_process_does_not_panic() {
        let mut seq = make_sequencer(&[]);
        let (pitch, trigger, gate) = tick(seq.as_mut(), 0.0);
        assert_eq!(pitch, 0.0, "pitch should be 0.0 for empty pattern");
        assert_eq!(trigger, 0.0, "trigger should be 0.0 for empty pattern");
        assert_eq!(gate, 0.0, "gate should be 0.0 for empty pattern");
        // Also tick with a rising clock edge to confirm no panic
        let (pitch, trigger, gate) = tick(seq.as_mut(), 1.0);
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
        r.register::<StepSequencer>();
        let result = r.create(
            "StepSequencer",
            &AudioEnvironment { sample_rate: 44100.0 },
            &ModuleShape { channels: 0 },
            &params,
        );
        assert!(result.is_err(), "expected Err for invalid step string");
    }

    #[test]
    fn basic_sequence_pitch_trigger_gate() {
        // Pattern: C3 D3 - _
        // step 0=C3, step 1=D3, step 2=Rest, step 3=Tie
        // Note: playing starts as false, so we use start signal or simply clock to advance.
        // The sequencer starts with playing=false; we need to start it first.
        let mut seq = make_sequencer(&["C3", "D3", "-", "_"]);

        // Start playing
        tick_ctrl(seq.as_mut(), 0.0, 1.0, 0.0, 0.0);
        tick_ctrl(seq.as_mut(), 0.0, 0.0, 0.0, 0.0);

        // Before first clock: step_index=0 → C3 Note, but trigger_pending=false.
        // playing=true, current_pitch=0.0 (C2), gate=1 (Note step), trigger=0.
        let (pitch, trigger, gate) = tick(seq.as_mut(), 0.0);
        assert_eq!(gate, 1.0, "gate at step 0 (C3)");
        assert_eq!(trigger, 0.0, "no trigger before first clock");
        assert_eq!(pitch, 0.0, "pitch is initial current_pitch C2");

        // Rising edge → advance to step 1 = D3
        let d3 = 1.0 + 2.0 / 12.0; // (3-2) + semitone(D=2)/12
        let (pitch, trigger, gate) = tick(seq.as_mut(), 1.0);
        assert_eq!(gate, 1.0, "gate on D3");
        assert_eq!(trigger, 1.0, "trigger on D3 advance");
        assert!((pitch - d3).abs() < 1e-12, "D3 voct = {}, got {}", d3, pitch);

        // Clock held high → no second rising edge; trigger drops
        let (_, trigger, gate) = tick(seq.as_mut(), 1.0);
        assert_eq!(trigger, 0.0);
        assert_eq!(gate, 1.0);

        // Clock low
        tick(seq.as_mut(), 0.0);

        // Rising edge → advance to step 2 = Rest
        let (pitch, trigger, gate) = tick(seq.as_mut(), 1.0);
        assert_eq!(gate, 0.0, "gate on rest");
        assert_eq!(trigger, 0.0, "no trigger on rest");
        assert!((pitch - d3).abs() < 1e-12, "pitch holds D3 on rest");

        // Clock low, then rising edge → advance to step 3 = Tie
        tick(seq.as_mut(), 0.0);
        let (_, trigger, gate) = tick(seq.as_mut(), 1.0);
        assert_eq!(gate, 1.0, "gate on tie");
        assert_eq!(trigger, 0.0, "no trigger on tie");

        // Clock low, then rising edge → wraps back to step 0 = C3
        tick(seq.as_mut(), 0.0);
        let c3 = 1.0; // (3-2) + 0/12
        let (pitch, trigger, gate) = tick(seq.as_mut(), 1.0);
        assert_eq!(gate, 1.0, "gate on C3 re-entry");
        assert_eq!(trigger, 1.0, "trigger on C3 re-entry");
        assert!((pitch - c3).abs() < 1e-12, "C3 voct = {}, got {}", c3, pitch);
    }

    #[test]
    fn stop_suppresses_gate_and_blocks_clock() {
        let mut seq = make_sequencer(&["C3", "D3"]);

        // Start playing first
        tick_ctrl(seq.as_mut(), 0.0, 1.0, 0.0, 0.0);
        tick_ctrl(seq.as_mut(), 0.0, 0.0, 0.0, 0.0);

        // Advance to step 1 = D3 via rising edge
        tick(seq.as_mut(), 1.0);
        tick(seq.as_mut(), 0.0);

        // Stop
        let (_, trigger, gate) = tick_ctrl(seq.as_mut(), 0.0, 0.0, 1.0, 0.0);
        assert_eq!(gate, 0.0, "gate suppressed on stop");
        assert_eq!(trigger, 0.0);

        // Clock while stopped → no advance, gate stays 0
        tick_ctrl(seq.as_mut(), 0.0, 0.0, 0.0, 0.0);
        let (_, _, gate) = tick_ctrl(seq.as_mut(), 1.0, 0.0, 0.0, 0.0);
        assert_eq!(gate, 0.0, "gate stays 0 while stopped");

        // Start
        tick_ctrl(seq.as_mut(), 0.0, 0.0, 0.0, 0.0);
        tick_ctrl(seq.as_mut(), 0.0, 1.0, 0.0, 0.0);
        let (_, _, gate) = tick_ctrl(seq.as_mut(), 0.0, 0.0, 0.0, 0.0);
        // Should be at step 1 still (D3), gate=1
        assert_eq!(gate, 1.0, "gate restored after start");
    }

    #[test]
    fn reset_returns_to_step_zero_then_advance() {
        let mut seq = make_sequencer(&["C3", "D3", "E3"]);

        // Start playing first
        tick_ctrl(seq.as_mut(), 0.0, 1.0, 0.0, 0.0);
        tick_ctrl(seq.as_mut(), 0.0, 0.0, 0.0, 0.0);

        // Advance to step 2 = E3
        tick(seq.as_mut(), 1.0);
        tick(seq.as_mut(), 0.0);
        tick(seq.as_mut(), 1.0);
        tick(seq.as_mut(), 0.0);

        // Reset → step_index = 0
        tick_ctrl(seq.as_mut(), 0.0, 0.0, 0.0, 1.0);
        tick_ctrl(seq.as_mut(), 0.0, 0.0, 0.0, 0.0);

        // Next clock → advance from 0 to step 1 = D3
        let (pitch, trigger, gate) = tick(seq.as_mut(), 1.0);
        let d3 = 1.0 + 2.0 / 12.0; // D3: (3-2) + semitone(D=2)/12
        assert!((pitch - d3).abs() < 1e-12, "D3 voct after reset, got {}", pitch);
        assert_eq!(trigger, 1.0);
        assert_eq!(gate, 1.0);
    }
}
