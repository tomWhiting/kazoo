//! Phase vocoder: STFT-based time stretching and pitch shifting.
//!
//! The [`PhaseVocoder`] uses a forward FFT to convert input audio into the
//! frequency domain, manipulates the magnitude/phase representation for time
//! stretching and pitch shifting, then reconstructs the output via inverse FFT
//! and overlap-add.

use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::{Error, ParamInfo, Processor, Result, sanitize_sample};

/// Default FFT size for the phase vocoder.
const FFT_SIZE: usize = 2048;

/// Overlap factor (4x overlap = hop size of `FFT_SIZE` / 4).
const OVERLAP_FACTOR: usize = 4;

// ---------------------------------------------------------------------------
// PhaseVocoder
// ---------------------------------------------------------------------------

/// STFT-based time stretcher and pitch shifter.
///
/// Parameters:
/// 0. `time_stretch` — stretch factor (0.25–4.0; 1.0 = no change)
/// 1. `pitch_shift` — semitone shift (−24 to +24)
pub struct PhaseVocoder {
    sample_rate: f32,
    fft_size: usize,
    hop_analysis: usize,

    // FFT plans.
    fft_forward: Arc<dyn Fft<f32>>,
    fft_inverse: Arc<dyn Fft<f32>>,

    // Pre-allocated buffers.
    analysis_window: Vec<f32>,
    input_buffer: Vec<f32>,
    input_write_pos: usize,
    output_buffer: Vec<f32>,
    output_read_pos: usize,
    complex_buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,

    // Phase accumulation.
    last_phase: Vec<f32>,
    sum_phase: Vec<f32>,

    // Temporary buffers for pitch-shifting bin remapping.
    magnitude_buf: Vec<f32>,
    frequency_buf: Vec<f32>,

    // Hop counters.
    input_hop_counter: usize,

    // Parameters.
    time_stretch: f32,
    pitch_shift: f32,
}

impl std::fmt::Debug for PhaseVocoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhaseVocoder")
            .field("fft_size", &self.fft_size)
            .field("sample_rate", &self.sample_rate)
            .field("time_stretch", &self.time_stretch)
            .field("pitch_shift", &self.pitch_shift)
            .finish_non_exhaustive()
    }
}

impl PhaseVocoder {
    const PARAM_TIME_STRETCH: usize = 0;
    const PARAM_PITCH_SHIFT: usize = 1;
    const PARAM_COUNT: usize = 2;

    /// Create a new phase vocoder at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };

        let fft_size = FFT_SIZE;
        let hop_analysis = fft_size / OVERLAP_FACTOR;

        let mut planner = FftPlanner::new();
        let fft_forward = planner.plan_fft_forward(fft_size);
        let fft_inverse = planner.plan_fft_inverse(fft_size);

        let scratch_len = fft_forward
            .get_inplace_scratch_len()
            .max(fft_inverse.get_inplace_scratch_len());

        // Hann analysis window.
        let analysis_window: Vec<f32> = (0..fft_size)
            .map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / fft_size as f32;
                0.5 * (1.0 - phase.cos())
            })
            .collect();

        // Output buffer needs to be large enough for overlap-add at maximum
        // stretch. We use 4x the FFT size as a safe margin.
        let output_buf_len = fft_size * 4;

        Self {
            sample_rate: sr,
            fft_size,
            hop_analysis,
            fft_forward,
            fft_inverse,
            analysis_window,
            input_buffer: vec![0.0; fft_size],
            input_write_pos: 0,
            output_buffer: vec![0.0; output_buf_len],
            output_read_pos: 0,
            complex_buf: vec![Complex::new(0.0, 0.0); fft_size],
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            last_phase: vec![0.0; fft_size],
            sum_phase: vec![0.0; fft_size],
            magnitude_buf: vec![0.0; fft_size],
            frequency_buf: vec![0.0; fft_size],
            input_hop_counter: 0,
            time_stretch: 1.0,
            pitch_shift: 0.0,
        }
    }

    /// Perform one STFT analysis-modification-synthesis frame.
    fn process_frame(&mut self) {
        let n = self.fft_size;
        let hop_a = self.hop_analysis;
        let hop_s = self.synthesis_hop();

        // Apply analysis window and load into complex buffer.
        for i in 0..n {
            let idx = (self.input_write_pos + i) % n;
            let windowed = self.input_buffer[idx] * self.analysis_window[i];
            self.complex_buf[i] = Complex::new(sanitize_sample(windowed), 0.0);
        }

        // Forward FFT.
        self.fft_forward
            .process_with_scratch(&mut self.complex_buf, &mut self.scratch);

        // --- Pass 1: Decompose into magnitude and true-frequency per bin ---
        let expected_phase_diff = std::f32::consts::TAU * hop_a as f32 / n as f32;

        for k in 0..n {
            let c = self.complex_buf[k];
            let re = if c.re.is_finite() { c.re } else { 0.0 };
            let im = if c.im.is_finite() { c.im } else { 0.0 };

            let magnitude = re.hypot(im);
            let phase = im.atan2(re);

            // Phase deviation from expected hop-to-hop advance for bin k.
            let expected = expected_phase_diff * k as f32;
            let phase_diff = wrap_phase(phase - self.last_phase[k] - expected);

            // True frequency as instantaneous phase increment per sample,
            // stored as "frequency deviation" (includes the expected term).
            self.magnitude_buf[k] = magnitude;
            self.frequency_buf[k] = phase_diff + expected;
            self.last_phase[k] = phase;
        }

        // --- Pass 2: Pitch-shift via bin remapping, then accumulate phase ---
        let pitch_ratio = (self.pitch_shift / 12.0).exp2();
        let time_scale = hop_s as f32 / hop_a as f32;

        // When pitch_ratio != 1.0, we remap: output bin k reads from source
        // bin k / pitch_ratio (with linear interpolation of magnitude) and
        // scales the frequency by pitch_ratio.
        //
        // When pitch_ratio == 1.0, this reduces to the identity remapping.
        for k in 0..n {
            let source_bin = k as f32 / pitch_ratio;
            let source_idx = source_bin.floor() as usize;

            let (mag, freq_dev) = if source_idx + 1 < n {
                let frac = source_bin - source_idx as f32;
                let m0 = self.magnitude_buf[source_idx];
                let m1 = self.magnitude_buf[source_idx + 1];
                let f0 = self.frequency_buf[source_idx];
                let f1 = self.frequency_buf[source_idx + 1];
                (
                    frac.mul_add(m1 - m0, m0),
                    frac.mul_add(f1 - f0, f0) * pitch_ratio,
                )
            } else if source_idx < n {
                (
                    self.magnitude_buf[source_idx],
                    self.frequency_buf[source_idx] * pitch_ratio,
                )
            } else {
                // Source bin beyond Nyquist — contribute silence.
                (0.0, 0.0)
            };

            // Accumulate phase with time-stretch scaling.
            self.sum_phase[k] += freq_dev * time_scale;
            // Wrap phase to (-pi, pi] to prevent unbounded growth and
            // resulting f32 precision loss in cos/sin after long playback.
            self.sum_phase[k] = wrap_phase(self.sum_phase[k]);

            let new_phase = self.sum_phase[k];
            self.complex_buf[k] = Complex::new(mag * new_phase.cos(), mag * new_phase.sin());
        }

        // Inverse FFT.
        self.fft_inverse
            .process_with_scratch(&mut self.complex_buf, &mut self.scratch);

        // Overlap-add into output buffer with synthesis window.
        let out_len = self.output_buffer.len();
        let inv_n = 1.0 / n as f32;
        for i in 0..n {
            let pos = (self.output_read_pos + i) % out_len;
            let windowed = self.complex_buf[i].re * inv_n * self.analysis_window[i];
            self.output_buffer[pos] += sanitize_sample(windowed);
        }
    }

    /// Compute the synthesis hop size based on time stretch.
    fn synthesis_hop(&self) -> usize {
        let hop = (self.hop_analysis as f32 * self.time_stretch).round() as usize;
        hop.max(1)
    }

    fn param_infos() -> [ParamInfo; Self::PARAM_COUNT] {
        [
            ParamInfo {
                name: "Time Stretch".into(),
                min: 0.25,
                max: 4.0,
                default: 1.0,
                unit: "x".into(),
            },
            ParamInfo {
                name: "Pitch Shift".into(),
                min: -24.0,
                max: 24.0,
                default: 0.0,
                unit: "st".into(),
            },
        ]
    }
}

/// Wrap a phase angle to `(-pi, pi]`.
#[inline]
#[must_use]
fn wrap_phase(phase: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let pi = std::f32::consts::PI;
    // Use rem_euclid to get a value in [0, TAU), then shift to (-pi, pi].
    let p = (phase + pi).rem_euclid(tau);
    p - pi
}

impl Processor for PhaseVocoder {
    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        if output.is_empty() {
            return;
        }

        let n = self.fft_size;
        let hop_a = self.hop_analysis;
        let hop_s = self.synthesis_hop();
        let out_buf_len = self.output_buffer.len();
        let input_len = input.len();

        // Feed input samples and process frames as needed.
        let mut input_idx = 0;
        let mut output_idx = 0;

        while output_idx < output.len() {
            // Consume input samples into the analysis buffer.
            while input_idx < input_len && self.input_hop_counter < hop_a {
                let pos = self.input_write_pos;
                self.input_buffer[pos] = sanitize_sample(input[input_idx]);
                self.input_write_pos = (pos + 1) % n;
                self.input_hop_counter += 1;
                input_idx += 1;
            }

            // If we do not have enough input for a hop yet, pad with zeros.
            if self.input_hop_counter < hop_a && input_idx >= input_len {
                while self.input_hop_counter < hop_a {
                    let pos = self.input_write_pos;
                    self.input_buffer[pos] = 0.0;
                    self.input_write_pos = (pos + 1) % n;
                    self.input_hop_counter += 1;
                }
            }

            // Process a frame when we have accumulated one hop of input.
            if self.input_hop_counter >= hop_a {
                self.process_frame();
                self.input_hop_counter = 0;

                // Read synthesis hop samples from the output buffer.
                let samples_to_read = hop_s.min(output.len() - output_idx);
                for _ in 0..samples_to_read {
                    let pos = self.output_read_pos;
                    output[output_idx] = sanitize_sample(self.output_buffer[pos]);
                    self.output_buffer[pos] = 0.0; // clear for next overlap
                    self.output_read_pos = (pos + 1) % out_buf_len;
                    output_idx += 1;
                }

                // If hop_s > samples_to_read, advance read pointer.
                let remaining = hop_s.saturating_sub(samples_to_read);
                for _ in 0..remaining {
                    let pos = self.output_read_pos;
                    self.output_buffer[pos] = 0.0;
                    self.output_read_pos = (pos + 1) % out_buf_len;
                }
            }

            // Safety: if we have consumed all input and filled all output.
            if input_idx >= input_len && self.input_hop_counter < hop_a {
                // Fill remaining output with whatever is in the overlap buffer.
                while output_idx < output.len() {
                    let pos = self.output_read_pos;
                    output[output_idx] = sanitize_sample(self.output_buffer[pos]);
                    self.output_buffer[pos] = 0.0;
                    self.output_read_pos = (pos + 1) % out_buf_len;
                    output_idx += 1;
                }
            }
        }
    }

    fn reset(&mut self) {
        self.input_buffer.fill(0.0);
        self.input_write_pos = 0;
        self.output_buffer.fill(0.0);
        self.output_read_pos = 0;
        self.last_phase.fill(0.0);
        self.sum_phase.fill(0.0);
        self.magnitude_buf.fill(0.0);
        self.frequency_buf.fill(0.0);
        self.input_hop_counter = 0;
    }

    fn latency_samples(&self) -> usize {
        self.fft_size
    }

    fn name(&self) -> &'static str {
        "Phase Vocoder"
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
            Self::PARAM_TIME_STRETCH => Some(self.time_stretch),
            Self::PARAM_PITCH_SHIFT => Some(self.pitch_shift),
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
            Self::PARAM_TIME_STRETCH => self.time_stretch = clamped,
            Self::PARAM_PITCH_SHIFT => {
                self.pitch_shift = clamped;
                // Clear phase accumulators to avoid audible discontinuities
                // from stale state when pitch changes.
                self.sum_phase.fill(0.0);
                self.last_phase.fill(0.0);
            }
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
        self.reset();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn sine_wave(freq: f32, sr: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| (2.0 * PI * freq * i as f32 / sr).sin())
            .collect()
    }

    #[test]
    fn passes_through_at_unity_settings() {
        let sr = 44100.0;
        let mut pv = PhaseVocoder::new(sr);
        // Default: time_stretch = 1.0, pitch_shift = 0.0.

        let input = sine_wave(440.0, sr, 8192);
        let mut output = vec![0.0_f32; 8192];
        pv.process(&input, &mut output);

        // At unity settings, the output should resemble the input
        // (after the initial latency window). Check that energy is present.
        let latency = pv.latency_samples();
        let tail = &output[latency..];
        let energy: f32 = tail.iter().map(|s| s * s).sum();
        assert!(
            energy > 1.0,
            "unity passthrough should have significant energy, got {energy}"
        );

        // All samples finite.
        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] = {s}");
        }
    }

    #[test]
    fn empty_buffers_no_panic() {
        let mut pv = PhaseVocoder::new(44100.0);
        pv.process(&[], &mut []);
    }

    #[test]
    fn nan_input_handled() {
        let mut pv = PhaseVocoder::new(44100.0);
        let input = vec![f32::NAN; 4096];
        let mut output = vec![0.0_f32; 4096];
        pv.process(&input, &mut output);
        for (i, &s) in output.iter().enumerate() {
            assert!(s.is_finite(), "output[{i}] = {s}");
        }
    }

    #[test]
    fn param_count_and_info() {
        let pv = PhaseVocoder::new(44100.0);
        assert_eq!(pv.param_count(), 2);
        for i in 0..2 {
            assert!(pv.param_info(i).is_some());
            assert!(pv.param_value(i).is_some());
        }
        assert!(pv.param_info(2).is_none());
        assert!(pv.param_value(2).is_none());
    }

    #[test]
    fn set_param_clamps() {
        let mut pv = PhaseVocoder::new(44100.0);
        pv.set_param(PhaseVocoder::PARAM_TIME_STRETCH, 10.0)
            .unwrap();
        let v = pv.param_value(PhaseVocoder::PARAM_TIME_STRETCH).unwrap();
        assert!(
            (v - 4.0).abs() < f32::EPSILON,
            "should clamp to 4.0, got {v}"
        );
    }

    #[test]
    fn set_param_invalid_index() {
        let mut pv = PhaseVocoder::new(44100.0);
        assert!(pv.set_param(99, 0.0).is_err());
    }

    #[test]
    fn reset_clears_state() {
        let mut pv = PhaseVocoder::new(44100.0);
        let input = sine_wave(440.0, 44100.0, 4096);
        let mut output = vec![0.0_f32; 4096];
        pv.process(&input, &mut output);

        pv.reset();
        assert_eq!(pv.input_write_pos, 0);
        assert_eq!(pv.output_read_pos, 0);
        assert_eq!(pv.input_hop_counter, 0);
    }

    #[test]
    fn sample_rate_change() {
        let mut pv = PhaseVocoder::new(44100.0);
        pv.set_sample_rate(96000.0);
        let input = sine_wave(440.0, 96000.0, 4096);
        let mut output = vec![0.0_f32; 4096];
        pv.process(&input, &mut output);
        for &s in &output {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn time_stretch_changes_output() {
        let sr = 44100.0;
        let input = sine_wave(440.0, sr, 8192);

        let mut pv1 = PhaseVocoder::new(sr);
        pv1.set_param(PhaseVocoder::PARAM_TIME_STRETCH, 1.0)
            .unwrap();
        let mut out1 = vec![0.0_f32; 8192];
        pv1.process(&input, &mut out1);

        let mut pv2 = PhaseVocoder::new(sr);
        pv2.set_param(PhaseVocoder::PARAM_TIME_STRETCH, 2.0)
            .unwrap();
        let mut out2 = vec![0.0_f32; 8192];
        pv2.process(&input, &mut out2);

        let diff: f32 = out1
            .iter()
            .zip(out2.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(
            diff > 1.0,
            "different stretch factors should produce different output, diff = {diff}"
        );
    }

    #[test]
    fn pitch_shift_changes_output() {
        let sr = 44100.0;
        let input = sine_wave(440.0, sr, 8192);

        let mut pv1 = PhaseVocoder::new(sr);
        pv1.set_param(PhaseVocoder::PARAM_PITCH_SHIFT, 0.0).unwrap();
        let mut out1 = vec![0.0_f32; 8192];
        pv1.process(&input, &mut out1);

        let mut pv2 = PhaseVocoder::new(sr);
        pv2.set_param(PhaseVocoder::PARAM_PITCH_SHIFT, 12.0)
            .unwrap();
        let mut out2 = vec![0.0_f32; 8192];
        pv2.process(&input, &mut out2);

        let diff: f32 = out1
            .iter()
            .zip(out2.iter())
            .map(|(a, b)| (a - b).abs())
            .sum();
        assert!(
            diff > 1.0,
            "different pitch shifts should produce different output, diff = {diff}"
        );

        // Verify all outputs are finite.
        for &s in &out2 {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn wrap_phase_works() {
        let pi = std::f32::consts::PI;
        assert!((wrap_phase(0.0)).abs() < 1e-6);
        // pi and -pi are equivalent phase angles.
        let w_pi = wrap_phase(pi);
        assert!(
            (w_pi - pi).abs() < 1e-6 || (w_pi + pi).abs() < 1e-6,
            "wrap_phase(pi) should be +/-pi, got {w_pi}"
        );
        let w_neg_pi = wrap_phase(-pi);
        assert!(
            (w_neg_pi - pi).abs() < 1e-6 || (w_neg_pi + pi).abs() < 1e-6,
            "wrap_phase(-pi) should be +/-pi, got {w_neg_pi}"
        );
        // 3*pi should wrap to approximately +/-pi.
        let wrapped = wrap_phase(3.0 * pi);
        assert!(
            (wrapped - pi).abs() < 1e-4 || (wrapped + pi).abs() < 1e-4,
            "3*pi should wrap near +/-pi, got {wrapped}"
        );
    }

    #[test]
    fn wrap_phase_large_multiples() {
        let pi = std::f32::consts::PI;
        // Large positive multiple of pi.
        let wrapped = wrap_phase(10.0 * pi);
        assert!(
            wrapped.abs() < pi + 0.01,
            "10*pi should wrap to [-pi, pi], got {wrapped}"
        );

        // Large negative multiple.
        let wrapped_neg = wrap_phase(-10.0 * pi);
        assert!(
            wrapped_neg.abs() < pi + 0.01,
            "-10*pi should wrap to [-pi, pi], got {wrapped_neg}"
        );

        // Very small near-zero.
        let tiny = wrap_phase(1e-7);
        assert!(
            (tiny - 1e-7).abs() < 1e-6,
            "tiny value should pass through: {tiny}"
        );
    }

    #[test]
    fn stretch_extremes_produce_finite_output() {
        let sr = 44100.0;
        let input = sine_wave(440.0, sr, 8192);

        for stretch in [0.25, 0.5, 2.0, 4.0] {
            let mut pv = PhaseVocoder::new(sr);
            pv.set_param(PhaseVocoder::PARAM_TIME_STRETCH, stretch)
                .unwrap();

            let mut output = vec![0.0_f32; 8192];
            pv.process(&input, &mut output);

            for (i, &s) in output.iter().enumerate() {
                assert!(
                    s.is_finite(),
                    "stretch={stretch}: output[{i}] = {s} not finite"
                );
            }
        }
    }

    #[test]
    fn pitch_shift_extremes_produce_finite_output() {
        let sr = 44100.0;
        let input = sine_wave(440.0, sr, 8192);

        for shift in [-24.0, -12.0, 12.0, 24.0] {
            let mut pv = PhaseVocoder::new(sr);
            pv.set_param(PhaseVocoder::PARAM_PITCH_SHIFT, shift)
                .unwrap();

            let mut output = vec![0.0_f32; 8192];
            pv.process(&input, &mut output);

            for (i, &s) in output.iter().enumerate() {
                assert!(s.is_finite(), "shift={shift}: output[{i}] = {s} not finite");
            }
        }
    }

    #[test]
    fn short_input_does_not_panic() {
        let mut pv = PhaseVocoder::new(44100.0);
        // Input shorter than hop size (FFT_SIZE/4 = 512).
        let input = [0.5_f32; 64];
        let mut output = [0.0_f32; 64];
        pv.process(&input, &mut output);

        for &s in &output {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn stability_with_noise() {
        let mut pv = PhaseVocoder::new(44100.0);
        pv.set_param(PhaseVocoder::PARAM_TIME_STRETCH, 1.5).unwrap();
        pv.set_param(PhaseVocoder::PARAM_PITCH_SHIFT, 5.0).unwrap();

        let mut rng: u32 = 0xCAFE_BABE;
        let noise: Vec<f32> = (0..8192)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                (rng as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect();
        let mut output = vec![0.0_f32; 8192];
        pv.process(&noise, &mut output);

        for (i, &s) in output.iter().enumerate() {
            assert!(
                s.is_finite() && s.abs() < 100.0,
                "noise stability: output[{i}] = {s}"
            );
        }
    }

    #[test]
    fn param_info_names_not_empty() {
        let pv = PhaseVocoder::new(44100.0);
        for i in 0..pv.param_count() {
            let info = pv.param_info(i).unwrap();
            assert!(!info.name.is_empty(), "param {i} has empty name");
        }
    }

    #[test]
    fn param_values_roundtrip() {
        let mut pv = PhaseVocoder::new(44100.0);
        pv.set_param(PhaseVocoder::PARAM_TIME_STRETCH, 2.0).unwrap();
        pv.set_param(PhaseVocoder::PARAM_PITCH_SHIFT, -7.0).unwrap();

        assert!(
            (pv.param_value(PhaseVocoder::PARAM_TIME_STRETCH).unwrap() - 2.0).abs() < f32::EPSILON
        );
        assert!(
            (pv.param_value(PhaseVocoder::PARAM_PITCH_SHIFT).unwrap() - (-7.0)).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn name_is_not_empty() {
        let pv = PhaseVocoder::new(44100.0);
        assert!(!pv.name().is_empty());
    }

    #[test]
    fn long_sustained_processing() {
        let sr = 44100.0;
        let mut pv = PhaseVocoder::new(sr);
        pv.set_param(PhaseVocoder::PARAM_TIME_STRETCH, 1.5).unwrap();

        let input: Vec<f32> = (0..1024)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sr).sin())
            .collect();
        let mut output = vec![0.0_f32; 1024];

        for _ in 0..20 {
            pv.process(&input, &mut output);
            for &s in &output {
                assert!(s.is_finite() && s.abs() < 100.0);
            }
        }
    }
}
