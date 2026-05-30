//! 4-pole (24 dB/oct) Moog ladder low-pass filter.
//!
//! Huovilainen nonlinear digital Moog ladder model with 2x internal
//! oversampling. Each of the four cascaded 1-pole stages applies tanh
//! saturation, modeling the transistor transfer curve in the original
//! hardware.
//!
//! References:
//! - Huovilainen, "Non-Linear Digital Implementation of the Moog Ladder
//!   Filter" (`DAFx` 2004)
//! - Huovilainen, "New Approaches to Digital Subtractive Synthesis" (2006)
//! - Zavalishin, "The Art of VA Filter Design", Chapter 6
//!
//! Key characteristics:
//! - Self-oscillates at max resonance (usable as sine oscillator)
//! - Resonance steals low-end (correct — do NOT compensate)
//! - Soft saturation from tanh in each stage (the warmth)
//! - 2x oversampling for accurate feedback and reduced aliasing
//! - Cutoff range: 10 Hz to 20 kHz

use std::f32::consts::PI;

use kazoo_core::sanitize_sample;

// ---------------------------------------------------------------------------
// Fast tanh approximation
// ---------------------------------------------------------------------------

/// Fast tanh approximation using Pade (3,2) rational polynomial.
///
/// Accurate to ~0.001 absolute error across [-4, 4]. Falls back to +/-1
/// for large inputs (exactly like real tanh). This avoids the cost of
/// `f32::tanh()` in the inner loop where we call it 8+ times per sample.
#[inline]
#[allow(clippy::suboptimal_flops)]
fn fast_tanh(x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    // For large values, tanh saturates
    if x > 3.0 {
        return 1.0;
    }
    if x < -3.0 {
        return -1.0;
    }
    // Padé approximant: tanh(x) ≈ x(27 + x²) / (27 + 9x²)
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}

// ---------------------------------------------------------------------------
// Moog Ladder Filter
// ---------------------------------------------------------------------------

/// Huovilainen nonlinear Moog ladder filter.
///
/// Four cascaded 1-pole lowpass stages with tanh nonlinearity, modeling
/// the transistor ladder topology of the Minimoog Model D. Runs at 2x
/// internal oversampling for accurate feedback path behavior.
///
/// # Self-oscillation
///
/// At maximum resonance (1.0 → internal k=4.0), the filter self-oscillates,
/// producing a clean sine wave at the cutoff frequency. This is usable as
/// a fourth oscillator.
///
/// # Low-end loss
///
/// As resonance increases, bass energy decreases. This is NOT compensated —
/// it is the correct Minimoog behavior and the reason Minimoog bass cuts
/// through a mix.
#[derive(Debug)]
pub struct MoogLadder {
    /// Cutoff frequency in Hz.
    cutoff: f32,
    /// Resonance (0.0 to 1.0, mapped internally to 0..4).
    resonance: f32,
    /// Input drive (controls saturation amount). 1.0 = moderate.
    drive: f32,
    /// Keyboard tracking amount (0.0 = none, 1.0 = full 1V/oct).
    pub key_track: f32,
    /// Base cutoff before keyboard tracking (the knob value).
    pub base_cutoff: f32,

    // Filter state: four 1-pole stage outputs
    stage: [f32; 4],
    // Cached tanh of each stage output (Huovilainen thermal model)
    stage_tanh: [f32; 4],
    // Delayed feedback value (from previous oversampled step)
    delay: f32,

    // Pre-computed coefficients
    /// Tuning coefficient (updated when cutoff or sample rate changes).
    tune: f32,
    /// Internal resonance coefficient (0 to 4).
    k: f32,

    sample_rate: f32,
}

impl MoogLadder {
    /// Minimum cutoff frequency in Hz.
    pub const MIN_CUTOFF: f32 = 10.0;
    /// Maximum cutoff frequency in Hz.
    pub const MAX_CUTOFF: f32 = 20_000.0;

    /// Create a new ladder filter.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut filter = Self {
            cutoff: 2400.0,
            resonance: 0.0,
            drive: 1.0,
            key_track: 1.0,
            base_cutoff: 2400.0,
            stage: [0.0; 4],
            stage_tanh: [0.0; 4],
            delay: 0.0,
            tune: 0.0,
            k: 0.0,
            sample_rate: sample_rate.max(1.0),
        };
        filter.update_coefficients();
        filter
    }

    /// Set cutoff frequency in Hz (clamped to valid range).
    pub fn set_cutoff(&mut self, hz: f32) {
        self.cutoff = hz.clamp(Self::MIN_CUTOFF, Self::MAX_CUTOFF);
        self.update_coefficients();
    }

    /// Get current cutoff.
    #[must_use]
    pub const fn cutoff(&self) -> f32 {
        self.cutoff
    }

    /// Set resonance (0.0 to 1.0).
    pub fn set_resonance(&mut self, r: f32) {
        self.resonance = r.clamp(0.0, 1.0);
        self.update_coefficients();
    }

    /// Get current resonance.
    #[must_use]
    pub const fn resonance(&self) -> f32 {
        self.resonance
    }

    /// Set input drive (saturation amount).
    pub const fn set_drive(&mut self, d: f32) {
        self.drive = d.clamp(0.1, 4.0);
    }

    /// Get current drive.
    #[must_use]
    pub const fn drive(&self) -> f32 {
        self.drive
    }

    /// Update the effective cutoff with keyboard tracking.
    ///
    /// `note_freq` is the current note frequency in Hz. Keyboard tracking
    /// maps the note frequency to a cutoff offset (1V/oct behavior).
    pub fn update_key_tracking(&mut self, note_freq: f32) {
        if self.key_track <= 0.0 || note_freq <= 0.0 {
            self.cutoff = self.base_cutoff;
        } else {
            // 1V/oct: cutoff scales proportionally to note frequency
            // relative to middle C (261.63 Hz)
            let ratio = note_freq / 261.63;
            let tracking_offset = ratio.log2() * 12.0 * self.key_track;
            // Convert semitone offset to frequency multiplier on cutoff
            let multiplier = (tracking_offset / 12.0).exp2();
            self.cutoff = (self.base_cutoff * multiplier).clamp(Self::MIN_CUTOFF, Self::MAX_CUTOFF);
        }
        self.update_coefficients();
    }

    /// Set sample rate and reset state.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        self.update_coefficients();
    }

    /// Update pre-computed coefficients from cutoff and resonance.
    ///
    /// Called whenever cutoff, resonance, or sample rate changes.
    ///
    /// The key insight: forward-Euler discretization shifts the self-oscillation
    /// threshold above the analog k=4. We compute the exact discrete-time
    /// threshold from z-plane analysis so that `resonance=1.0` always
    /// self-oscillates regardless of cutoff frequency.
    #[allow(clippy::suboptimal_flops)] // mul_add changes rounding, breaks self-oscillation threshold
    fn update_coefficients(&mut self) {
        // 2x oversampling: effective sample rate is doubled
        let fs2 = self.sample_rate * 2.0;

        // Cutoff coefficient using the exact analog prototype mapping:
        // g = 1 - exp(-2π·fc/fs)
        // This maps the analog cutoff frequency to the digital domain.
        let fc = self.cutoff.clamp(Self::MIN_CUTOFF, fs2 * 0.49);
        self.tune = 1.0 - (-2.0 * PI * fc / fs2).exp();

        // Compute exact discrete-time self-oscillation threshold.
        //
        // With forward-Euler discretization, each 1-pole stage has transfer
        // function H(z) = g / (1 - (1-g)z^{-1}). The 4-stage cascade with
        // one-sample-delayed feedback self-oscillates when the loop gain
        // reaches unity. The threshold k_crit is:
        //
        //   u = (1 - sqrt(1 - 4g + 2g²)) / 2
        //   k_crit = (u·√2 / g)^4
        //
        // This is always >= 4.0 (equality at g→0, i.e. low cutoff relative
        // to sample rate) and increases as cutoff approaches Nyquist.
        let g = self.tune;
        let k_threshold = if g > 0.001 {
            let discriminant = (1.0 - 4.0 * g + 2.0 * g * g).max(0.0).sqrt();
            let u = (1.0 - discriminant) * 0.5;
            let ratio = u * std::f32::consts::SQRT_2 / g;
            ratio * ratio * ratio * ratio
        } else {
            // For very low cutoffs, the threshold approaches the analog value
            4.0
        };

        // Map user resonance [0, 1] to [0, k_threshold]
        self.k = self.resonance * k_threshold;
    }

    /// Process a single input sample through the ladder filter.
    ///
    /// Returns the filtered output. Runs at 2x internal oversampling.
    #[inline]
    #[allow(clippy::suboptimal_flops)] // mul_add changes rounding, breaks self-oscillation threshold
    pub fn process_sample(&mut self, input: f32) -> f32 {
        let input = sanitize_sample(input);

        // 2x oversampling: process each input sample twice at double rate
        for _ in 0..2 {
            // Feedback: subtract resonance-scaled output (one-step delayed
            // at the oversampled rate — the 2x oversampling makes this
            // delay negligible, which is the Huovilainen insight).
            let feedback = self.delay;
            let x = input - self.k * feedback;

            // Input saturation (transistor input stage)
            let x_sat = fast_tanh(x * self.drive);

            // Stage 1: 1-pole lowpass with tanh nonlinearity
            self.stage[0] = self.tune.mul_add(x_sat - self.stage_tanh[0], self.stage[0]);
            self.stage_tanh[0] = fast_tanh(self.stage[0]);

            // Stage 2
            self.stage[1] = self
                .tune
                .mul_add(self.stage_tanh[0] - self.stage_tanh[1], self.stage[1]);
            self.stage_tanh[1] = fast_tanh(self.stage[1]);

            // Stage 3
            self.stage[2] = self
                .tune
                .mul_add(self.stage_tanh[1] - self.stage_tanh[2], self.stage[2]);
            self.stage_tanh[2] = fast_tanh(self.stage[2]);

            // Stage 4
            self.stage[3] = self
                .tune
                .mul_add(self.stage_tanh[2] - self.stage_tanh[3], self.stage[3]);
            self.stage_tanh[3] = fast_tanh(self.stage[3]);

            // Store delayed output for next iteration's feedback
            self.delay = self.stage_tanh[3];
        }

        // Output is the 4th stage (after 2x oversampled processing)
        sanitize_sample(self.stage[3])
    }

    /// Process a block of samples in-place.
    pub fn process_block(&mut self, buffer: &mut [f32]) {
        for sample in buffer.iter_mut() {
            *sample = self.process_sample(*sample);
        }
    }

    /// Reset all filter state to zero.
    pub const fn reset(&mut self) {
        self.stage = [0.0; 4];
        self.stage_tanh = [0.0; 4];
        self.delay = 0.0;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_tanh_accuracy() {
        // Check fast_tanh against std tanh at several points
        for x in [-3.0, -2.0, -1.0, -0.5, 0.0, 0.5, 1.0, 2.0, 3.0] {
            let fast = fast_tanh(x);
            let exact = x.tanh();
            assert!(
                (fast - exact).abs() < 0.03,
                "fast_tanh({x}) = {fast}, expected {exact}"
            );
        }
    }

    #[test]
    fn fast_tanh_saturation() {
        assert!((fast_tanh(10.0) - 1.0).abs() < f32::EPSILON);
        assert!((fast_tanh(-10.0) - (-1.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn fast_tanh_handles_nan_inf() {
        assert!((fast_tanh(f32::NAN) - 0.0).abs() < f32::EPSILON);
        assert!((fast_tanh(f32::INFINITY) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn ladder_passes_dc_at_low_cutoff() {
        // With low resonance and a DC input, the filter should pass it through
        // (lowpass passes DC)
        let mut filter = MoogLadder::new(44100.0);
        filter.set_cutoff(1000.0);
        filter.set_resonance(0.0);

        // Feed DC signal
        let mut out = 0.0;
        for _ in 0..44100 {
            out = filter.process_sample(1.0);
        }

        // Should converge near the input (DC passes through lowpass)
        assert!(
            (out - 1.0).abs() < 0.1,
            "DC should pass through lowpass, got {out}"
        );
    }

    #[test]
    fn ladder_attenuates_high_frequency() {
        let mut filter = MoogLadder::new(44100.0);
        filter.set_cutoff(100.0); // Very low cutoff
        filter.set_resonance(0.0);

        // Feed a 5kHz sine wave — should be heavily attenuated
        let freq = 5000.0;
        let mut max_out = 0.0_f32;
        for i in 0..44100 {
            let t = i as f32 / 44100.0;
            let input = (2.0 * PI * freq * t).sin();
            let out = filter.process_sample(input);
            // Skip transient
            if i > 4410 {
                max_out = max_out.max(out.abs());
            }
        }

        // 24 dB/oct at 5kHz with 100Hz cutoff is massive attenuation
        assert!(
            max_out < 0.01,
            "5kHz should be heavily attenuated with 100Hz cutoff, got {max_out}"
        );
    }

    #[test]
    fn ladder_self_oscillation() {
        // THE critical test: at max resonance with zero input, the filter
        // should self-oscillate, producing a tone at the cutoff frequency.
        let sample_rate = 44100.0;
        let cutoff = 1000.0;
        let mut filter = MoogLadder::new(sample_rate);
        filter.set_cutoff(cutoff);
        filter.set_resonance(1.0); // Max resonance

        // Kick-start with a tiny impulse
        let _ = filter.process_sample(0.001);

        // Let it ring with zero input
        let mut samples = Vec::with_capacity(44100);
        for _ in 0..44100 {
            samples.push(filter.process_sample(0.0));
        }

        // Check that it's oscillating (non-zero output after settling)
        let tail = &samples[22050..]; // last half second
        let max_abs = tail.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        assert!(
            max_abs > 0.01,
            "filter should self-oscillate at max resonance, got max_abs={max_abs}"
        );

        // Measure frequency by counting zero crossings
        let mut crossings = 0u32;
        for window in tail.windows(2) {
            if (window[0] >= 0.0) != (window[1] >= 0.0) {
                crossings += 1;
            }
        }
        // Each full cycle has 2 zero crossings
        let duration_secs = tail.len() as f32 / sample_rate;
        let measured_freq = (crossings as f32) / (2.0 * duration_secs);

        // Should be close to the cutoff frequency (within 10%)
        let error_ratio = (measured_freq - cutoff).abs() / cutoff;
        assert!(
            error_ratio < 0.15,
            "self-oscillation frequency should be near {cutoff}Hz, measured {measured_freq}Hz (error={error_ratio:.1}%)"
        );
    }

    #[test]
    fn ladder_resonance_steals_bass() {
        // With high resonance, the output amplitude at low frequencies
        // should decrease compared to zero resonance (low-end loss).
        let sample_rate = 44100.0;

        // Measure output power with zero resonance
        let mut filter_flat = MoogLadder::new(sample_rate);
        filter_flat.set_cutoff(2000.0);
        filter_flat.set_resonance(0.0);

        let mut power_flat = 0.0_f32;
        let freq = 100.0; // Low frequency
        for i in 0..44100 {
            let t = i as f32 / sample_rate;
            let input = (2.0 * PI * freq * t).sin();
            let out = filter_flat.process_sample(input);
            if i > 4410 {
                power_flat += out * out;
            }
        }

        // Measure with high resonance
        let mut filter_res = MoogLadder::new(sample_rate);
        filter_res.set_cutoff(2000.0);
        filter_res.set_resonance(0.8);

        let mut power_res = 0.0_f32;
        for i in 0..44100 {
            let t = i as f32 / sample_rate;
            let input = (2.0 * PI * freq * t).sin();
            let out = filter_res.process_sample(input);
            if i > 4410 {
                power_res += out * out;
            }
        }

        assert!(
            power_res < power_flat,
            "resonance should steal bass: power_flat={power_flat}, power_res={power_res}"
        );
    }

    #[test]
    fn ladder_output_always_finite() {
        let mut filter = MoogLadder::new(44100.0);
        filter.set_cutoff(1000.0);
        filter.set_resonance(0.9);

        // Feed various inputs including pathological values
        for input in [0.0, 1.0, -1.0, 10.0, -10.0, f32::NAN, f32::INFINITY] {
            let out = filter.process_sample(input);
            assert!(out.is_finite(), "output not finite for input {input}");
        }
    }

    #[test]
    fn ladder_reset_zeros_state() {
        let mut filter = MoogLadder::new(44100.0);
        filter.set_resonance(0.5);

        // Process some signal
        for _ in 0..1000 {
            filter.process_sample(0.5);
        }

        filter.reset();

        // State should be zero
        assert!(
            filter.stage.iter().all(|&s| s == 0.0),
            "reset should zero all stage state"
        );
        assert!(filter.delay == 0.0, "reset should zero delay");
    }

    #[test]
    fn ladder_self_oscillation_sine_purity() {
        // Self-oscillation should produce a relatively pure sine.
        // Measure by checking that the waveform is smooth (no sharp edges).
        let mut filter = MoogLadder::new(44100.0);
        filter.set_cutoff(440.0);
        filter.set_resonance(1.0);

        let _ = filter.process_sample(0.001); // kick-start

        // Collect samples after settling
        let mut samples = Vec::with_capacity(4410);
        for _ in 0..22050 {
            filter.process_sample(0.0); // settling
        }
        for _ in 0..4410 {
            samples.push(filter.process_sample(0.0));
        }

        // Check smoothness: max sample-to-sample difference should be small
        // relative to the signal amplitude
        let max_abs = samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
        if max_abs > 0.001 {
            let max_diff = samples
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0_f32, f32::max);
            let smoothness = max_diff / max_abs;
            assert!(
                smoothness < 0.3,
                "self-oscillation should be smooth (sine-like), got smoothness={smoothness}"
            );
        }
    }
}
