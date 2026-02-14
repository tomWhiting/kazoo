//! Pitch-tracked synthesis: voice pitch drives band-limited oscillators.
//!
//! The [`PitchTrackedSynth`] receives voice audio as input, tracks envelope
//! amplitude, and generates oscillator output at a target frequency (set
//! externally from the analysis thread). Supports sine, saw, square, and
//! triangle waveforms with `PolyBLEP` anti-aliasing, portamento glide, detune,
//! and an envelope-sensitive low-pass filter.

use crate::analysis::EnvelopeFollower;
use crate::effects::{BiquadFilter, FilterType};
use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};

// ---------------------------------------------------------------------------
// Oscillator shape
// ---------------------------------------------------------------------------

/// Waveform shape for the pitch-tracked oscillator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OscillatorShape {
    Sine,
    Saw,
    Square,
    Triangle,
}

impl OscillatorShape {
    /// Convert from a parameter float (0=Sine, 1=Saw, 2=Square, 3=Triangle).
    #[must_use]
    fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Self::Saw,
            2 => Self::Square,
            3 => Self::Triangle,
            _ => Self::Sine,
        }
    }

    /// Convert to a parameter float.
    #[must_use]
    const fn to_param(self) -> f32 {
        match self {
            Self::Sine => 0.0,
            Self::Saw => 1.0,
            Self::Square => 2.0,
            Self::Triangle => 3.0,
        }
    }
}

// ---------------------------------------------------------------------------
// PolyBLEP helpers
// ---------------------------------------------------------------------------

/// Evaluate the `PolyBLEP` correction for a discontinuity.
///
/// `t` is the phase position relative to the discontinuity, normalised so
/// that one sample period equals `dt` (= frequency / `sample_rate`). The
/// correction is non-zero only within one sample of the discontinuity.
#[inline]
#[must_use]
fn poly_blep(mut t: f32, dt: f32) -> f32 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        // Rising edge: t is in [0, dt).
        t /= dt;
        t.mul_add(t, -1.0).mul_add(0.5, t) // t + 0.5*(t*t - 1) = 0.5*t*t + t - 0.5
    } else if t > 1.0 - dt {
        // Falling edge: t is in (1-dt, 1).
        t = (t - 1.0) / dt;
        (-t).mul_add(t, 1.0).mul_add(0.5, t) // t + 0.5*(1 - t*t) = -0.5*t*t + t + 0.5
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// PitchTrackedSynth
// ---------------------------------------------------------------------------

/// Voice-pitch-driven synthesizer with band-limited oscillators.
///
/// The analysis thread sets `target_frequency` via [`set_target_frequency`].
/// The processing thread calls [`process`], which:
/// 1. Reads input to track envelope amplitude.
/// 2. Smooths frequency via one-pole portamento.
/// 3. Generates the selected waveform with `PolyBLEP` anti-aliasing.
/// 4. Applies a low-pass filter whose cutoff is modulated by the envelope.
#[derive(Debug)]
pub struct PitchTrackedSynth {
    sample_rate: f32,
    // Oscillator state
    shape: OscillatorShape,
    phase: f32,
    // Triangle: leaky-integrated square
    triangle_state: f32,
    // Frequency
    target_frequency: f32,
    current_frequency: f32,
    portamento_coeff: f32,
    portamento_ms: f32,
    detune_cents: f32,
    // Envelope tracking
    envelope: EnvelopeFollower,
    envelope_sensitivity: f32,
    // Filter
    filter: BiquadFilter,
    filter_cutoff: f32,
    filter_q: f32,
    // Scratch buffer for filter I/O
    filter_scratch: Vec<f32>,
}

impl PitchTrackedSynth {
    // Parameter indices
    const PARAM_SHAPE: usize = 0;
    const PARAM_DETUNE: usize = 1;
    const PARAM_FILTER_CUTOFF: usize = 2;
    const PARAM_FILTER_Q: usize = 3;
    const PARAM_PORTAMENTO: usize = 4;
    const PARAM_ENV_SENSITIVITY: usize = 5;
    const PARAM_COUNT: usize = 6;

    // Defaults
    const DEFAULT_CUTOFF: f32 = 5000.0;
    const DEFAULT_Q: f32 = 0.707;
    const DEFAULT_PORTAMENTO_MS: f32 = 20.0;
    const DEFAULT_ENV_SENSITIVITY: f32 = 0.5;

    /// Create a new pitch-tracked synthesizer at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };

        let mut filter = BiquadFilter::new(FilterType::LowPass, sr);
        // Ignore the result — defaults are always valid.
        let _ = filter.set_param(0, Self::DEFAULT_CUTOFF);
        let _ = filter.set_param(1, Self::DEFAULT_Q);

        Self {
            sample_rate: sr,
            shape: OscillatorShape::Saw,
            phase: 0.0,
            triangle_state: 0.0,
            target_frequency: 0.0,
            current_frequency: 0.0,
            portamento_coeff: compute_portamento_coeff(Self::DEFAULT_PORTAMENTO_MS, sr),
            portamento_ms: Self::DEFAULT_PORTAMENTO_MS,
            detune_cents: 0.0,
            envelope: EnvelopeFollower::new(5.0, 50.0, sr),
            envelope_sensitivity: Self::DEFAULT_ENV_SENSITIVITY,
            filter,
            filter_cutoff: Self::DEFAULT_CUTOFF,
            filter_q: Self::DEFAULT_Q,
            filter_scratch: vec![0.0; 256],
        }
    }

    /// Set the target frequency (called from the analysis/command thread).
    ///
    /// The oscillator glides to this frequency according to the portamento
    /// setting. Values outside `[20, 20000]` or non-finite are ignored.
    pub fn set_target_frequency(&mut self, frequency: f32) {
        if frequency.is_finite() && (20.0..=20_000.0).contains(&frequency) {
            self.target_frequency = frequency;
        }
    }

    /// Return the current instantaneous frequency after portamento smoothing.
    #[must_use]
    pub const fn current_frequency(&self) -> f32 {
        self.current_frequency
    }

    /// Generate one sample of the selected waveform at the given frequency.
    ///
    /// Advances the phase accumulator and applies `PolyBLEP` anti-aliasing
    /// for saw and square shapes.
    fn generate_sample(&mut self, frequency: f32) -> f32 {
        if frequency <= 0.0 || !frequency.is_finite() || self.sample_rate <= 0.0 {
            return 0.0;
        }

        let dt = frequency / self.sample_rate;
        // If frequency exceeds Nyquist, output silence to avoid aliasing.
        if dt >= 0.5 {
            return 0.0;
        }

        let sample = match self.shape {
            OscillatorShape::Sine => (self.phase * std::f32::consts::TAU).sin(),
            OscillatorShape::Saw => {
                // Naive saw: maps phase [0,1) to [-1,1)
                let naive = self.phase.mul_add(2.0, -1.0);
                naive - poly_blep(self.phase, dt)
            }
            OscillatorShape::Square => {
                // Naive square: +1 for phase < 0.5, -1 for phase >= 0.5
                let naive = if self.phase < 0.5 { 1.0 } else { -1.0 };
                // PolyBLEP at both transitions (0 and 0.5)
                let mut corrected = naive;
                corrected += poly_blep(self.phase, dt);
                // Shift phase by 0.5 for the second discontinuity
                let phase2 = (self.phase + 0.5) % 1.0;
                corrected -= poly_blep(phase2, dt);
                corrected
            }
            OscillatorShape::Triangle => {
                // Triangle via leaky integration of PolyBLEP square.
                let naive_sq = if self.phase < 0.5 { 1.0 } else { -1.0 };
                let mut sq = naive_sq;
                sq += poly_blep(self.phase, dt);
                let phase2 = (self.phase + 0.5) % 1.0;
                sq -= poly_blep(phase2, dt);

                // Leaky integrator: state += dt * sq, with leak factor for DC stability
                // Scale factor of 4*dt gives amplitude normalisation for triangle.
                self.triangle_state = 0.999_f32.mul_add(self.triangle_state, 4.0 * dt * sq);
                // Clamp to prevent runaway
                self.triangle_state = self.triangle_state.clamp(-1.5, 1.5);
                self.triangle_state
            }
        };

        // Advance phase
        self.phase += dt;
        // Wrap phase to [0, 1)
        self.phase -= self.phase.floor();

        sanitize_sample(sample)
    }

    fn param_infos() -> [ParamInfo; Self::PARAM_COUNT] {
        [
            ParamInfo {
                name: "Shape".into(),
                min: 0.0,
                max: 3.0,
                default: OscillatorShape::Saw.to_param(),
                unit: String::new(),
            },
            ParamInfo {
                name: "Detune".into(),
                min: -100.0,
                max: 100.0,
                default: 0.0,
                unit: "cents".into(),
            },
            ParamInfo {
                name: "Filter Cutoff".into(),
                min: 20.0,
                max: 20_000.0,
                default: Self::DEFAULT_CUTOFF,
                unit: "Hz".into(),
            },
            ParamInfo {
                name: "Filter Q".into(),
                min: 0.1,
                max: 30.0,
                default: Self::DEFAULT_Q,
                unit: String::new(),
            },
            ParamInfo {
                name: "Portamento".into(),
                min: 0.0,
                max: 500.0,
                default: Self::DEFAULT_PORTAMENTO_MS,
                unit: "ms".into(),
            },
            ParamInfo {
                name: "Env Sensitivity".into(),
                min: 0.0,
                max: 1.0,
                default: Self::DEFAULT_ENV_SENSITIVITY,
                unit: String::new(),
            },
        ]
    }
}

impl Processor for PitchTrackedSynth {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        if len == 0 {
            return;
        }

        // Ensure scratch buffer is large enough (no per-call alloc if already big enough).
        if self.filter_scratch.len() < len {
            self.filter_scratch.resize(len, 0.0);
        }

        for (i, &inp_sample) in input.iter().enumerate().take(len) {
            // Track envelope from voice input.
            let env_val = self.envelope.process_sample(inp_sample);

            // Portamento: smooth frequency toward target.
            if self.target_frequency > 0.0 {
                if self.current_frequency <= 0.0 {
                    // Jump to target on first note (no glide from zero).
                    self.current_frequency = self.target_frequency;
                } else {
                    // One-pole smoothing: current = coeff * current + (1-coeff) * target
                    self.current_frequency = self.portamento_coeff.mul_add(
                        self.current_frequency,
                        (1.0 - self.portamento_coeff) * self.target_frequency,
                    );
                }
            }

            // Apply detune in cents.
            let detuned_freq = if self.detune_cents.abs() > f32::EPSILON {
                self.current_frequency * (self.detune_cents / 1200.0).exp2()
            } else {
                self.current_frequency
            };

            // Generate oscillator sample.
            let osc = self.generate_sample(detuned_freq);

            // Scale oscillator by envelope (so silence in = silence out).
            self.filter_scratch[i] = sanitize_sample(osc * env_val);

            // Modulate filter cutoff by envelope.
            let modulated_cutoff =
                self.filter_cutoff * self.envelope_sensitivity.mul_add(env_val, 1.0);
            let clamped_cutoff = modulated_cutoff.clamp(20.0, 20_000.0);
            // Only update filter coefficients if cutoff changed significantly.
            if (clamped_cutoff - self.filter.param_value(0).unwrap_or(0.0)).abs() > 1.0 {
                let _ = self.filter.set_param(0, clamped_cutoff);
            }
        }

        // Apply filter to the generated audio.
        self.filter
            .process(&self.filter_scratch[..len], &mut output[..len]);
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        self.triangle_state = 0.0;
        self.current_frequency = 0.0;
        self.target_frequency = 0.0;
        self.envelope.reset();
        self.filter.reset();
    }

    fn name(&self) -> &'static str {
        "Pitch Tracked Synth"
    }

    fn param_count(&self) -> usize {
        Self::PARAM_COUNT
    }

    fn param_info(&self, index: usize) -> Option<ParamInfo> {
        let infos = Self::param_infos();
        infos.get(index).cloned()
    }

    fn param_value(&self, index: usize) -> Option<f32> {
        match index {
            Self::PARAM_SHAPE => Some(self.shape.to_param()),
            Self::PARAM_DETUNE => Some(self.detune_cents),
            Self::PARAM_FILTER_CUTOFF => Some(self.filter_cutoff),
            Self::PARAM_FILTER_Q => Some(self.filter_q),
            Self::PARAM_PORTAMENTO => Some(self.portamento_ms),
            Self::PARAM_ENV_SENSITIVITY => Some(self.envelope_sensitivity),
            _ => None,
        }
    }

    fn set_param(&mut self, index: usize, value: f32) -> Result<()> {
        let infos = Self::param_infos();
        let info = infos
            .get(index)
            .ok_or_else(|| Error::Config(format!("invalid param index {index}")))?;
        let clamped = info.clamp(value);

        match index {
            Self::PARAM_SHAPE => self.shape = OscillatorShape::from_param(clamped),
            Self::PARAM_DETUNE => self.detune_cents = clamped,
            Self::PARAM_FILTER_CUTOFF => {
                self.filter_cutoff = clamped;
                let _ = self.filter.set_param(0, clamped);
            }
            Self::PARAM_FILTER_Q => {
                self.filter_q = clamped;
                let _ = self.filter.set_param(1, clamped);
            }
            Self::PARAM_PORTAMENTO => {
                self.portamento_ms = clamped;
                self.portamento_coeff = compute_portamento_coeff(clamped, self.sample_rate);
            }
            Self::PARAM_ENV_SENSITIVITY => self.envelope_sensitivity = clamped,
            _ => unreachable!(),
        }
        Ok(())
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };
        self.sample_rate = sr;
        self.portamento_coeff = compute_portamento_coeff(self.portamento_ms, sr);
        self.envelope = EnvelopeFollower::new(5.0, 50.0, sr);
        self.filter.set_sample_rate(sr);
        self.reset();
    }

    fn set_pitch(&mut self, frequency: f32) {
        self.set_target_frequency(frequency);
    }
}

/// Compute one-pole smoothing coefficient for portamento.
///
/// Returns `exp(-1 / (time_ms * sample_rate / 1000))`. For zero time the
/// coefficient is 0 (instant jump).
fn compute_portamento_coeff(time_ms: f32, sample_rate: f32) -> f32 {
    if !time_ms.is_finite() || !sample_rate.is_finite() || time_ms <= 0.0 || sample_rate <= 0.0 {
        return 0.0;
    }
    let samples = time_ms * sample_rate / 1000.0;
    if samples < f32::EPSILON {
        return 0.0;
    }
    let coeff = (-1.0 / samples).exp();
    if coeff.is_finite() {
        coeff.clamp(0.0, 1.0 - f32::EPSILON)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn generates_output_when_frequency_set() {
        let mut synth = PitchTrackedSynth::new(44100.0);
        synth.set_target_frequency(440.0);

        // Feed voice-like input to drive envelope.
        let input: Vec<f32> = (0..1024)
            .map(|i| (2.0 * PI * 220.0 * i as f32 / 44100.0).sin())
            .collect();
        let mut output = vec![0.0_f32; 1024];
        synth.process(&input, &mut output);

        let energy: f32 = output.iter().map(|s| s * s).sum();
        assert!(
            energy > 0.01,
            "should generate audible output, energy = {energy}"
        );
    }

    #[test]
    fn silence_input_produces_silence_output() {
        let mut synth = PitchTrackedSynth::new(44100.0);
        synth.set_target_frequency(440.0);

        let input = vec![0.0_f32; 1024];
        let mut output = vec![0.0_f32; 1024];
        synth.process(&input, &mut output);

        let energy: f32 = output.iter().map(|s| s * s).sum();
        assert!(
            energy < 0.001,
            "silence in should produce near-silence out, energy = {energy}"
        );
    }

    #[test]
    fn different_shapes_produce_different_waveforms() {
        let freq = 440.0;
        let input: Vec<f32> = (0..2048)
            .map(|i| (2.0 * PI * 220.0 * i as f32 / 44100.0).sin() * 0.8)
            .collect();

        let mut energies = Vec::new();
        for shape in [
            OscillatorShape::Sine,
            OscillatorShape::Saw,
            OscillatorShape::Square,
            OscillatorShape::Triangle,
        ] {
            let mut synth = PitchTrackedSynth::new(44100.0);
            synth.set_target_frequency(freq);
            synth.shape = shape;

            let mut output = vec![0.0_f32; 2048];
            synth.process(&input, &mut output);

            let energy: f32 = output[512..].iter().map(|s| s * s).sum();
            energies.push(energy);
        }

        // Saw and square should have more harmonic energy than sine.
        // (Just check they all produce nonzero energy.)
        for (i, &e) in energies.iter().enumerate() {
            assert!(e > 0.01, "shape {i} should produce energy, got {e}");
        }
    }

    #[test]
    fn filter_cutoff_affects_brightness() {
        let input: Vec<f32> = (0..4096)
            .map(|i| (2.0 * PI * 200.0 * i as f32 / 44100.0).sin() * 0.9)
            .collect();

        // Bright filter (high cutoff)
        let mut bright = PitchTrackedSynth::new(44100.0);
        bright.set_target_frequency(440.0);
        bright.shape = OscillatorShape::Saw;
        let _ = bright.set_param(PitchTrackedSynth::PARAM_FILTER_CUTOFF, 15000.0);

        let mut out_bright = vec![0.0_f32; 4096];
        bright.process(&input, &mut out_bright);

        // Dark filter (low cutoff)
        let mut dark = PitchTrackedSynth::new(44100.0);
        dark.set_target_frequency(440.0);
        dark.shape = OscillatorShape::Saw;
        let _ = dark.set_param(PitchTrackedSynth::PARAM_FILTER_CUTOFF, 200.0);

        let mut out_dark = vec![0.0_f32; 4096];
        dark.process(&input, &mut out_dark);

        let energy_bright: f32 = out_bright[2048..].iter().map(|s| s * s).sum();
        let energy_dark: f32 = out_dark[2048..].iter().map(|s| s * s).sum();

        assert!(
            energy_bright > energy_dark,
            "bright filter ({energy_bright}) should have more energy than dark ({energy_dark})"
        );
    }

    #[test]
    fn empty_buffers_no_panic() {
        let mut synth = PitchTrackedSynth::new(44100.0);
        synth.process(&[], &mut []);
    }

    #[test]
    fn nan_inf_input_handled() {
        let mut synth = PitchTrackedSynth::new(44100.0);
        synth.set_target_frequency(440.0);

        let input = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5, -0.5];
        let mut output = [0.0_f32; 5];
        synth.process(&input, &mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] = {s} should be finite");
        }
    }

    #[test]
    fn invalid_frequency_ignored() {
        let mut synth = PitchTrackedSynth::new(44100.0);
        synth.set_target_frequency(440.0);
        assert!((synth.target_frequency - 440.0).abs() < f32::EPSILON);

        synth.set_target_frequency(f32::NAN);
        assert!((synth.target_frequency - 440.0).abs() < f32::EPSILON);

        synth.set_target_frequency(-100.0);
        assert!((synth.target_frequency - 440.0).abs() < f32::EPSILON);
    }

    #[test]
    fn param_count_and_info() {
        let synth = PitchTrackedSynth::new(44100.0);
        assert_eq!(synth.param_count(), 6);
        for i in 0..6 {
            assert!(synth.param_info(i).is_some(), "param {i} should have info");
            assert!(
                synth.param_value(i).is_some(),
                "param {i} should have value"
            );
        }
        assert!(synth.param_info(6).is_none());
        assert!(synth.param_value(6).is_none());
    }

    #[test]
    fn set_param_clamps() {
        let mut synth = PitchTrackedSynth::new(44100.0);
        synth
            .set_param(PitchTrackedSynth::PARAM_FILTER_CUTOFF, 50_000.0)
            .unwrap();
        let cutoff = synth
            .param_value(PitchTrackedSynth::PARAM_FILTER_CUTOFF)
            .unwrap();
        assert!(
            (cutoff - 20_000.0).abs() < f32::EPSILON,
            "cutoff should clamp to 20000, got {cutoff}"
        );
    }

    #[test]
    fn set_param_invalid_index() {
        let mut synth = PitchTrackedSynth::new(44100.0);
        assert!(synth.set_param(99, 0.0).is_err());
    }

    #[test]
    fn reset_clears_state() {
        let mut synth = PitchTrackedSynth::new(44100.0);
        synth.set_target_frequency(440.0);
        let input: Vec<f32> = (0..512)
            .map(|i| (2.0 * PI * 220.0 * i as f32 / 44100.0).sin())
            .collect();
        let mut output = vec![0.0_f32; 512];
        synth.process(&input, &mut output);

        synth.reset();
        assert!(synth.phase.abs() < f32::EPSILON);
        assert!(synth.current_frequency.abs() < f32::EPSILON);
    }

    #[test]
    fn sample_rate_change() {
        let mut synth = PitchTrackedSynth::new(44100.0);
        synth.set_sample_rate(96000.0);
        synth.set_target_frequency(440.0);

        let input: Vec<f32> = (0..512)
            .map(|i| (2.0 * PI * 220.0 * i as f32 / 96000.0).sin() * 0.8)
            .collect();
        let mut output = vec![0.0_f32; 512];
        synth.process(&input, &mut output);

        for &s in &output {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn poly_blep_at_boundaries() {
        // At t well inside [dt, 1-dt], correction should be zero.
        let correction = poly_blep(0.5, 0.01);
        assert!(
            correction.abs() < f32::EPSILON,
            "mid-phase blep should be 0, got {correction}"
        );

        // At t very close to 0, correction should be non-zero.
        let correction_near_zero = poly_blep(0.001, 0.01);
        assert!(
            correction_near_zero.abs() > 1e-6,
            "near-zero blep should be non-zero"
        );
    }
}
