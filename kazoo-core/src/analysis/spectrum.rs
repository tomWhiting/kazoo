//! FFT-based spectrum analyser.
//!
//! Computes magnitude spectra from incoming audio using `rustfft`. Includes a
//! Hann window, conversion to decibels, and optional exponential moving average
//! (EMA) smoothing between consecutive frames.

use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::sanitize_sample;

/// Magnitude spectrum output from the analyser.
#[derive(Debug, Clone)]
pub struct SpectrumData {
    /// Magnitude of each frequency bin in dB (length = `num_bins`).
    pub magnitudes_db: Vec<f32>,
    /// Centre frequency of each bin in Hz (length = `num_bins`).
    pub bin_frequencies: Vec<f32>,
    /// Number of bins (FFT size / 2 + 1).
    pub num_bins: usize,
}

/// Real-time FFT-based spectrum analyser.
///
/// Accumulates incoming audio into an internal buffer, applies a Hann window,
/// computes a forward FFT, converts the result to decibels, and optionally
/// smooths consecutive frames via an EMA filter.
pub struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    fft_size: usize,
    sample_rate: f32,
    window: Vec<f32>,
    input_buffer: Vec<f32>,
    buffer_pos: usize,
    complex_buffer: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    magnitudes: Vec<f32>,
    smoothing: f32,
    bin_frequencies: Vec<f32>,
}

impl std::fmt::Debug for SpectrumAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpectrumAnalyzer")
            .field("fft_size", &self.fft_size)
            .field("sample_rate", &self.sample_rate)
            .field("smoothing", &self.smoothing)
            .field("buffer_pos", &self.buffer_pos)
            .finish_non_exhaustive()
    }
}

impl SpectrumAnalyzer {
    /// Create a new spectrum analyser.
    ///
    /// * `fft_size` - The FFT size in samples. Must be at least 2. The number
    ///   of output bins is `fft_size / 2 + 1`.
    /// * `sample_rate` - Audio sample rate in Hz. Must be positive and finite.
    /// * `smoothing` - EMA coefficient in `[0.0, 1.0)`. `0.0` disables
    ///   smoothing; values closer to `1.0` produce a slower-responding display.
    #[must_use]
    pub fn new(fft_size: usize, sample_rate: f32, smoothing: f32) -> Self {
        let fft_size = fft_size.max(2);
        let safe_sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };
        let safe_smoothing = if smoothing.is_finite() {
            smoothing.clamp(0.0, 1.0 - f32::EPSILON)
        } else {
            0.0
        };

        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);
        let scratch_len = fft.get_inplace_scratch_len();

        let num_bins = fft_size / 2 + 1;

        // Pre-compute Hann window.
        let window: Vec<f32> = (0..fft_size)
            .map(|i| {
                let phase = 2.0 * std::f32::consts::PI * i as f32 / fft_size as f32;
                0.5 * (1.0 - phase.cos())
            })
            .collect();

        // Pre-compute bin centre frequencies.
        let bin_frequencies: Vec<f32> = (0..num_bins)
            .map(|i| i as f32 * safe_sr / fft_size as f32)
            .collect();

        Self {
            fft,
            fft_size,
            sample_rate: safe_sr,
            window,
            input_buffer: vec![0.0; fft_size],
            buffer_pos: 0,
            complex_buffer: vec![Complex::new(0.0, 0.0); fft_size],
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            magnitudes: vec![-100.0; num_bins],
            smoothing: safe_smoothing,
            bin_frequencies,
        }
    }

    /// Push audio samples into the analyser.
    ///
    /// Returns `Some(SpectrumData)` when a complete FFT frame has been
    /// accumulated and processed, or `None` if more samples are needed.
    /// NaN/Inf samples are sanitized to `0.0`.
    pub fn push_samples(&mut self, samples: &[f32]) -> Option<SpectrumData> {
        let mut result = None;

        for &s in samples {
            self.input_buffer[self.buffer_pos] = sanitize_sample(s);
            self.buffer_pos += 1;

            if self.buffer_pos >= self.fft_size {
                result = Some(self.compute_spectrum());
                self.buffer_pos = 0;
            }
        }

        result
    }

    /// Reset the analyser, clearing all buffers and smoothed magnitudes.
    pub fn reset(&mut self) {
        self.input_buffer.fill(0.0);
        self.buffer_pos = 0;
        self.magnitudes.fill(-100.0);
    }

    /// Return the FFT size.
    #[must_use]
    pub const fn fft_size(&self) -> usize {
        self.fft_size
    }

    /// Return the number of output bins.
    #[must_use]
    pub const fn num_bins(&self) -> usize {
        self.fft_size / 2 + 1
    }

    /// Return the sample rate.
    #[must_use]
    pub const fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Compute the magnitude spectrum from the current input buffer.
    fn compute_spectrum(&mut self) -> SpectrumData {
        let num_bins = self.fft_size / 2 + 1;

        // Apply Hann window and copy into the complex buffer.
        for i in 0..self.fft_size {
            let windowed = self.input_buffer[i] * self.window[i];
            self.complex_buffer[i] = Complex::new(sanitize_sample(windowed), 0.0);
        }

        // In-place forward FFT.
        self.fft
            .process_with_scratch(&mut self.complex_buffer, &mut self.scratch);

        // Compute magnitudes and convert to dB with EMA smoothing.
        let mut magnitudes_db = Vec::with_capacity(num_bins);
        for i in 0..num_bins {
            let c = self.complex_buffer[i];
            // Compute magnitude. Guard against non-finite FFT output.
            let re = if c.re.is_finite() { c.re } else { 0.0 };
            let im = if c.im.is_finite() { c.im } else { 0.0 };
            let mag = re.hypot(im) / self.fft_size as f32;

            // Convert to dB, avoiding log10(0).
            let db = 20.0 * (mag + 1e-10).log10();
            let db = if db.is_finite() { db } else { -100.0 };

            // EMA smoothing.
            let old = self.magnitudes[i];
            let smoothed = if old.is_finite() {
                self.smoothing.mul_add(old, (1.0 - self.smoothing) * db)
            } else {
                db
            };
            let smoothed = if smoothed.is_finite() {
                smoothed
            } else {
                -100.0
            };
            self.magnitudes[i] = smoothed;
            magnitudes_db.push(smoothed);
        }

        SpectrumData {
            magnitudes_db,
            bin_frequencies: self.bin_frequencies.clone(),
            num_bins,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn generate_sine(frequency: f32, sample_rate: f32, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate;
                (2.0 * PI * frequency * t).sin()
            })
            .collect()
    }

    #[test]
    fn sine_1khz_peak_at_correct_bin() {
        let fft_size = 2048;
        let sample_rate = 44100.0_f32;
        let mut analyzer = SpectrumAnalyzer::new(fft_size, sample_rate, 0.0);

        let samples = generate_sine(1000.0, sample_rate, fft_size * 2);
        let mut last_data = None;
        for chunk in samples.chunks(512) {
            if let Some(data) = analyzer.push_samples(chunk) {
                last_data = Some(data);
            }
        }

        let data = last_data.expect("should have produced spectrum data");
        assert_eq!(data.num_bins, fft_size / 2 + 1);
        assert_eq!(data.bin_frequencies.len(), data.num_bins);
        assert_eq!(data.magnitudes_db.len(), data.num_bins);

        // Find the bin with the highest magnitude.
        let peak_idx = data
            .magnitudes_db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;

        let peak_freq = data.bin_frequencies[peak_idx];
        let bin_width = sample_rate / fft_size as f32;

        // The peak should be within 1.5 bins of 1000 Hz.
        assert!(
            (peak_freq - 1000.0).abs() < bin_width * 1.5,
            "peak at {peak_freq} Hz, expected ~1000 Hz (bin width {bin_width})"
        );
    }

    #[test]
    fn dc_input_peak_at_bin_zero() {
        let fft_size = 1024;
        let sample_rate = 44100.0;
        let mut analyzer = SpectrumAnalyzer::new(fft_size, sample_rate, 0.0);

        let dc = vec![1.0_f32; fft_size];
        let data = analyzer.push_samples(&dc).expect("should produce data");

        // DC component should be in bin 0.
        let peak_idx = data
            .magnitudes_db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert_eq!(peak_idx, 0, "DC input should peak at bin 0");
    }

    #[test]
    fn smoothing_reduces_frame_variance() {
        let fft_size = 512;
        let sample_rate = 44100.0;

        // Without smoothing.
        let mut no_smooth = SpectrumAnalyzer::new(fft_size, sample_rate, 0.0);
        // With smoothing.
        let mut with_smooth = SpectrumAnalyzer::new(fft_size, sample_rate, 0.8);

        let sine = generate_sine(1000.0, sample_rate, fft_size);
        let silence = vec![0.0_f32; fft_size];

        // First frame: sine.
        let _ = no_smooth.push_samples(&sine);
        let _ = with_smooth.push_samples(&sine);

        // Second frame: silence. The smoothed version should still retain
        // some energy from the sine.
        let data_no = no_smooth.push_samples(&silence).unwrap();
        let data_sm = with_smooth.push_samples(&silence).unwrap();

        let max_no = data_no
            .magnitudes_db
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let max_sm = data_sm
            .magnitudes_db
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);

        // Smoothed version should have higher retained energy after silence.
        assert!(
            max_sm > max_no,
            "smoothed max {max_sm} should exceed non-smoothed {max_no} after transition to silence"
        );
    }

    #[test]
    fn reset_clears_state() {
        let fft_size = 512;
        let mut analyzer = SpectrumAnalyzer::new(fft_size, 44100.0, 0.5);

        let samples = generate_sine(440.0, 44100.0, fft_size);
        let _ = analyzer.push_samples(&samples);

        analyzer.reset();
        assert_eq!(analyzer.buffer_pos, 0);
    }

    #[test]
    fn nan_inf_input_handled() {
        let fft_size = 256;
        let mut analyzer = SpectrumAnalyzer::new(fft_size, 44100.0, 0.0);

        let mut samples = vec![f32::NAN; 128];
        samples.extend_from_slice(&vec![f32::INFINITY; 128]);

        let data = analyzer
            .push_samples(&samples)
            .expect("should produce data");
        for &db in &data.magnitudes_db {
            assert!(db.is_finite(), "all dB values should be finite, got {db}");
        }
    }

    #[test]
    fn bin_frequencies_correct() {
        let fft_size = 1024;
        let sample_rate = 48000.0;
        let analyzer = SpectrumAnalyzer::new(fft_size, sample_rate, 0.0);

        let bin_width = sample_rate / fft_size as f32;
        assert!(
            (analyzer.bin_frequencies[0]).abs() < f32::EPSILON,
            "bin 0 should be at 0 Hz"
        );
        assert!(
            (analyzer.bin_frequencies[1] - bin_width).abs() < 0.01,
            "bin 1 should be at {bin_width} Hz"
        );

        let last_bin = analyzer.num_bins() - 1;
        let expected_nyquist = last_bin as f32 * bin_width;
        assert!(
            (analyzer.bin_frequencies[last_bin] - expected_nyquist).abs() < 0.01,
            "last bin should be at Nyquist"
        );
    }

    #[test]
    fn empty_input_returns_none() {
        let mut analyzer = SpectrumAnalyzer::new(512, 44100.0, 0.0);
        assert!(analyzer.push_samples(&[]).is_none());
    }

    #[test]
    fn bad_constructor_params() {
        // Zero fft_size should be clamped to 2.
        let a = SpectrumAnalyzer::new(0, 44100.0, 0.0);
        assert!(a.fft_size() >= 2);

        // NaN sample rate should default.
        let b = SpectrumAnalyzer::new(512, f32::NAN, 0.0);
        assert!(b.sample_rate() > 0.0);

        // NaN smoothing should default to 0.
        let _ = SpectrumAnalyzer::new(512, 44100.0, f32::NAN);
    }
}
