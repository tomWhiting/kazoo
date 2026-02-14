//! Formant extraction via Linear Predictive Coding (LPC).
//!
//! Analyses the spectral envelope of voiced speech to find formant frequencies
//! (resonant peaks of the vocal tract). Uses Levinson-Durbin recursion to
//! compute LPC coefficients, then finds roots of the resulting polynomial to
//! identify formant frequencies and bandwidths.

use crate::sanitize_sample;

/// Formant analysis output.
#[derive(Debug, Clone)]
pub struct FormantData {
    /// Formant frequencies in Hz, sorted ascending (F1, F2, ...).
    pub frequencies: Vec<f32>,
    /// Bandwidth of each formant in Hz (same order as `frequencies`).
    pub bandwidths: Vec<f32>,
    /// Number of formants found.
    pub num_formants: usize,
}

/// Extracts formant frequencies from audio using LPC analysis.
///
/// Incoming samples are accumulated into a frame buffer. When a full frame is
/// available the extractor computes LPC coefficients via Levinson-Durbin
/// recursion, finds the roots of the LPC polynomial, and extracts formant
/// frequencies and bandwidths from those roots.
#[derive(Debug, Clone)]
pub struct FormantExtractor {
    lpc_order: usize,
    frame_size: usize,
    sample_rate: f32,
    buffer: Vec<f32>,
    buffer_pos: usize,
    /// Scratch space for autocorrelation (length = `lpc_order` + 1).
    autocorrelation: Vec<f64>,
    /// Scratch space for LPC coefficients (length = `lpc_order` + 1).
    lpc_coefficients: Vec<f64>,
}

impl FormantExtractor {
    /// Create a new formant extractor.
    ///
    /// * `lpc_order` - Order of the LPC model. Higher orders capture more
    ///   spectral detail but increase computation. Typical range: 8-16 for
    ///   speech at 8-16 kHz sample rates, 20-30 for 44.1 kHz.
    /// * `frame_size` - Number of samples per analysis frame.
    /// * `sample_rate` - Audio sample rate in Hz.
    #[must_use]
    pub fn new(lpc_order: usize, frame_size: usize, sample_rate: f32) -> Self {
        let lpc_order = lpc_order.max(2);
        let frame_size = frame_size.max(lpc_order + 1);
        let safe_sr = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };

        Self {
            lpc_order,
            frame_size,
            sample_rate: safe_sr,
            buffer: vec![0.0; frame_size],
            buffer_pos: 0,
            autocorrelation: vec![0.0; lpc_order + 1],
            lpc_coefficients: vec![0.0; lpc_order + 1],
        }
    }

    /// Push audio samples and return formant data when a frame is complete.
    ///
    /// Returns `Some(FormantData)` when a full frame has been accumulated and
    /// analysed, or `None` if more samples are needed.
    pub fn push_samples(&mut self, samples: &[f32]) -> Option<FormantData> {
        let mut result = None;

        for &s in samples {
            self.buffer[self.buffer_pos] = sanitize_sample(s);
            self.buffer_pos += 1;

            if self.buffer_pos >= self.frame_size {
                result = Some(self.analyze_frame());
                self.buffer_pos = 0;
            }
        }

        result
    }

    /// Reset the extractor, clearing all internal state.
    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.buffer_pos = 0;
    }

    /// Return the LPC order.
    #[must_use]
    pub const fn lpc_order(&self) -> usize {
        self.lpc_order
    }

    /// Return the frame size.
    #[must_use]
    pub const fn frame_size(&self) -> usize {
        self.frame_size
    }

    /// Analyse the current buffer to extract formant data.
    fn analyze_frame(&mut self) -> FormantData {
        // Apply a Hamming window to the frame.
        let windowed: Vec<f64> = (0..self.frame_size)
            .map(|i| {
                let w = 0.46f64.mul_add(
                    -(2.0 * std::f64::consts::PI * i as f64 / (self.frame_size as f64 - 1.0)).cos(),
                    0.54,
                );
                f64::from(self.buffer[i]) * w
            })
            .collect();

        // Pre-emphasis to boost high frequencies (common in speech analysis).
        let mut emphasized = vec![0.0_f64; windowed.len()];
        emphasized[0] = windowed[0];
        for i in 1..windowed.len() {
            emphasized[i] = 0.97f64.mul_add(-windowed[i - 1], windowed[i]);
        }

        // Compute autocorrelation.
        self.compute_autocorrelation(&emphasized);

        // Check for silence / degenerate input.
        if self.autocorrelation[0] < 1e-10 {
            return FormantData {
                frequencies: Vec::new(),
                bandwidths: Vec::new(),
                num_formants: 0,
            };
        }

        // Levinson-Durbin to get LPC coefficients.
        if !self.levinson_durbin() {
            return FormantData {
                frequencies: Vec::new(),
                bandwidths: Vec::new(),
                num_formants: 0,
            };
        }

        // Find roots of the LPC polynomial and extract formants.
        self.extract_formants_from_lpc()
    }

    /// Compute the autocorrelation of the signal for lags `0..=lpc_order`.
    fn compute_autocorrelation(&mut self, signal: &[f64]) {
        let n = signal.len();
        for lag in 0..=self.lpc_order {
            let mut sum = 0.0_f64;
            for i in 0..n - lag {
                sum += signal[i] * signal[i + lag];
            }
            self.autocorrelation[lag] = if sum.is_finite() { sum } else { 0.0 };
        }
    }

    /// Levinson-Durbin recursion to compute LPC coefficients.
    ///
    /// Returns `false` if the recursion fails (e.g. due to numerical issues
    /// with the input signal).
    fn levinson_durbin(&mut self) -> bool {
        let order = self.lpc_order;
        let r = &self.autocorrelation;

        if r[0].abs() < 1e-30 {
            self.lpc_coefficients.fill(0.0);
            return false;
        }

        let mut a = vec![0.0_f64; order + 1];
        let mut a_prev = vec![0.0_f64; order + 1];
        a[0] = 1.0;
        a_prev[0] = 1.0;
        let mut error = r[0];

        for i in 1..=order {
            // Compute reflection coefficient.
            let mut lambda = 0.0_f64;
            for j in 0..i {
                lambda += a_prev[j] * r[i - j];
            }

            if error.abs() < 1e-30 {
                self.lpc_coefficients.fill(0.0);
                return false;
            }

            lambda = -lambda / error;

            if !lambda.is_finite() || lambda.abs() >= 1.0 {
                // Unstable or non-finite reflection coefficient: bail out
                // with what we have so far.
                break;
            }

            // Update coefficients.
            for j in 0..=i {
                a[j] = lambda.mul_add(a_prev[i - j], a_prev[j]);
            }

            error *= lambda.mul_add(-lambda, 1.0);
            if error < 1e-30 {
                break;
            }

            a_prev[..=i].copy_from_slice(&a[..=i]);
        }

        self.lpc_coefficients[..=order].copy_from_slice(&a[..=order]);
        true
    }

    /// Extract formant frequencies and bandwidths from the LPC polynomial roots.
    ///
    /// Uses the Durand-Kerner method to find roots of the LPC polynomial,
    /// then filters for roots inside the unit circle with positive frequency
    /// (positive angle) to identify formant candidates.
    fn extract_formants_from_lpc(&self) -> FormantData {
        let roots = find_polynomial_roots(&self.lpc_coefficients[..=self.lpc_order]);

        let nyquist = f64::from(self.sample_rate) / 2.0;
        let mut formants: Vec<(f32, f32)> = Vec::new();

        for root in &roots {
            // Only consider roots inside or on the unit circle with positive
            // imaginary part (each conjugate pair corresponds to one formant).
            let mag = root.norm();
            if mag >= 1.0 || root.im <= 0.0 {
                continue;
            }

            // Frequency from the angle of the root.
            let angle = root.im.atan2(root.re);
            if angle <= 0.0 || !angle.is_finite() {
                continue;
            }
            let freq = angle * f64::from(self.sample_rate) / (2.0 * std::f64::consts::PI);

            if !freq.is_finite() || freq <= 0.0 || freq >= nyquist {
                continue;
            }

            // Bandwidth from the distance of the root to the unit circle.
            let bw = -f64::from(self.sample_rate) / (2.0 * std::f64::consts::PI) * mag.ln();
            let bw = if bw.is_finite() && bw > 0.0 { bw } else { 50.0 };

            // Filter: typical speech formants have bandwidths < ~500 Hz and
            // frequencies above ~90 Hz.
            if bw < 600.0 && freq > 80.0 {
                formants.push((freq as f32, bw as f32));
            }
        }

        // Sort by frequency.
        formants.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Deduplicate: merge formants that are very close together.
        let mut merged: Vec<(f32, f32)> = Vec::new();
        for (freq, bw) in &formants {
            if let Some(last) = merged.last_mut() {
                if (*freq - last.0).abs() < 50.0 {
                    // Merge: keep the one with narrower bandwidth.
                    if *bw < last.1 {
                        *last = (*freq, *bw);
                    }
                    continue;
                }
            }
            merged.push((*freq, *bw));
        }

        let num_formants = merged.len();
        let (frequencies, bandwidths): (Vec<f32>, Vec<f32>) = merged.into_iter().unzip();

        FormantData {
            frequencies,
            bandwidths,
            num_formants,
        }
    }
}

/// Complex number type for root finding (using f64 for numerical stability).
#[derive(Debug, Clone, Copy)]
struct Complexf64 {
    re: f64,
    im: f64,
}

impl Complexf64 {
    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn norm(&self) -> f64 {
        self.re.hypot(self.im)
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }

    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re.mul_add(other.re, -self.im * other.im),
            self.re.mul_add(other.im, self.im * other.re),
        )
    }

    fn div(self, other: Self) -> Self {
        let denom = other.re.mul_add(other.re, other.im * other.im);
        if denom < 1e-30 {
            return Self::new(0.0, 0.0);
        }
        Self::new(
            self.re.mul_add(other.re, self.im * other.im) / denom,
            self.im.mul_add(other.re, -self.re * other.im) / denom,
        )
    }
}

/// Find the roots of a polynomial using the Durand-Kerner method.
///
/// The polynomial is given as `coeffs[0] + coeffs[1]*z + coeffs[2]*z^2 + ...`.
/// Note: the LPC polynomial has `coeffs[0] = 1.0` (the leading coefficient),
/// so we interpret it as `1 + a1*z^-1 + a2*z^-2 + ...` which is equivalent to
/// `z^n + a1*z^(n-1) + ... + an` after multiplication by `z^n`.
fn find_polynomial_roots(coeffs: &[f64]) -> Vec<Complexf64> {
    let n = coeffs.len();
    if n <= 1 {
        return Vec::new();
    }
    let degree = n - 1;
    if degree == 0 {
        return Vec::new();
    }

    // Normalize polynomial so leading coefficient is 1.
    // LPC coefficients are [1, a1, a2, ...], so we reverse for standard form:
    // p(z) = z^n + a1*z^(n-1) + ... + an
    // This is already in the correct form for root finding.
    let lead = coeffs[0];
    if lead.abs() < 1e-30 {
        return Vec::new();
    }

    // Build the companion polynomial coefficients in standard form
    // p(z) = z^n + c[1]*z^(n-1) + ... + c[n]
    let mut poly = vec![0.0_f64; n];
    for (i, c) in coeffs.iter().enumerate() {
        poly[i] = c / lead;
    }

    // Durand-Kerner iteration.
    // Initial guesses: equally spaced around a circle of radius 0.9.
    let mut roots: Vec<Complexf64> = (0..degree)
        .map(|k| {
            let angle = 2.0 * std::f64::consts::PI * k as f64 / degree as f64 + 0.4;
            let r = 0.9;
            Complexf64::new(r * angle.cos(), r * angle.sin())
        })
        .collect();

    let max_iter = 200;
    let tolerance = 1e-10;

    for _ in 0..max_iter {
        let mut max_delta = 0.0_f64;

        for i in 0..degree {
            // Evaluate polynomial at roots[i].
            let z = roots[i];
            let mut p_val = Complexf64::new(1.0, 0.0);
            for coeff in &poly[1..] {
                p_val = p_val.mul(z);
                p_val = Complexf64::new(p_val.re + coeff, p_val.im);
            }

            // Compute denominator: product of (roots[i] - roots[j]) for j != i.
            let mut denom = Complexf64::new(1.0, 0.0);
            for (j, root_j) in roots.iter().enumerate() {
                if j != i {
                    let diff = z.sub(*root_j);
                    denom = denom.mul(diff);
                }
            }

            let correction = p_val.div(denom);
            let delta = correction.norm();
            if delta.is_finite() {
                roots[i] = z.sub(correction);
                if delta > max_delta {
                    max_delta = delta;
                }
            }
        }

        if max_delta < tolerance {
            break;
        }
    }

    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Generate a synthetic vowel-like signal by summing sinusoids at
    /// formant frequencies with decreasing amplitudes.
    fn generate_synthetic_vowel(
        formant_freqs: &[f32],
        sample_rate: f32,
        num_samples: usize,
    ) -> Vec<f32> {
        let mut signal = vec![0.0_f32; num_samples];
        for (idx, &freq) in formant_freqs.iter().enumerate() {
            let amplitude = 1.0 / (idx as f32 + 1.0);
            for (i, s) in signal.iter_mut().enumerate() {
                let t = i as f32 / sample_rate;
                *s += amplitude * (2.0 * PI * freq * t).sin();
            }
        }
        // Normalize.
        let max_abs = signal.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        if max_abs > f32::EPSILON {
            for s in &mut signal {
                *s /= max_abs;
            }
        }
        signal
    }

    #[test]
    fn synthetic_vowel_ah_detects_formants() {
        // "ah" vowel: F1 ~ 730 Hz, F2 ~ 1090 Hz, F3 ~ 2440 Hz
        let sample_rate = 16000.0;
        let frame_size = 1024;
        let lpc_order = 16; // Enough for 3-4 formants at 16 kHz.

        let mut extractor = FormantExtractor::new(lpc_order, frame_size, sample_rate);

        let formant_freqs = [730.0, 1090.0, 2440.0];
        let signal = generate_synthetic_vowel(&formant_freqs, sample_rate, frame_size * 3);

        let mut last_data = None;
        for chunk in signal.chunks(256) {
            if let Some(data) = extractor.push_samples(chunk) {
                last_data = Some(data);
            }
        }

        let data = last_data.expect("should have produced formant data");

        // We should find at least 1 formant.
        assert!(
            data.num_formants >= 1,
            "should detect at least 1 formant, got {}",
            data.num_formants
        );

        // Check that at least one detected formant is in a reasonable speech range.
        let has_reasonable = data.frequencies.iter().any(|&f| f > 200.0 && f < 5000.0);
        assert!(
            has_reasonable,
            "at least one formant should be in speech range, got {:?}",
            data.frequencies
        );

        // All frequencies should be positive and below Nyquist.
        let nyquist = sample_rate / 2.0;
        for &freq in &data.frequencies {
            assert!(freq > 0.0, "formant freq should be positive, got {freq}");
            assert!(
                freq < nyquist,
                "formant freq should be below Nyquist, got {freq}"
            );
        }

        // All bandwidths should be positive.
        for &bw in &data.bandwidths {
            assert!(bw > 0.0, "bandwidth should be positive, got {bw}");
        }
    }

    #[test]
    fn silence_yields_no_formants() {
        let mut extractor = FormantExtractor::new(12, 512, 44100.0);
        let silence = vec![0.0_f32; 512];
        let data = extractor
            .push_samples(&silence)
            .expect("should produce data");
        assert_eq!(data.num_formants, 0, "silence should have no formants");
    }

    #[test]
    fn formants_sorted_ascending() {
        let sample_rate = 16000.0;
        let frame_size = 1024;
        let mut extractor = FormantExtractor::new(16, frame_size, sample_rate);

        let signal = generate_synthetic_vowel(&[500.0, 1500.0, 2500.0], sample_rate, frame_size);
        let data = extractor
            .push_samples(&signal)
            .expect("should produce data");

        for window in data.frequencies.windows(2) {
            assert!(
                window[0] <= window[1],
                "formants should be ascending: {} > {}",
                window[0],
                window[1]
            );
        }
    }

    #[test]
    fn reset_clears_state() {
        let mut extractor = FormantExtractor::new(12, 512, 44100.0);
        let samples = vec![0.5_f32; 256];
        let _ = extractor.push_samples(&samples);
        assert!(extractor.buffer_pos > 0);

        extractor.reset();
        assert_eq!(extractor.buffer_pos, 0);
    }

    #[test]
    fn nan_inf_handled() {
        let mut extractor = FormantExtractor::new(12, 256, 44100.0);
        let mut samples = vec![f32::NAN; 128];
        samples.extend_from_slice(&[f32::INFINITY; 128]);

        let data = extractor
            .push_samples(&samples)
            .expect("should produce data");
        for &f in &data.frequencies {
            assert!(f.is_finite(), "frequency should be finite, got {f}");
        }
        for &bw in &data.bandwidths {
            assert!(bw.is_finite(), "bandwidth should be finite, got {bw}");
        }
    }

    #[test]
    fn bad_constructor_params() {
        // LPC order 0 should be clamped to 2.
        let e = FormantExtractor::new(0, 100, 44100.0);
        assert!(e.lpc_order() >= 2);

        // Frame size smaller than lpc_order should be adjusted.
        let e2 = FormantExtractor::new(20, 5, 44100.0);
        assert!(e2.frame_size() > e2.lpc_order());

        // NaN sample rate should default.
        let e3 = FormantExtractor::new(12, 512, f32::NAN);
        assert!(e3.sample_rate > 0.0);
    }

    #[test]
    fn polynomial_root_finding_basic() {
        // p(z) = z^2 - 3z + 2 = (z-1)(z-2)
        // coeffs: [1.0, -3.0, 2.0]
        let roots = find_polynomial_roots(&[1.0, -3.0, 2.0]);
        assert_eq!(roots.len(), 2);

        // Sort by real part for deterministic checking.
        let mut real_parts: Vec<f64> = roots.iter().map(|r| r.re).collect();
        real_parts.sort_by(|a, b| a.partial_cmp(b).unwrap());

        assert!(
            (real_parts[0] - 1.0).abs() < 0.01,
            "root should be ~1.0, got {}",
            real_parts[0]
        );
        assert!(
            (real_parts[1] - 2.0).abs() < 0.01,
            "root should be ~2.0, got {}",
            real_parts[1]
        );
    }

    #[test]
    fn polynomial_root_finding_complex_roots() {
        // p(z) = z^2 + 1 = (z-i)(z+i)
        let roots = find_polynomial_roots(&[1.0, 0.0, 1.0]);
        assert_eq!(roots.len(), 2);

        for root in &roots {
            let mag = root.norm();
            assert!(
                (mag - 1.0).abs() < 0.01,
                "roots of z^2+1 should have magnitude 1, got {mag}"
            );
        }
    }
}
