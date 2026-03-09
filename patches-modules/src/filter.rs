use std::f64::consts::{FRAC_1_SQRT_2, TAU};
use crate::common::approximate::fast_tanh;

use patches_core::{
    AudioEnvironment, InstanceId, Module, ModuleDescriptor,
    ModuleShape, ParameterDescriptor, ParameterKind, PortConnectivity, PortDescriptor,
};
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

    // ── Connectivity ──────────────────────────────────────────────────────
    any_cv_connected: bool,

    // ── Update counter (CV path only) ─────────────────────────────────────
    update_counter: u32,

    // ── Saturation ────────────────────────────────────────────────────────
    saturate: bool,
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
}

impl Module for ResonantLowpass {
    fn describe(shape: &ModuleShape) -> ModuleDescriptor {
        ModuleDescriptor {
            module_name: "ResonantLowpass",
            shape: shape.clone(),
            inputs: vec![
                PortDescriptor { name: "in", index: 0 },
                PortDescriptor { name: "cutoff_cv", index: 0 },
                PortDescriptor { name: "resonance_cv", index: 0 },
            ],
            outputs: vec![PortDescriptor { name: "out", index: 0 }],
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
            any_cv_connected: false,
            update_counter: 0,
            saturate: false,
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
        if !self.any_cv_connected {
            self.recompute_static_coeffs();
        }
    }

    fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }

    fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    fn set_connectivity(&mut self, connectivity: PortConnectivity) {
        let cutoff_cv_connected = connectivity.inputs[1];
        let resonance_cv_connected = connectivity.inputs[2];
        let any_cv_connected = cutoff_cv_connected || resonance_cv_connected;
        if any_cv_connected != self.any_cv_connected {
            self.any_cv_connected = any_cv_connected;
            if !any_cv_connected {
                // Transitioning to static path: sync coefficients to the current
                // target to prevent zipper noise.
                self.b0 = self.b0t;
                self.b1 = self.b1t;
                self.b2 = self.b2t;
                self.a1 = self.a1t;
                self.a2 = self.a2t;
            }
        }
    }

    fn process(&mut self, inputs: &[f64], outputs: &mut [f64]) {
        if !self.any_cv_connected {
            // ── Static path: coefficients do not change ───────────────────
            let x = inputs[0];
            let y = self.b0 * x + self.s1;
            let fb = if self.saturate { fast_tanh(y) } else { y };
            self.s1 = self.b1 * x - self.a1 * fb + self.s2;
            self.s2 = self.b2 * x - self.a2 * fb;
            outputs[0] = y;
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
            let effective_cutoff =
                (self.cutoff * inputs[1].exp2()).clamp(20.0, self.sample_rate * 0.499);
            let effective_resonance = (self.resonance + inputs[2]).clamp(0.0, 1.0);

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
        let x = inputs[0];
        let y = self.b0 * x + self.s1;
        let fb = if self.saturate { fast_tanh(y) } else { y };
        self.s1 = self.b1 * x - self.a1 * fb + self.s2;
        self.s2 = self.b2 * x - self.a2 * fb;
        outputs[0] = y;

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
    use patches_core::{AudioEnvironment, Module, ModuleShape, PortConnectivity, Registry};
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

    /// Settle a filter by running `n` silent samples through it.
    fn settle(m: &mut Box<dyn Module>, n: usize) {
        let mut out = [0.0f64];
        for _ in 0..n {
            m.process(&[0.0, 0.0, 0.0], &mut out);
        }
    }

    /// Measure the peak absolute output of `m` driven by a sine at `freq_hz`
    /// over `n` samples at `sample_rate`.
    fn measure_peak(m: &mut Box<dyn Module>, freq_hz: f64, sample_rate: f64, n: usize) -> f64 {
        let mut out = [0.0f64];
        let mut peak = 0.0f64;
        for i in 0..n {
            let x = (TAU * freq_hz * i as f64 / sample_rate).sin();
            m.process(&[x, 0.0, 0.0], &mut out);
            peak = peak.max(out[0].abs());
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
        // A DC signal (constant 1.0) should pass through a lowpass nearly
        // unattenuated once the filter has settled.
        let mut f = make_filter(1000.0, 0.0);
        let mut out = [0.0f64];
        for _ in 0..4096 {
            f.process(&[1.0, 0.0, 0.0], &mut out);
        }
        assert!(
            (out[0] - 1.0).abs() < 0.001,
            "DC should pass through lowpass; got {}",
            out[0]
        );
    }

    #[test]
    fn attenuates_above_cutoff() {
        // A second-order lowpass has −40 dB/decade rolloff. At 10× the cutoff
        // the signal should be attenuated to below 5% of the input amplitude.
        let sr = 44100.0;
        let mut f = make_filter_sr(1000.0, 0.0, sr);
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
        // High resonance should boost signals near the cutoff relative to the
        // Butterworth (resonance=0) case.
        let sr = 44100.0;
        let cutoff = 1000.0;
        let mut flat = make_filter_sr(cutoff, 0.0, sr);
        let mut resonant = make_filter_sr(cutoff, 0.8, sr);
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
        // cutoff_cv = +1.0 V/oct doubles the cutoff. A signal between the
        // original and doubled cutoff should be less attenuated with +1 V CV.
        let sr = 44100.0;
        let base_cutoff = 500.0;
        let test_freq = 800.0; // between 500 Hz and 1000 Hz

        let mut no_cv = make_filter_sr(base_cutoff, 0.0, sr);
        let mut with_cv = make_filter_sr(base_cutoff, 0.0, sr);

        // Tell with_cv that cutoff_cv is connected so it uses the CV path.
        with_cv.set_connectivity(PortConnectivity {
            inputs: vec![true, true, false].into_boxed_slice(),
            outputs: vec![true].into_boxed_slice(),
        });

        let mut no_cv_out = [0.0f64];
        let mut with_cv_out = [0.0f64];

        // Settle both filters; with_cv receives +1 V during settling.
        for _ in 0..4096 {
            no_cv.process(&[0.0, 0.0, 0.0], &mut no_cv_out);
            with_cv.process(&[0.0, 1.0, 0.0], &mut with_cv_out);
        }

        // Measure peak output for a sine at test_freq.
        let mut no_cv_peak = 0.0f64;
        let mut with_cv_peak = 0.0f64;
        for i in 0..4096usize {
            let x = (TAU * test_freq * i as f64 / sr).sin();
            no_cv.process(&[x, 0.0, 0.0], &mut no_cv_out);
            with_cv.process(&[x, 1.0, 0.0], &mut with_cv_out);
            no_cv_peak = no_cv_peak.max(no_cv_out[0].abs());
            with_cv_peak = with_cv_peak.max(with_cv_out[0].abs());
        }

        assert!(
            with_cv_peak > no_cv_peak * 1.5,
            "cutoff_cv +1 oct should raise cutoff and reduce attenuation at {test_freq} Hz; \
             no_cv={no_cv_peak:.4}, with_cv={with_cv_peak:.4}"
        );
    }

    #[test]
    fn static_path_passes_dc_when_no_cv() {
        // Explicit connectivity update declaring no CV inputs should keep the
        // filter on the static path and produce correct DC behaviour.
        let mut f = make_filter(1000.0, 0.0);
        f.set_connectivity(PortConnectivity {
            inputs: vec![true, false, false].into_boxed_slice(),
            outputs: vec![true].into_boxed_slice(),
        });
        let mut out = [0.0f64];
        for _ in 0..4096 {
            f.process(&[1.0, 0.0, 0.0], &mut out);
        }
        assert!(
            (out[0] - 1.0).abs() < 0.001,
            "DC should pass in static path; got {}",
            out[0]
        );
    }
}
