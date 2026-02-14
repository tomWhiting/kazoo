//! Envelope follower for tracking the amplitude contour of an audio signal.
//!
//! Uses a simple one-pole smoothing filter with separate attack and release
//! coefficients. The attack coefficient governs how quickly the envelope rises
//! to follow a louder signal, while the release coefficient controls the decay
//! when the signal drops.

use crate::sanitize_sample;

/// Tracks the instantaneous amplitude envelope of an audio signal.
///
/// The envelope follower computes smoothing coefficients from attack/release
/// times in milliseconds and maintains a running envelope value updated on
/// each sample via a one-pole filter.
#[derive(Debug, Clone)]
pub struct EnvelopeFollower {
    /// Smoothing coefficient for the rising (attack) phase.
    attack_coeff: f32,
    /// Smoothing coefficient for the falling (release) phase.
    release_coeff: f32,
    /// Current envelope value.
    current: f32,
}

impl EnvelopeFollower {
    /// Create a new envelope follower.
    ///
    /// Coefficients are computed as `exp(-1.0 / (time_ms * sample_rate / 1000.0))`.
    /// Invalid inputs (NaN, Inf, non-positive) are treated defensively: times
    /// are clamped to a minimum of `0.01` ms and sample rate to `1.0` Hz.
    #[must_use]
    pub fn new(attack_ms: f32, release_ms: f32, sample_rate: f32) -> Self {
        Self {
            attack_coeff: compute_coefficient(attack_ms, sample_rate),
            release_coeff: compute_coefficient(release_ms, sample_rate),
            current: 0.0,
        }
    }

    /// Process a single sample and return the updated envelope value.
    ///
    /// If the absolute value of the input exceeds the current envelope, the
    /// attack coefficient is used; otherwise the release coefficient governs
    /// the decay. NaN/Inf inputs are treated as `0.0`.
    #[inline]
    pub fn process_sample(&mut self, sample: f32) -> f32 {
        let rectified = sanitize_sample(sample).abs();
        let coeff = if rectified > self.current {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        // One-pole filter: current = coeff * current + (1 - coeff) * rectified
        self.current = coeff.mul_add(self.current, (1.0 - coeff) * rectified);
        // Defensive: ensure we never drift to non-finite values.
        self.current = sanitize_sample(self.current);
        self.current
    }

    /// Process a block of samples and return the final envelope value.
    ///
    /// Every sample in the block is processed sequentially; the return value
    /// is the envelope state after the last sample.
    pub fn process_block(&mut self, samples: &[f32]) -> f32 {
        let mut last = self.current;
        for &s in samples {
            last = self.process_sample(s);
        }
        last
    }

    /// Reset the envelope to zero (silence).
    pub const fn reset(&mut self) {
        self.current = 0.0;
    }

    /// Return the current envelope value without processing any new input.
    #[must_use]
    pub const fn current(&self) -> f32 {
        self.current
    }
}

/// Compute a one-pole smoothing coefficient from a time constant in
/// milliseconds and a sample rate in Hz.
///
/// The formula is `exp(-1.0 / (time_ms * sample_rate / 1000.0))`. Non-finite
/// or non-positive inputs are clamped to safe minimums so the result is always
/// in the range `[0.0, 1.0)`.
fn compute_coefficient(time_ms: f32, sample_rate: f32) -> f32 {
    let safe_time = if time_ms.is_finite() && time_ms > 0.0 {
        time_ms
    } else {
        0.01
    };
    let safe_sr = if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate
    } else {
        1.0
    };
    let samples = safe_time * safe_sr / 1000.0;
    // Guard against extremely small denominators.
    if samples < f32::EPSILON {
        return 0.0;
    }
    let coeff = (-1.0_f32 / samples).exp();
    if coeff.is_finite() {
        coeff.clamp(0.0, 1.0 - f32::EPSILON)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn step_response_rises() {
        let mut env = EnvelopeFollower::new(5.0, 50.0, 44100.0);
        // Feed silence then a step of 1.0.
        for _ in 0..100 {
            env.process_sample(0.0);
        }
        assert!(env.current() < 0.001, "should be near zero after silence");

        // Feed 1.0 for a number of samples; envelope should rise.
        for _ in 0..500 {
            env.process_sample(1.0);
        }
        assert!(
            env.current() > 0.8,
            "envelope should have risen significantly, got {}",
            env.current()
        );
    }

    #[test]
    fn step_response_falls() {
        let mut env = EnvelopeFollower::new(1.0, 20.0, 44100.0);
        // Drive envelope up.
        for _ in 0..2000 {
            env.process_sample(1.0);
        }
        let peak = env.current();
        assert!(peak > 0.95, "should be near 1.0, got {peak}");

        // Let it decay.
        for _ in 0..4000 {
            env.process_sample(0.0);
        }
        assert!(
            env.current() < peak * 0.3,
            "envelope should have decayed, still at {}",
            env.current()
        );
    }

    #[test]
    fn sine_tracking() {
        let mut env = EnvelopeFollower::new(10.0, 50.0, 44100.0);
        let freq = 220.0;
        let sample_rate = 44100.0;
        let num_samples = 44100; // 1 second

        let mut last = 0.0_f32;
        for i in 0..num_samples {
            let t = i as f32 / sample_rate;
            let sample = (2.0 * PI * freq * t).sin();
            last = env.process_sample(sample);
        }
        // A sine of amplitude 1.0 has peak absolute value 1.0.
        // The envelope should track somewhere in a reasonable range.
        assert!(
            last > 0.3,
            "envelope of unit sine should be substantial, got {last}"
        );
        assert!(last <= 1.0, "envelope should not exceed 1.0, got {last}");
    }

    #[test]
    fn attack_faster_than_release() {
        let attack_ms = 1.0;
        let release_ms = 100.0;
        let sr = 44100.0;
        let mut env = EnvelopeFollower::new(attack_ms, release_ms, sr);

        // Measure attack: how many samples to reach 0.9 from 0
        let mut attack_count = 0u32;
        for _ in 0..44100 {
            env.process_sample(1.0);
            attack_count += 1;
            if env.current() >= 0.9 {
                break;
            }
        }

        let peak = env.current();

        // Measure release: how many samples to fall below 0.1 * peak
        let mut release_count = 0u32;
        for _ in 0..44100 * 5 {
            env.process_sample(0.0);
            release_count += 1;
            if env.current() <= 0.1 * peak {
                break;
            }
        }

        assert!(
            release_count > attack_count,
            "release ({release_count}) should take longer than attack ({attack_count})"
        );
    }

    #[test]
    fn process_block_returns_final() {
        let mut env = EnvelopeFollower::new(5.0, 50.0, 44100.0);
        let block: Vec<f32> = (0..256).map(|i| (i as f32 / 256.0).sin()).collect();
        let result = env.process_block(&block);
        assert!(
            (result - env.current()).abs() < f32::EPSILON,
            "process_block should return final envelope"
        );
    }

    #[test]
    fn process_block_empty_is_noop() {
        let mut env = EnvelopeFollower::new(5.0, 50.0, 44100.0);
        env.process_sample(0.5);
        let before = env.current();
        let result = env.process_block(&[]);
        assert!(
            (result - before).abs() < f32::EPSILON,
            "empty block should not change state"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut env = EnvelopeFollower::new(5.0, 50.0, 44100.0);
        for _ in 0..1000 {
            env.process_sample(1.0);
        }
        assert!(env.current() > 0.5);
        env.reset();
        assert!(
            env.current().abs() < f32::EPSILON,
            "reset should clear to 0"
        );
    }

    #[test]
    fn nan_input_treated_as_zero() {
        let mut env = EnvelopeFollower::new(5.0, 50.0, 44100.0);
        for _ in 0..1000 {
            env.process_sample(1.0);
        }
        let before = env.current();
        // Feed NaN — should act like silence (0.0).
        env.process_sample(f32::NAN);
        assert!(
            env.current() <= before,
            "NaN input should not raise envelope"
        );
        assert!(env.current().is_finite(), "envelope should remain finite");
    }

    #[test]
    fn inf_input_treated_as_zero() {
        let mut env = EnvelopeFollower::new(5.0, 50.0, 44100.0);
        env.process_sample(f32::INFINITY);
        assert!(
            env.current().is_finite(),
            "Inf input should yield finite envelope"
        );
        env.process_sample(f32::NEG_INFINITY);
        assert!(
            env.current().is_finite(),
            "neg Inf input should yield finite envelope"
        );
    }

    #[test]
    fn bad_constructor_params() {
        // Zero/negative times and sample rate should not panic.
        let env = EnvelopeFollower::new(0.0, -1.0, 0.0);
        assert!(env.current().is_finite());

        let env2 = EnvelopeFollower::new(f32::NAN, f32::INFINITY, f32::NEG_INFINITY);
        assert!(env2.current().is_finite());
    }
}
