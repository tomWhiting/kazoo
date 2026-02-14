//! Onset detection using spectral flux.
//!
//! Computes the positive half-wave rectified difference between consecutive
//! magnitude spectra to detect transient events (note onsets, percussive hits,
//! etc.) in the audio stream. Uses a running median of spectral flux values to
//! adapt the threshold, so only sudden increases relative to the local context
//! trigger an onset.

use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::sanitize_sample;

/// A detected onset event within a block of audio.
#[derive(Debug, Clone, Copy)]
pub struct OnsetEvent {
    /// Offset in samples from the start of the block where the onset occurred.
    pub sample_offset: usize,
    /// Onset strength normalised to `[0.0, 1.0]`.
    pub strength: f32,
    /// Spectral centroid at the onset frame, in Hz. Indicates brightness.
    pub spectral_centroid: f32,
}

/// Length of the running history buffer used for adaptive thresholding.
const HISTORY_LEN: usize = 16;

/// Spectral-flux onset detector.
///
/// Accumulates audio in hop-sized chunks, computes FFTs, measures spectral flux
/// (the sum of positive magnitude differences between consecutive frames), and
/// fires onset events when the flux exceeds a multiple of the running median.
pub struct OnsetDetector {
    fft: Arc<dyn Fft<f32>>,
    fft_size: usize,
    hop_size: usize,
    sample_rate: f32,
    /// Multiplicative threshold factor. An onset fires when
    /// `flux > (1 + threshold_factor) * median(recent_flux) + absolute_floor`.
    threshold_factor: f32,
    prev_magnitudes: Vec<f32>,
    input_buffer: Vec<f32>,
    buffer_pos: usize,
    complex_buffer: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    window: Vec<f32>,
    /// Running sample offset within the current call to `push_samples`.
    total_samples_pushed: usize,
    /// Whether we have a previous frame to compare against.
    has_prev: bool,
    /// Circular buffer of recent flux values for adaptive thresholding.
    flux_history: Vec<f32>,
    /// Write position in `flux_history`.
    flux_history_pos: usize,
    /// Number of flux values written so far (up to `HISTORY_LEN`).
    flux_history_count: usize,
    /// Peak flux ever observed, used for normalizing strength to [0, 1].
    peak_flux: f32,
}

impl std::fmt::Debug for OnsetDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnsetDetector")
            .field("fft_size", &self.fft_size)
            .field("hop_size", &self.hop_size)
            .field("threshold_factor", &self.threshold_factor)
            .field("buffer_pos", &self.buffer_pos)
            .finish_non_exhaustive()
    }
}

impl OnsetDetector {
    /// Create a new onset detector.
    ///
    /// * `fft_size` - FFT frame size in samples (minimum 4).
    /// * `hop_size` - Number of samples between consecutive analysis frames
    ///   (minimum 1, typically `fft_size / 4`).
    /// * `sample_rate` - Audio sample rate in Hz.
    /// * `threshold` - Sensitivity control in `[0.0, 1.0]`. Lower values mean
    ///   more sensitive (more onsets detected). Internally converted to a
    ///   multiplicative factor applied to the running median of flux values.
    #[must_use]
    pub fn new(fft_size: usize, hop_size: usize, sample_rate: f32, threshold: f32) -> Self {
        let fft_size = fft_size.max(4);
        let hop_size = hop_size.max(1).min(fft_size);
        let safe_sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };
        let safe_threshold = if threshold.is_finite() {
            threshold.clamp(0.0, 1.0)
        } else {
            0.3
        };

        // Map the [0, 1] threshold parameter to a multiplicative factor.
        // threshold=0 -> factor=1.5 (very sensitive)
        // threshold=1 -> factor=10  (very selective)
        let threshold_factor = 1.5 + safe_threshold * 8.5;

        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);
        let scratch_len = fft.get_inplace_scratch_len();

        let num_bins = fft_size / 2 + 1;

        // Hann window.
        let window: Vec<f32> = (0..fft_size)
            .map(|i| {
                let phase = 2.0 * std::f32::consts::PI * i as f32 / fft_size as f32;
                0.5 * (1.0 - phase.cos())
            })
            .collect();

        Self {
            fft,
            fft_size,
            hop_size,
            sample_rate: safe_sr,
            threshold_factor,
            prev_magnitudes: vec![0.0; num_bins],
            input_buffer: vec![0.0; fft_size],
            buffer_pos: 0,
            complex_buffer: vec![Complex::new(0.0, 0.0); fft_size],
            scratch: vec![Complex::new(0.0, 0.0); scratch_len],
            window,
            total_samples_pushed: 0,
            has_prev: false,
            flux_history: vec![0.0; HISTORY_LEN],
            flux_history_pos: 0,
            flux_history_count: 0,
            peak_flux: f32::EPSILON,
        }
    }

    /// Push audio samples and return any detected onset events.
    ///
    /// The returned vector may be empty if no onsets were detected in this
    /// chunk. Each `OnsetEvent`'s `sample_offset` is relative to the start of
    /// the `samples` slice passed to this call.
    pub fn push_samples(&mut self, samples: &[f32]) -> Vec<OnsetEvent> {
        let mut events = Vec::new();
        self.total_samples_pushed = 0;

        for &s in samples {
            self.input_buffer[self.buffer_pos] = sanitize_sample(s);
            self.buffer_pos += 1;
            self.total_samples_pushed += 1;

            if self.buffer_pos >= self.fft_size {
                // We have a full frame. Analyse it.
                if let Some(event) = self.analyze_frame(self.total_samples_pushed) {
                    events.push(event);
                }
                // Shift buffer by hop_size (overlap-save).
                let keep = self.fft_size - self.hop_size;
                self.input_buffer.copy_within(self.hop_size.., 0);
                // Zero out the freed portion.
                for sample in &mut self.input_buffer[keep..] {
                    *sample = 0.0;
                }
                self.buffer_pos = keep;
            }
        }

        events
    }

    /// Reset the detector, clearing all internal state.
    pub fn reset(&mut self) {
        self.input_buffer.fill(0.0);
        self.buffer_pos = 0;
        self.prev_magnitudes.fill(0.0);
        self.has_prev = false;
        self.total_samples_pushed = 0;
        self.flux_history.fill(0.0);
        self.flux_history_pos = 0;
        self.flux_history_count = 0;
        self.peak_flux = f32::EPSILON;
    }

    /// Compute the median of the recent flux history.
    fn flux_median(&self) -> f32 {
        let count = self.flux_history_count.min(HISTORY_LEN);
        if count == 0 {
            return 0.0;
        }
        let mut sorted: Vec<f32> = self.flux_history[..count].to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = count / 2;
        if count % 2 == 0 && count >= 2 {
            f32::midpoint(sorted[mid - 1], sorted[mid])
        } else {
            sorted[mid]
        }
    }

    /// Record a flux value into the running history.
    fn record_flux(&mut self, flux: f32) {
        self.flux_history[self.flux_history_pos] = flux;
        self.flux_history_pos = (self.flux_history_pos + 1) % HISTORY_LEN;
        if self.flux_history_count < HISTORY_LEN {
            self.flux_history_count += 1;
        }
    }

    /// Analyse the current frame and return an onset event if the spectral
    /// flux exceeds the adaptive threshold.
    fn analyze_frame(&mut self, sample_offset: usize) -> Option<OnsetEvent> {
        let num_bins = self.fft_size / 2 + 1;

        // Apply window and copy into complex buffer.
        for i in 0..self.fft_size {
            let windowed = self.input_buffer[i] * self.window[i];
            self.complex_buffer[i] =
                Complex::new(if windowed.is_finite() { windowed } else { 0.0 }, 0.0);
        }

        // Forward FFT.
        self.fft
            .process_with_scratch(&mut self.complex_buffer, &mut self.scratch);

        // Compute magnitudes for the positive-frequency bins.
        let current_magnitudes: Vec<f32> = self.complex_buffer[..num_bins]
            .iter()
            .map(|c| {
                let re = if c.re.is_finite() { c.re } else { 0.0 };
                let im = if c.im.is_finite() { c.im } else { 0.0 };
                re.hypot(im)
            })
            .collect();

        if !self.has_prev {
            // First frame: just store magnitudes, no onset possible.
            self.prev_magnitudes.copy_from_slice(&current_magnitudes);
            self.has_prev = true;
            return None;
        }

        // Spectral flux: sum of positive half-wave rectified differences.
        let mut flux = 0.0_f32;
        for (cur, prev) in current_magnitudes.iter().zip(self.prev_magnitudes.iter()) {
            let diff = cur - prev;
            if diff > 0.0 {
                flux += diff;
            }
        }
        let flux = if flux.is_finite() { flux } else { 0.0 };

        // Adaptive thresholding via running median.
        // Compute the threshold *before* recording the current flux value,
        // so the current frame is compared against its predecessors.
        let median = self.flux_median();
        let adaptive_threshold = self.threshold_factor.mul_add(median, f32::EPSILON);

        // Record this flux value.
        self.record_flux(flux);

        // Update peak flux for strength normalization.
        if flux > self.peak_flux {
            self.peak_flux = flux;
        }

        // Compute spectral centroid for brightness classification.
        let centroid =
            compute_spectral_centroid(&current_magnitudes, self.sample_rate, self.fft_size);

        // Store current magnitudes for next frame.
        self.prev_magnitudes.copy_from_slice(&current_magnitudes);

        // Fire onset if the flux exceeds the adaptive threshold.
        if flux > adaptive_threshold && flux > f32::EPSILON {
            let strength = (flux / self.peak_flux).clamp(0.0, 1.0);
            Some(OnsetEvent {
                sample_offset,
                strength,
                spectral_centroid: centroid,
            })
        } else {
            None
        }
    }
}

/// Compute the spectral centroid from a magnitude spectrum.
///
/// The spectral centroid is the weighted mean of the frequencies present in
/// the signal, where the magnitudes are the weights.
fn compute_spectral_centroid(magnitudes: &[f32], sample_rate: f32, fft_size: usize) -> f32 {
    let num_bins = magnitudes.len();
    if num_bins == 0 || fft_size == 0 {
        return 0.0;
    }

    let bin_width = sample_rate / fft_size as f32;
    let mut weighted_sum = 0.0_f32;
    let mut total_magnitude = 0.0_f32;

    for (i, &mag) in magnitudes.iter().enumerate() {
        let freq = i as f32 * bin_width;
        let safe_mag = if mag.is_finite() && mag >= 0.0 {
            mag
        } else {
            0.0
        };
        weighted_sum += freq * safe_mag;
        total_magnitude += safe_mag;
    }

    if total_magnitude > f32::EPSILON {
        let centroid = weighted_sum / total_magnitude;
        if centroid.is_finite() { centroid } else { 0.0 }
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_in_silence_detected() {
        let fft_size = 512;
        let hop_size = 256;
        let sample_rate = 44100.0;
        // Use a very low threshold (very sensitive).
        let mut detector = OnsetDetector::new(fft_size, hop_size, sample_rate, 0.05);

        // Build a signal: silence, then a sharp click, then silence.
        let mut signal = vec![0.0_f32; fft_size * 2];
        // Insert a click (impulse).
        let click_pos = fft_size + fft_size / 2;
        signal.resize(click_pos, 0.0);
        signal.push(1.0);
        // A few more high-energy samples to make it clearly transient.
        for _ in 0..32 {
            signal.push(0.8);
        }
        signal.resize(signal.len() + fft_size * 2, 0.0);

        let events = detector.push_samples(&signal);

        assert!(
            !events.is_empty(),
            "should detect at least one onset from a click in silence"
        );

        // The onset should have positive strength.
        for event in &events {
            assert!(
                event.strength > 0.0,
                "onset strength should be positive, got {}",
                event.strength
            );
            assert!(
                event.strength <= 1.0,
                "onset strength should be <= 1.0, got {}",
                event.strength
            );
            assert!(
                event.spectral_centroid.is_finite(),
                "spectral centroid should be finite"
            );
        }
    }

    #[test]
    fn silence_no_onsets() {
        let mut detector = OnsetDetector::new(512, 256, 44100.0, 0.3);
        let silence = vec![0.0_f32; 4096];
        let events = detector.push_samples(&silence);
        assert!(
            events.is_empty(),
            "pure silence should produce no onsets, got {}",
            events.len()
        );
    }

    #[test]
    fn continuous_tone_minimal_onsets() {
        let fft_size = 512;
        let hop_size = 256;
        let sample_rate = 44100.0;
        // Use a moderate threshold.
        let mut detector = OnsetDetector::new(fft_size, hop_size, sample_rate, 0.5);

        // Generate a continuous 440 Hz sine (steady state, no transients
        // after ramp-up).
        let num_samples = 44100; // 1 second
        let samples: Vec<f32> = (0..num_samples)
            .map(|i| {
                let t = i as f32 / sample_rate;
                (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            })
            .collect();

        let events = detector.push_samples(&samples);

        // A steady sine should produce very few onsets. The first frame or
        // two may trigger as energy ramps up, but the adaptive median-based
        // threshold should suppress the steady-state.
        assert!(
            events.len() <= 5,
            "steady sine should have few onsets, got {}",
            events.len()
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut detector = OnsetDetector::new(512, 256, 44100.0, 0.3);

        // Push some audio.
        let samples = vec![1.0_f32; 512];
        let _ = detector.push_samples(&samples);

        detector.reset();
        assert_eq!(detector.buffer_pos, 0);
        assert!(!detector.has_prev);
    }

    #[test]
    fn nan_inf_handled() {
        let mut detector = OnsetDetector::new(256, 128, 44100.0, 0.3);
        let mut samples = vec![f32::NAN; 256];
        samples.extend_from_slice(&[f32::INFINITY; 256]);
        // Should not panic.
        let events = detector.push_samples(&samples);
        for event in &events {
            assert!(event.strength.is_finite());
            assert!(event.spectral_centroid.is_finite());
        }
    }

    #[test]
    fn spectral_centroid_is_reasonable() {
        let magnitudes = vec![0.0, 0.0, 0.0, 1.0, 0.0]; // Peak at bin 3.
        let sample_rate = 44100.0;
        let fft_size = 8; // 5 bins for fft_size 8.
        let centroid = compute_spectral_centroid(&magnitudes, sample_rate, fft_size);

        let expected_freq = 3.0 * sample_rate / fft_size as f32;
        assert!(
            (centroid - expected_freq).abs() < 0.01,
            "centroid should be at bin 3 frequency {expected_freq}, got {centroid}"
        );
    }

    #[test]
    fn spectral_centroid_empty() {
        assert!((compute_spectral_centroid(&[], 44100.0, 512)).abs() < f32::EPSILON);
    }

    #[test]
    fn bad_constructor_params() {
        // Very small fft_size.
        let d = OnsetDetector::new(1, 1, 44100.0, 0.5);
        assert!(d.fft_size >= 4);

        // Zero hop.
        let d2 = OnsetDetector::new(512, 0, 44100.0, 0.5);
        assert!(d2.hop_size >= 1);

        // NaN threshold.
        let d3 = OnsetDetector::new(512, 256, 44100.0, f32::NAN);
        assert!(d3.threshold_factor.is_finite());
    }
}
