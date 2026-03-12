use std::f64::consts::{FRAC_1_SQRT_2, TAU};
use crate::common::approximate::fast_tanh;

use patches_core::{
    AudioEnvironment, CableValue, InputPort, InstanceId, Module, ModuleDescriptor,
    MonoInput, MonoOutput, ModuleShape, OutputPort, ParameterDescriptor, ParameterKind, PortDescriptor,
};
use patches_core::CableKind;
use patches_core::parameter_map::{ParameterMap, ParameterValue};

/// Number of samples between biquad-coefficient recomputations in the
/// CV-modulated code path. At 48 kHz this gives a ~1500 Hz refresh rate —
/// fast enough for LFO and envelope modulation. Linear interpolation of
/// coefficients across the interval prevents audible stepping.
const COEFF_UPDATE_INTERVAL: u32 = 32;
const COEFF_UPDATE_INTERVAL_RECIPROCAL: f64 = 1.0 / COEFF_UPDATE_INTERVAL as f64;

/// Maps normalised resonance [0, 1] to filter Q.
///
/// At 0.0 the Q equals the Butterworth value (≈ 0.707), giving a maximally
/// flat pass-band with no resonance peak. At 1.0 the Q is 10.0, producing
/// strong, audible resonance without self-oscillation.
#[inline]
fn resonance_to_q(resonance: f64) -> f64 {
    // 0.0 → Q = 1/√2 ≈ 0.707 (Butterworth), 1.0 → Q = 10.0
    FRAC_1_SQRT_2 + (10.0 - FRAC_1_SQRT_2) * resonance
}

/// Compute normalised biquad lowpass coefficients (a0 = 1).
///
/// Uses the Audio EQ Cookbook (RBJ) design equations. `cutoff_hz` is clamped
/// to [1, sample_rate × 0.499] to prevent instability near DC or Nyquist.
///
/// Returns `(b0, b1, b2, a1, a2)` ready for Transposed Direct Form II.
#[inline]
fn compute_biquad_lowpass(cutoff_hz: f64, resonance: f64, sample_rate: f64) -> (f64, f64, f64, f64, f64) {
    let q = resonance_to_q(resonance);
    let f = cutoff_hz.clamp(1.0, sample_rate * 0.499);
    let w0 = TAU * f / sample_rate;
    let sin_w0 = w0.sin();
    let cos_w0 = w0.cos();
    let alpha = sin_w0 / (2.0 * q);
    let inv_a0 = 1.0 / (1.0 + alpha);
    let b0 = (1.0 - cos_w0) * 0.5 * inv_a0;
    let b1 = (1.0 - cos_w0) * inv_a0;
    let b2 = b0;
    let a1 = -2.0 * cos_w0 * inv_a0;
    let a2 = (1.0 - alpha) * inv_a0;
    (b0, b1, b2, a1, a2)
}

/// Resonant lowpass filter (biquad, Transposed Direct Form II).
///
/// **Port layout**
///
/// | Index | Name            | Direction | Description                              |
/// |-------|-----------------|-----------|------------------------------------------|
/// | 0     | `in`            | input     | Audio signal to filter                   |
/// | 1     | `cutoff_cv`     | input     | V/oct offset applied to cutoff frequency |
/// | 2     | `resonance_cv`  | input     | Additive offset for normalised resonance |
/// | 0     | `out`           | output    | Filtered signal                          |
///
/// **Parameters**
///
/// | Name        | Range        | Default | Description                             |
/// |-------------|--------------|---------|-----------------------------------------|
/// | `cutoff`    | 20–20 000 Hz | 1000    | Base cutoff frequency in Hz             |
/// | `resonance` | 0.0–1.0      | 0.0     | Resonance (0 = Butterworth, 1 = max)    |
///
/// **Connectivity optimisation**
///
/// When neither `cutoff_cv` nor `resonance_cv` is connected the module
/// computes biquad coefficients once per parameter change and runs a
/// zero-overhead static-coefficient path in `process`. When one or both CV
/// inputs are connected coefficients are recomputed every
/// [`COEFF_UPDATE_INTERVAL`] samples using the live CV values, and linearly
/// interpolated sample-by-sample between updates. This avoids per-sample
/// trigonometric evaluation while keeping changes smooth enough that no
/// audible zipper artefacts are introduced at LFO or envelope modulation rates.
pub struct ResonantLowpass {
    instance_id: InstanceId,
    descriptor: ModuleDescriptor,
    sample_rate: f64,

    // ── Parameters ────────────────────────────────────────────────────────
    cutoff: f64,    // Hz
    resonance: f64, // 0–1 normalised

    // ── Current biquad coefficients (what the filter uses this sample) ───
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,

    // ── Target coefficients and per-sample increments (CV path only) ─────
    b0t: f64,
    b1t: f64,
    b2t: f64,
    a1t: f64,
    a2t: f64,
    db0: f64,
    db1: f64,
    db2: f64,
    da1: f64,
    da2: f64,

    // ── Filter state (Transposed Direct Form II) ──────────────────────────
    s1: f64,
    s2: f64,

    // ── Update counter (CV path only) ─────────────────────────────────────
    update_counter: u32,

    // ── Saturation ────────────────────────────────────────────────────────
    saturate: bool,

    // ── Port fields ───────────────────────────────────────────────────────
    in_audio: MonoInput,
    in_cutoff_cv: MonoInput,
    in_resonance_cv: MonoInput,
    out_audio: MonoOutput,
}

impl ResonantLowpass {
    /// Recompute coefficients from the base parameters and sync both the
    /// current and target slots. Used when parameters change in static mode,
    /// or when connectivity transitions from CV to static.
    fn recompute_static_coeffs(&mut self) {
        let (b0, b1, b2, a1, a2) =
            compute_biquad_lowpass(self.cutoff, self.resonance, self.sample_rate);
        self.b0 = b0;
        self.b1 = b1;
        self.b2 = b2;
        self.a1 = a1;
        self.a2 = a2;
        self.b0t = b0;
        self.b1t = b1;
        self.b2t = b2;
        self.a1t = a1;
        self.a2t = a2;
        self.db0 = 0.0;
        self.db1 = 0.0;
        self.db2 = 0.0;
        self.da1 = 0.0;
        self.da2 = 0.0;
    }

    fn any_cv_connected(&self) -> bool {
        self.in_cutoff_cv.is_connected() || self.in_resonance_cv.is_connected()
    }
}

impl Module for ResonantLowpass {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "ResonantLowpass",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "in", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "cutoff_cv", index: 0, kind: CableKind::Mono },
                PortDescriptor { name: "resonance_cv", index: 0, kind: CableKind::Mono },
            ],
            outputs: vec![PortDescriptor { name: "out", index: 0, kind: CableKind::Mono }],
            parameters: vec![
                ParameterDescriptor {
                    name: "cutoff",
                    index: 0,
                    parameter_type: ParameterKind::Float {
                        min: 20.0,
                        max: 20_000.0,
                        default: 1000.0,
                    },
                },
                ParameterDescriptor {
                    name: "resonance",
                    index: 1,
                    parameter_type: ParameterKind::Float {
                        min: 0.0,
                        max: 1.0,
                        default: 0.0,
                    },
                },
                ParameterDescriptor {
                    name: "saturate",
                    index: 0,
                    parameter_type: ParameterKind::Bool { default: false },
                },
            ],
            is_sink: false,
        }
    }

    fn prepare(audio_environment: &AudioEnvironment, descriptor: ModuleDescriptor, instance_id: InstanceId) -> Self {
        let default_cutoff = 1000.0;
        let default_resonance = 0.0;
        let (b0, b1, b2, a1, a2) =
            compute_biquad_lowpass(default_cutoff, default_resonance, audio_environment.sample_rate);
        Self {
            instance_id,
            descriptor,
            sample_rate: audio_environment.sample_rate,
            cutoff: default_cutoff,
            resonance: default_resonance,
            b0,
            b1,
            b2,
            a1,
            a2,
            b0t: b0,
            b1t: b1,
            b2t: b2,
            a1t: a1,
            a2t: a2,
            db0: 0.0,
            db1: 0.0,
            db2: 0.0,
            da1: 0.0,
            da2: 0.0,
            s1: 0.0,
            s2: 0.0,
            update_counter: 0,
            saturate: false,
            in_audio: MonoInput::default(),
            in_cutoff_cv: MonoInput::default(),
            in_resonance_cv: MonoInput::default(),
            out_audio: MonoOutput::default(),
        }
    }

    fn update_validated_parameters(&mut self, params: &ParameterMap) {
        if let Some(ParameterValue::Float(v)) = params.get("cutoff") {
            self.cutoff = *v;
        }
        if let Some(ParameterValue::Float(v)) = params.get("resonance") {
            self.resonance = *v;
        }
        if let Some(ParameterValue::Bool(v)) = params.get("saturate") {
            self.saturate = *v;
        }
        // In the CV path the next update_counter == 0 will recompute using the
        // new base parameters combined with the live CV values. In the static
        // path we recompute immediately.
        if !self.any_cv_connected() {
            self.recompute_static_coeffs();
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_ports(&mut self, inputs: &[InputPort], outputs: &[OutputPort]) {
        self.in_audio = MonoInput::from_ports(inputs, 0);
        self.in_cutoff_cv = MonoInput::from_ports(inputs, 1);
        self.in_resonance_cv = MonoInput::from_ports(inputs, 2);
        self.out_audio = MonoOutput::from_ports(outputs, 0);
        // If connectivity changed to non-CV, recompute static coefficients.
        if !self.any_cv_connected() {
            self.recompute_static_coeffs();
        }
    }

    fn process(&mut self, pool: &mut [[CableValue; 2]], wi: usize) {
        let ri = 1 - wi;

        if !self.any_cv_connected() {
            // ── Static path: coefficients do not change ───────────────────
            let x = self.in_audio.read_from(pool, ri);
            let y = self.b0 * x + self.s1;
            let fb = if self.saturate { fast_tanh(y) } else { y };
            self.s1 = self.b1 * x - self.a1 * fb + self.s2;
            self.s2 = self.b2 * x - self.a2 * fb;
            self.out_audio.write_to(pool, wi, y);
            return;
        }

        // ── CV path: recompute coefficients every COEFF_UPDATE_INTERVAL ──
        if self.update_counter == 0 {
            // Snap to the previous target to eliminate accumulated float
            // drift before starting a new interpolation ramp.
            self.b0 = self.b0t;
            self.b1 = self.b1t;
            self.b2 = self.b2t;
            self.a1 = self.a1t;
            self.a2 = self.a2t;

            // Effective parameters: base values offset by CV.
            // cutoff_cv is V/oct: +1 V doubles the frequency.
            let cutoff_cv = if self.in_cutoff_cv.is_connected() {
                self.in_cutoff_cv.read_from(pool, ri)
            } else {
                0.0
            };
            let resonance_cv = if self.in_resonance_cv.is_connected() {
                self.in_resonance_cv.read_from(pool, ri)
            } else {
                0.0
            };
            let effective_cutoff =
                (self.cutoff * cutoff_cv.exp2()).clamp(20.0, self.sample_rate * 0.499);
            let effective_resonance = (self.resonance + resonance_cv).clamp(0.0, 1.0);

            let (b0t, b1t, b2t, a1t, a2t) =
                compute_biquad_lowpass(effective_cutoff, effective_resonance, self.sample_rate);

            self.db0 = (b0t - self.b0) * COEFF_UPDATE_INTERVAL_RECIPROCAL;
            self.db1 = (b1t - self.b1) * COEFF_UPDATE_INTERVAL_RECIPROCAL;
            self.db2 = (b2t - self.b2) * COEFF_UPDATE_INTERVAL_RECIPROCAL;
            self.da1 = (a1t - self.a1) * COEFF_UPDATE_INTERVAL_RECIPROCAL;
            self.da2 = (a2t - self.a2) * COEFF_UPDATE_INTERVAL_RECIPROCAL;

            self.b0t = b0t;
            self.b1t = b1t;
            self.b2t = b2t;
            self.a1t = a1t;
            self.a2t = a2t;
        }

        // Apply filter (Transposed Direct Form II).
        let x = self.in_audio.read_from(pool, ri);
        let y = self.b0 * x + self.s1;
        let fb = if self.saturate { fast_tanh(y) } else { y };
        self.s1 = self.b1 * x - self.a1 * fb + self.s2;
        self.s2 = self.b2 * x - self.a2 * fb;
        self.out_audio.write_to(pool, wi, y);

        // Advance interpolation toward the target.
        self.b0 += self.db0;
        self.b1 += self.db1;
        self.b2 += self.db2;
        self.a1 += self.da1;
        self.a2 += self.da2;

        self.update_counter += 1;
        if self.update_counter >= COEFF_UPDATE_INTERVAL {
            self.update_counter = 0;
        }
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

    fn make_filter(cutoff: f64, resonance: f64) -> Box<dyn Module> {
        make_filter_sr(cutoff, resonance, 44100.0)
    }

    fn make_filter_sr(cutoff: f64, resonance: f64, sample_rate: f64) -> Box<dyn Module> {
        let mut params = ParameterMap::new();
        params.insert("cutoff".into(), ParameterValue::Float(cutoff));
        params.insert("resonance".into(), ParameterValue::Float(resonance));
        let mut r = Registry::new();
        r.register::<ResonantLowpass>();
        r.create(
            "ResonantLowpass",
            &AudioEnvironment { sample_rate },
            &ModuleShape { channels: 0, length: 0 },
            &params,
            InstanceId::next(),
        )
        .unwrap()
    }

    fn make_pool(n: usize) -> Vec<[CableValue; 2]> {
        vec![[CableValue::Mono(0.0); 2]; n]
    }

    // Ports: 0=in, 1=cutoff_cv, 2=resonance_cv, 3=out
    fn set_static_ports(module: &mut Box<dyn Module>) {
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: false }),
            InputPort::Mono(MonoInput { cable_idx: 2, scale: 1.0, connected: false }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 3, connected: true }),
        ];
        module.set_ports(&inputs, &outputs);
    }

    fn set_cv_ports(module: &mut Box<dyn Module>) {
        let inputs = vec![
            InputPort::Mono(MonoInput { cable_idx: 0, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 1, scale: 1.0, connected: true }),
            InputPort::Mono(MonoInput { cable_idx: 2, scale: 1.0, connected: true }),
        ];
        let outputs = vec![
            OutputPort::Mono(MonoOutput { cable_idx: 3, connected: true }),
        ];
        module.set_ports(&inputs, &outputs);
    }

    /// Settle a filter by running `n` silent samples through it.
    fn settle(m: &mut Box<dyn Module>, n: usize) {
        let mut pool = make_pool(4);
        for i in 0..n {
            pool[0][1 - (i % 2)] = CableValue::Mono(0.0);
            m.process(&mut pool, i % 2);
        }
    }

    /// Measure the peak absolute output of `m` driven by a sine at `freq_hz`
    /// over `n` samples at `sample_rate`.
    fn measure_peak(m: &mut Box<dyn Module>, freq_hz: f64, sample_rate: f64, n: usize) -> f64 {
        let mut pool = make_pool(4);
        let mut peak = 0.0f64;
        for i in 0..n {
            let x = (TAU * freq_hz * i as f64 / sample_rate).sin();
            pool[0][1 - (i % 2)] = CableValue::Mono(x);
            m.process(&mut pool, i % 2);
            if let CableValue::Mono(v) = pool[3][i % 2] {
                peak = peak.max(v.abs());
            }
        }
        peak
    }

    #[test]
    fn descriptor_shape() {
        let f = make_filter(1000.0, 0.0);
        let desc = f.descriptor();
        assert_eq!(desc.module_name, "ResonantLowpass");
        assert_eq!(desc.inputs.len(), 3);
        assert_eq!(desc.inputs[0].name, "in");
        assert_eq!(desc.inputs[1].name, "cutoff_cv");
        assert_eq!(desc.inputs[2].name, "resonance_cv");
        assert_eq!(desc.outputs.len(), 1);
        assert_eq!(desc.outputs[0].name, "out");
    }

    #[test]
    fn instance_ids_are_distinct() {
        let a = make_filter(1000.0, 0.0);
        let b = make_filter(1000.0, 0.0);
        assert_ne!(a.instance_id(), b.instance_id());
    }

    #[test]
    fn passes_dc_after_settling() {
        let mut f = make_filter(1000.0, 0.0);
        set_static_ports(&mut f);
        let mut pool = make_pool(4);
        for i in 0..4096 {
            pool[0][1 - (i % 2)] = CableValue::Mono(1.0);
            f.process(&mut pool, i % 2);
        }
        if let CableValue::Mono(v) = pool[3][4095 % 2] {
            assert!(
                (v - 1.0).abs() < 0.001,
                "DC should pass through lowpass; got {}",
                v
            );
        } else { panic!("expected Mono"); }
    }

    #[test]
    fn attenuates_above_cutoff() {
        let sr = 44100.0;
        let mut f = make_filter_sr(1000.0, 0.0, sr);
        set_static_ports(&mut f);
        settle(&mut f, 4096);
        let peak = measure_peak(&mut f, 10_000.0, sr, 1024);
        assert!(
            peak < 0.05,
            "expected strong attenuation above cutoff; peak was {}",
            peak
        );
    }

    #[test]
    fn resonance_boosts_near_cutoff() {
        let sr = 44100.0;
        let cutoff = 1000.0;
        let mut flat = make_filter_sr(cutoff, 0.0, sr);
        let mut resonant = make_filter_sr(cutoff, 0.8, sr);
        set_static_ports(&mut flat);
        set_static_ports(&mut resonant);
        settle(&mut flat, 4096);
        settle(&mut resonant, 4096);
        let flat_peak = measure_peak(&mut flat, cutoff, sr, 4096);
        let res_peak = measure_peak(&mut resonant, cutoff, sr, 4096);
        assert!(
            res_peak > flat_peak * 1.5,
            "resonance should boost signal near cutoff; flat={flat_peak}, resonant={res_peak}"
        );
    }

    #[test]
    fn cutoff_cv_shifts_cutoff_upward() {
        let sr = 44100.0;
        let base_cutoff = 500.0;
        let test_freq = 800.0;

        let mut no_cv = make_filter_sr(base_cutoff, 0.0, sr);
        let mut with_cv = make_filter_sr(base_cutoff, 0.0, sr);
        set_static_ports(&mut no_cv);
        set_cv_ports(&mut with_cv);

        let mut pool_no_cv = make_pool(4);
        let mut pool_with_cv = make_pool(4);

        // Settle both filters; with_cv receives +1 V during settling.
        for i in 0..4096 {
            pool_no_cv[0][1 - (i % 2)] = CableValue::Mono(0.0);
            no_cv.process(&mut pool_no_cv, i % 2);
            pool_with_cv[0][1 - (i % 2)] = CableValue::Mono(0.0);
            pool_with_cv[1][1 - (i % 2)] = CableValue::Mono(1.0);
            pool_with_cv[2][1 - (i % 2)] = CableValue::Mono(0.0);
            with_cv.process(&mut pool_with_cv, i % 2);
        }

        let mut no_cv_peak = 0.0f64;
        let mut with_cv_peak = 0.0f64;
        for i in 0..4096usize {
            let x = (TAU * test_freq * i as f64 / sr).sin();
            pool_no_cv[0][1 - (i % 2)] = CableValue::Mono(x);
            no_cv.process(&mut pool_no_cv, i % 2);
            if let CableValue::Mono(v) = pool_no_cv[3][i % 2] {
                no_cv_peak = no_cv_peak.max(v.abs());
            }
            pool_with_cv[0][1 - (i % 2)] = CableValue::Mono(x);
            pool_with_cv[1][1 - (i % 2)] = CableValue::Mono(1.0);
            pool_with_cv[2][1 - (i % 2)] = CableValue::Mono(0.0);
            with_cv.process(&mut pool_with_cv, i % 2);
            if let CableValue::Mono(v) = pool_with_cv[3][i % 2] {
                with_cv_peak = with_cv_peak.max(v.abs());
            }
        }

        assert!(
            with_cv_peak > no_cv_peak * 1.5,
            "cutoff_cv +1 oct should raise cutoff and reduce attenuation at {test_freq} Hz; \
             no_cv={no_cv_peak:.4}, with_cv={with_cv_peak:.4}"
        );
    }

    #[test]
    fn static_path_passes_dc_when_no_cv() {
        let mut f = make_filter(1000.0, 0.0);
        set_static_ports(&mut f);
        let mut pool = make_pool(4);
        for i in 0..4096 {
            pool[0][1 - (i % 2)] = CableValue::Mono(1.0);
            f.process(&mut pool, i % 2);
        }
        if let CableValue::Mono(v) = pool[3][4095 % 2] {
            assert!(
                (v - 1.0).abs() < 0.001,
                "DC should pass in static path; got {}",
                v
            );
        } else { panic!("expected Mono"); }
    }
}
