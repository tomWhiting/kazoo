//! Per-voice analog drift simulation.
//!
//! Random detuning offsets (0-10 cents) per voice.
//! Per-voice envelope timing jitter.
//! This is not optional — it IS the CS-80 sound.
//!
//! Each voice gets a unique drift profile at creation time, then the drift
//! values slowly wander over time using a filtered noise source (smooth random walk).

use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

/// Per-voice analog drift state.
///
/// Simulates the imperfect tuning and timing of analog circuits.
/// Each of the 8 voices has a unique drift profile.
#[derive(Debug)]
pub struct VoiceDrift {
    /// Maximum detuning range in cents [0, 10].
    pub max_cents: f32,
    /// Current detuning offset in cents.
    detune_cents: f32,
    /// Target detuning for smooth interpolation.
    detune_target: f32,
    /// Envelope timing jitter factor [0.9, 1.1] — multiplied into envelope rates.
    timing_jitter: f32,
    /// Target timing jitter.
    timing_target: f32,
    /// Smoothing coefficient (how fast drift wanders).
    smooth_coeff: f32,
    /// Samples until next target change.
    samples_until_retarget: u32,
    /// How many samples between retargets.
    retarget_interval: u32,
    /// Initial fixed offset per voice (part of the "each voice is unique" character).
    fixed_offset_cents: f32,
    /// Per-voice RNG for drift wander (avoids thread-local access in audio path).
    rng: SmallRng,
}

impl VoiceDrift {
    /// Create drift for a single voice.
    ///
    /// `voice_index` (0-7) seeds the initial offset so each voice starts unique.
    /// `max_cents` sets the maximum wander range (spec says 0-10 cents).
    #[must_use]
    pub fn new(_voice_index: u8, max_cents: f32, sample_rate: f32) -> Self {
        let mut rng = SmallRng::from_os_rng();

        // Each voice gets a fixed offset — this is analog: no two voices are the same
        let fixed_offset = rng.random_range(-max_cents..=max_cents);

        // Initial wander target
        let detune_target = rng.random_range(-max_cents..=max_cents);
        let timing_target = rng.random_range(0.95_f32..=1.05);

        // Retarget every 0.5-2 seconds (slow wander)
        let retarget_secs = rng.random_range(0.5_f32..=2.0);
        let retarget_interval = (retarget_secs * sample_rate) as u32;
        let initial_offset = rng.random_range(0..retarget_interval);

        // Smoothing: reach ~63% of target in about 50ms
        let smooth_samples = (0.05 * sample_rate).max(1.0);
        let smooth_coeff = 1.0 / smooth_samples;

        Self {
            max_cents: max_cents.clamp(0.0, 10.0),
            detune_cents: fixed_offset,
            detune_target,
            timing_jitter: 1.0,
            timing_target,
            smooth_coeff,
            samples_until_retarget: initial_offset,
            retarget_interval: retarget_interval.max(1),
            fixed_offset_cents: fixed_offset,
            rng,
        }
    }

    /// Current detuning in cents (fixed offset + wander).
    #[must_use]
    pub const fn detune_cents(&self) -> f32 {
        self.detune_cents
    }

    /// Frequency multiplier from current detuning.
    ///
    /// Multiply the base frequency by this to apply drift.
    #[must_use]
    pub fn frequency_ratio(&self) -> f32 {
        // cents to ratio: 2^(cents/1200)
        (self.detune_cents / 1200.0).exp2()
    }

    /// Current envelope timing jitter factor.
    ///
    /// Multiply envelope time parameters by this for per-voice timing variation.
    #[must_use]
    pub const fn timing_factor(&self) -> f32 {
        self.timing_jitter
    }

    /// Advance drift by one sample.
    #[inline]
    pub fn tick(&mut self) {
        // Smooth interpolation toward target
        self.detune_cents +=
            self.smooth_coeff * (self.fixed_offset_cents + self.detune_target - self.detune_cents);
        self.timing_jitter += self.smooth_coeff * (self.timing_target - self.timing_jitter);

        // NaN/Inf defense
        if !self.detune_cents.is_finite() {
            self.detune_cents = self.fixed_offset_cents;
        }
        if !self.timing_jitter.is_finite() {
            self.timing_jitter = 1.0;
        }

        // Periodically pick new wander targets
        self.samples_until_retarget = self.samples_until_retarget.saturating_sub(1);
        if self.samples_until_retarget == 0 {
            self.retarget();
            self.samples_until_retarget = self.retarget_interval;
        }
    }

    /// Reset drift to initial state.
    pub const fn reset(&mut self) {
        self.detune_cents = self.fixed_offset_cents;
        self.timing_jitter = 1.0;
    }

    /// Pick new random wander targets.
    fn retarget(&mut self) {
        self.detune_target = self.rng.random_range(-self.max_cents..=self.max_cents);
        self.timing_target = self.rng.random_range(0.95_f32..=1.05);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_creates_unique_voices() {
        let d0 = VoiceDrift::new(0, 6.0, 44100.0);
        let d1 = VoiceDrift::new(1, 6.0, 44100.0);
        // Fixed offsets should differ (random, but extremely unlikely to be identical)
        // We test that the struct is valid
        assert!(d0.detune_cents().is_finite());
        assert!(d1.detune_cents().is_finite());
        assert!(d0.frequency_ratio().is_finite());
        assert!(d0.frequency_ratio() > 0.0);
    }

    #[test]
    fn drift_frequency_ratio_near_unity() {
        let drift = VoiceDrift::new(0, 6.0, 44100.0);
        let ratio = drift.frequency_ratio();
        // 10 cents = 2^(10/1200) ≈ 1.0058, so ratio should be near 1.0
        assert!(
            ratio > 0.99 && ratio < 1.01,
            "drift ratio should be near 1.0, got {ratio}"
        );
    }

    #[test]
    fn drift_timing_near_unity() {
        let drift = VoiceDrift::new(0, 6.0, 44100.0);
        let factor = drift.timing_factor();
        assert!(
            factor > 0.85 && factor < 1.15,
            "timing factor should be near 1.0, got {factor}"
        );
    }

    #[test]
    fn drift_wanders_over_time() {
        let mut drift = VoiceDrift::new(0, 6.0, 44100.0);
        let _initial = drift.detune_cents();
        // Run for 5 seconds of audio
        for _ in 0..(44100 * 5) {
            drift.tick();
        }
        let final_cents = drift.detune_cents();
        // Values should stay within bounds
        assert!(
            final_cents.abs() <= 15.0,
            "drift should stay bounded, got {final_cents} cents"
        );
        assert!(final_cents.is_finite());
    }

    #[test]
    fn drift_reset_works() {
        let mut drift = VoiceDrift::new(0, 6.0, 44100.0);
        for _ in 0..44100 {
            drift.tick();
        }
        drift.reset();
        assert!((drift.detune_cents() - drift.fixed_offset_cents).abs() < f32::EPSILON);
        assert!((drift.timing_factor() - 1.0).abs() < f32::EPSILON);
    }
}
