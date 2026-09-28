//! Three-band channel equaliser for the mixer strips.
//!
//! Each strip carries a console-style EQ: a low shelf, a peaking mid band and a
//! high shelf, each with a fixed corner frequency and a boost/cut range of
//! ±[`EQ_RANGE_DB`]. Coefficients follow the RBJ "Audio EQ Cookbook" and are
//! only recalculated when a band's gain actually changes, so the audio callback
//! pays for trigonometry at most once per control change.
//!
//! Everything here is allocation-free and safe to run inside the output
//! callback. Non-finite input or a filter state that has gone non-finite resets
//! the affected filter and yields silence for that sample.

use std::f32::consts::PI;

/// Maximum boost or cut, in decibels, for every EQ band.
pub const EQ_RANGE_DB: f32 = 15.0;

/// Corner frequency of the low shelf, in hertz.
pub const LOW_SHELF_HZ: f32 = 100.0;

/// Centre frequency of the mid peaking band, in hertz.
pub const MID_PEAK_HZ: f32 = 1_000.0;

/// Corner frequency of the high shelf, in hertz.
pub const HIGH_SHELF_HZ: f32 = 8_000.0;

/// Quality factor of the mid peaking band.
const MID_Q: f32 = 0.7;

/// Shelf slope (S = 1 is the steepest slope without overshoot).
const SHELF_SLOPE: f32 = 1.0;

/// Gains closer to zero than this are treated as flat and bypassed.
const FLAT_EPSILON_DB: f32 = 0.01;

/// Filter state smaller than this is flushed to zero so decaying tails never
/// fall into slow subnormal arithmetic.
const DENORMAL_FLOOR: f32 = 1e-20;

/// Per-band gain settings for one channel strip.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EqSettings {
    /// Low shelf gain in decibels.
    pub low_db: f32,
    /// Mid peaking gain in decibels.
    pub mid_db: f32,
    /// High shelf gain in decibels.
    pub high_db: f32,
}

impl EqSettings {
    /// A flat EQ: every band at 0 dB.
    pub const FLAT: Self = Self {
        low_db: 0.0,
        mid_db: 0.0,
        high_db: 0.0,
    };

    /// Return a copy with every band clamped into the legal range. Non-finite
    /// values become 0 dB.
    #[must_use]
    pub fn clamped(self) -> Self {
        Self {
            low_db: clamp_band(self.low_db),
            mid_db: clamp_band(self.mid_db),
            high_db: clamp_band(self.high_db),
        }
    }
}

fn clamp_band(db: f32) -> f32 {
    if db.is_finite() {
        db.clamp(-EQ_RANGE_DB, EQ_RANGE_DB)
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BandShape {
    LowShelf,
    Peak,
    HighShelf,
}

/// Normalised biquad coefficients (a0 = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Coefficients {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Coefficients {
    const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn design(shape: BandShape, frequency_hz: f32, gain_db: f32, sample_rate: f32) -> Self {
        let nyquist_guard = sample_rate * 0.45;
        let frequency = frequency_hz.clamp(10.0, nyquist_guard.max(10.0));
        let a = 10.0_f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * frequency / sample_rate;
        let (sin_w0, cos_w0) = w0.sin_cos();

        let (b0, b1, b2, a0, a1, a2) = match shape {
            BandShape::Peak => {
                let alpha = sin_w0 / (2.0 * MID_Q);
                (
                    alpha.mul_add(a, 1.0),
                    -2.0 * cos_w0,
                    (-alpha).mul_add(a, 1.0),
                    alpha.mul_add(1.0 / a, 1.0),
                    -2.0 * cos_w0,
                    (-alpha).mul_add(1.0 / a, 1.0),
                )
            }
            BandShape::LowShelf | BandShape::HighShelf => {
                let alpha = sin_w0 / 2.0
                    * (a + 1.0 / a)
                        .mul_add(1.0 / SHELF_SLOPE - 1.0, 2.0)
                        .max(0.0)
                        .sqrt();
                let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
                let ap1 = a + 1.0;
                let am1 = a - 1.0;
                if shape == BandShape::LowShelf {
                    (
                        a * (am1.mul_add(-cos_w0, ap1) + two_sqrt_a_alpha),
                        2.0 * a * ap1.mul_add(-cos_w0, am1),
                        a * (am1.mul_add(-cos_w0, ap1) - two_sqrt_a_alpha),
                        am1.mul_add(cos_w0, ap1) + two_sqrt_a_alpha,
                        -2.0 * ap1.mul_add(cos_w0, am1),
                        am1.mul_add(cos_w0, ap1) - two_sqrt_a_alpha,
                    )
                } else {
                    (
                        a * (am1.mul_add(cos_w0, ap1) + two_sqrt_a_alpha),
                        -2.0 * a * ap1.mul_add(cos_w0, am1),
                        a * (am1.mul_add(cos_w0, ap1) - two_sqrt_a_alpha),
                        am1.mul_add(-cos_w0, ap1) + two_sqrt_a_alpha,
                        2.0 * ap1.mul_add(-cos_w0, am1),
                        am1.mul_add(-cos_w0, ap1) - two_sqrt_a_alpha,
                    )
                }
            }
        };

        if !a0.is_finite() || a0.abs() < f32::EPSILON {
            return Self::IDENTITY;
        }
        let coefficients = Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        };
        if coefficients.is_finite() {
            coefficients
        } else {
            Self::IDENTITY
        }
    }

    const fn is_finite(&self) -> bool {
        self.b0.is_finite()
            && self.b1.is_finite()
            && self.b2.is_finite()
            && self.a1.is_finite()
            && self.a2.is_finite()
    }
}

/// Transposed direct-form II state for one audio channel.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct BiquadState {
    z1: f32,
    z2: f32,
}

impl BiquadState {
    fn process(&mut self, c: &Coefficients, input: f32) -> f32 {
        if !input.is_finite() {
            *self = Self::default();
            return 0.0;
        }
        let output = c.b0.mul_add(input, self.z1);
        self.z1 = flush_denormal(c.b1.mul_add(input, (-c.a1).mul_add(output, self.z2)));
        self.z2 = flush_denormal(c.b2.mul_add(input, -c.a2 * output));
        if output.is_finite() && self.z1.is_finite() && self.z2.is_finite() {
            output
        } else {
            *self = Self::default();
            0.0
        }
    }
}

fn flush_denormal(value: f32) -> f32 {
    if value.abs() < DENORMAL_FLOOR {
        0.0
    } else {
        value
    }
}

/// One EQ band with independent left/right state.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    shape: BandShape,
    frequency_hz: f32,
    gain_db: f32,
    coefficients: Coefficients,
    left: BiquadState,
    right: BiquadState,
}

impl Band {
    const fn new(shape: BandShape, frequency_hz: f32) -> Self {
        Self {
            shape,
            frequency_hz,
            gain_db: 0.0,
            coefficients: Coefficients::IDENTITY,
            left: BiquadState { z1: 0.0, z2: 0.0 },
            right: BiquadState { z1: 0.0, z2: 0.0 },
        }
    }

    fn set_gain(&mut self, gain_db: f32, sample_rate: f32) {
        if (gain_db - self.gain_db).abs() < f32::EPSILON {
            return;
        }
        let was_flat = self.is_flat();
        self.gain_db = gain_db;
        self.coefficients = if self.is_flat() {
            Coefficients::IDENTITY
        } else {
            Coefficients::design(self.shape, self.frequency_hz, gain_db, sample_rate)
        };
        if was_flat {
            // The band was bypassed, so its state is stale; start clean rather
            // than ringing out whatever was left from before the bypass.
            self.left = BiquadState::default();
            self.right = BiquadState::default();
        }
    }

    fn is_flat(&self) -> bool {
        self.gain_db.abs() < FLAT_EPSILON_DB
    }

    fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.is_flat() {
            return (left, right);
        }
        (
            self.left.process(&self.coefficients, left),
            self.right.process(&self.coefficients, right),
        )
    }

    fn reset(&mut self) {
        self.left = BiquadState::default();
        self.right = BiquadState::default();
    }
}

/// Stereo three-band EQ owned by one channel strip.
#[derive(Debug, Clone, PartialEq)]
pub struct StereoEq {
    sample_rate: f32,
    low: Band,
    mid: Band,
    high: Band,
}

impl StereoEq {
    /// Create a flat EQ for the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate >= 1_000.0 {
            sample_rate
        } else {
            48_000.0
        };
        Self {
            sample_rate,
            low: Band::new(BandShape::LowShelf, LOW_SHELF_HZ),
            mid: Band::new(BandShape::Peak, MID_PEAK_HZ),
            high: Band::new(BandShape::HighShelf, HIGH_SHELF_HZ),
        }
    }

    /// Apply new band gains. Coefficients are only recomputed for bands whose
    /// gain changed.
    pub fn set(&mut self, settings: EqSettings) {
        let settings = settings.clamped();
        self.low.set_gain(settings.low_db, self.sample_rate);
        self.mid.set_gain(settings.mid_db, self.sample_rate);
        self.high.set_gain(settings.high_db, self.sample_rate);
    }

    /// Move each band toward `target` by at most `max_step_db`, so a large
    /// change is spread over several calls instead of switching the filter in
    /// one jump.
    pub fn step_toward(&mut self, target: EqSettings, max_step_db: f32) {
        let target = target.clamped();
        let max_step = if max_step_db.is_finite() {
            max_step_db.abs()
        } else {
            0.0
        };
        let step = |current: f32, goal: f32| current + (goal - current).clamp(-max_step, max_step);
        self.set(EqSettings {
            low_db: step(self.low.gain_db, target.low_db),
            mid_db: step(self.mid.gain_db, target.mid_db),
            high_db: step(self.high.gain_db, target.high_db),
        });
    }

    /// Current settings.
    #[must_use]
    pub const fn settings(&self) -> EqSettings {
        EqSettings {
            low_db: self.low.gain_db,
            mid_db: self.mid.gain_db,
            high_db: self.high.gain_db,
        }
    }

    /// Process one stereo frame.
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let (left, right) = self.low.process(left, right);
        let (left, right) = self.mid.process(left, right);
        self.high.process(left, right)
    }

    /// Clear all filter memory.
    pub fn reset(&mut self) {
        self.low.reset();
        self.mid.reset();
        self.high.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: f32 = 48_000.0;

    /// Steady-state peak amplitude of `eq` driven by a unit sine at `hz`.
    fn sine_gain(eq: &mut StereoEq, hz: f32) -> f32 {
        eq.reset();
        let settle = (SAMPLE_RATE * 0.25) as usize;
        let measure = (SAMPLE_RATE * 0.25) as usize;
        let mut peak = 0.0_f32;
        for n in 0..settle + measure {
            let x = (2.0 * PI * hz * n as f32 / SAMPLE_RATE).sin();
            let (left, _) = eq.process(x, x);
            if n >= settle {
                peak = peak.max(left.abs());
            }
        }
        peak
    }

    fn db(linear: f32) -> f32 {
        20.0 * linear.log10()
    }

    #[test]
    fn flat_eq_is_bit_transparent() {
        let mut eq = StereoEq::new(SAMPLE_RATE);
        for n in 0..1_000 {
            let x = (n as f32 * 0.01).sin();
            assert_eq!(eq.process(x, -x), (x, -x));
        }
    }

    #[test]
    fn low_shelf_boost_lifts_bass_and_leaves_treble() {
        let mut eq = StereoEq::new(SAMPLE_RATE);
        eq.set(EqSettings {
            low_db: 12.0,
            ..EqSettings::FLAT
        });
        let bass = db(sine_gain(&mut eq, 30.0));
        let treble = db(sine_gain(&mut eq, 10_000.0));
        assert!((bass - 12.0).abs() < 1.0, "bass gain {bass} dB");
        assert!(treble.abs() < 0.5, "treble gain {treble} dB");
    }

    #[test]
    fn high_shelf_cut_lowers_treble_and_leaves_bass() {
        let mut eq = StereoEq::new(SAMPLE_RATE);
        eq.set(EqSettings {
            high_db: -12.0,
            ..EqSettings::FLAT
        });
        let treble = db(sine_gain(&mut eq, 18_000.0));
        let bass = db(sine_gain(&mut eq, 60.0));
        assert!((treble + 12.0).abs() < 1.0, "treble gain {treble} dB");
        assert!(bass.abs() < 0.5, "bass gain {bass} dB");
    }

    #[test]
    fn mid_peak_hits_target_at_centre_frequency() {
        let mut eq = StereoEq::new(SAMPLE_RATE);
        eq.set(EqSettings {
            mid_db: 9.0,
            ..EqSettings::FLAT
        });
        let centre = db(sine_gain(&mut eq, MID_PEAK_HZ));
        let far = db(sine_gain(&mut eq, 40.0));
        assert!((centre - 9.0).abs() < 0.5, "centre gain {centre} dB");
        assert!(far.abs() < 1.0, "far gain {far} dB");
    }

    #[test]
    fn settings_are_clamped_and_non_finite_is_flat() {
        let mut eq = StereoEq::new(SAMPLE_RATE);
        eq.set(EqSettings {
            low_db: 99.0,
            mid_db: f32::NAN,
            high_db: -99.0,
        });
        assert_eq!(
            eq.settings(),
            EqSettings {
                low_db: EQ_RANGE_DB,
                mid_db: 0.0,
                high_db: -EQ_RANGE_DB,
            }
        );
    }

    #[test]
    fn non_finite_input_yields_silence_and_recovers() {
        let mut eq = StereoEq::new(SAMPLE_RATE);
        eq.set(EqSettings {
            low_db: 6.0,
            mid_db: -3.0,
            high_db: 4.0,
        });
        assert_eq!(eq.process(f32::NAN, f32::INFINITY), (0.0, 0.0));
        for _ in 0..100 {
            let (left, right) = eq.process(0.25, -0.25);
            assert!(left.is_finite() && right.is_finite());
        }
    }

    #[test]
    fn invalid_sample_rate_falls_back() {
        let mut eq = StereoEq::new(f32::NAN);
        eq.set(EqSettings {
            high_db: 6.0,
            ..EqSettings::FLAT
        });
        let (left, _) = eq.process(0.5, 0.5);
        assert!(left.is_finite());
    }

    #[test]
    fn step_toward_moves_in_bounded_steps_and_lands_exactly() {
        let mut eq = StereoEq::new(SAMPLE_RATE);
        let target = EqSettings {
            low_db: 6.0,
            mid_db: -1.2,
            high_db: -15.0,
        };
        eq.step_toward(target, 0.5);
        assert_eq!(
            eq.settings(),
            EqSettings {
                low_db: 0.5,
                mid_db: -0.5,
                high_db: -0.5,
            }
        );
        for _ in 0..40 {
            eq.step_toward(target, 0.5);
        }
        assert_eq!(eq.settings(), target);
        eq.step_toward(EqSettings::FLAT, f32::NAN);
        assert_eq!(eq.settings(), target, "a non-finite step moves nothing");
    }

    #[test]
    fn decaying_state_flushes_to_zero() {
        let mut eq = StereoEq::new(SAMPLE_RATE);
        eq.set(EqSettings {
            low_db: 12.0,
            mid_db: 6.0,
            high_db: -6.0,
        });
        eq.process(1.0, 1.0);
        for _ in 0..200_000 {
            eq.process(0.0, 0.0);
        }
        for band in [&eq.low, &eq.mid, &eq.high] {
            assert_eq!(band.left, BiquadState::default());
            assert_eq!(band.right, BiquadState::default());
        }
    }
}
