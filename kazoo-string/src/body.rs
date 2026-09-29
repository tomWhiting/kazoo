//! The wooden body of the instrument: three fixed resonances that ring along
//! with the strings, the way an acoustic guitar's air cavity, top plate and
//! back do. A string on its own sounds like a wire on a nail; the body is
//! what makes it an instrument.

use std::f32::consts::TAU;

/// `(centre hertz, Q, weight)` of each resonance.
const MODES: [(f32, f32, f32); 3] = [(98.0, 9.0, 1.0), (205.0, 7.0, 0.75), (410.0, 5.0, 0.5)];
/// How hard the resonances are driven at full body.
const DRIVE: f32 = 1.6;

/// A bandpass biquad with unity gain at its centre.
#[derive(Debug, Clone, Copy, Default)]
struct Resonator {
    b0: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Resonator {
    fn new(sample_rate: f32, hz: f32, q: f32) -> Self {
        // Keep the centre below Nyquist whatever the device rate.
        let w = TAU * hz.min(sample_rate * 0.45) / sample_rate;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: alpha / a0,
            a1: -2.0 * w.cos() / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Transposed direct form II; the bandpass numerator is `b0 (1 - z^-2)`.
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0.mul_add(x, self.z1);
        self.z1 = (-self.a1).mul_add(y, self.z2);
        self.z2 = (-self.b0).mul_add(x, -self.a2 * y);
        y
    }
}

/// The three body resonances, mixed in parallel with the dry strings.
#[derive(Debug, Clone, Copy)]
pub struct Body {
    modes: [Resonator; MODES.len()],
}

impl Body {
    /// Tune the resonances for `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        Self {
            modes: MODES.map(|(hz, q, _)| Resonator::new(rate, hz, q)),
        }
    }

    /// Silence the resonators.
    pub fn reset(&mut self) {
        for mode in &mut self.modes {
            mode.z1 = 0.0;
            mode.z2 = 0.0;
        }
    }

    /// The dry sample plus `amount` (0 to 1) of the body's ring.
    pub fn process(&mut self, x: f32, amount: f32) -> f32 {
        let amount = if amount.is_finite() {
            amount.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut wet = 0.0;
        for (mode, &(_, _, weight)) in self.modes.iter_mut().zip(&MODES) {
            wet = weight.mul_add(mode.process(x), wet);
        }
        let out = (amount * DRIVE).mul_add(wet, x);
        if out.is_finite() {
            out
        } else {
            self.reset();
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 48_000.0;

    fn gain_at(hz: f32, amount: f32) -> f32 {
        let mut body = Body::new(RATE);
        let w = TAU * hz / RATE;
        let out: Vec<f32> = (0..24_000)
            .map(|n| body.process((w * n as f32).sin(), amount))
            .collect();
        out[12_000..].iter().fold(0.0_f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn zero_body_is_transparent() {
        for hz in [60.0, 98.0, 1_000.0] {
            assert!((gain_at(hz, 0.0) - 1.0).abs() < 1.0e-3, "{hz}");
        }
    }

    #[test]
    fn resonances_lift_their_own_frequencies() {
        assert!(gain_at(98.0, 1.0) > 1.0 + 1.0);
        assert!(gain_at(205.0, 1.0) > gain_at(3_000.0, 1.0) + 0.5);
        // Far from any mode the dry sound passes almost untouched.
        assert!((gain_at(5_000.0, 1.0) - 1.0).abs() < 0.1);
    }

    #[test]
    fn hostile_input_recovers() {
        let mut body = Body::new(RATE);
        assert!(body.process(f32::NAN, 1.0).abs() < f32::EPSILON);
        assert!(body.process(f32::INFINITY, f32::NAN).is_finite());
        assert!((body.process(0.5, f32::NAN) - 0.5).abs() < 1.0e-3);
        let ok = body.process(0.25, 1.0);
        assert!(ok.is_finite());
    }

    #[test]
    fn odd_sample_rates_are_safe() {
        for rate in [f32::NAN, -1.0, 0.0, 8_000.0, 192_000.0] {
            let mut body = Body::new(rate);
            assert!((0..1_000).all(|n| body.process((n as f32).sin(), 1.0).is_finite()));
        }
    }
}
