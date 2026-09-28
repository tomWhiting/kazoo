//! The filter banks a vocoder is built from.
//!
//! Bands are laid out evenly on the Bark scale, the ear's own critical-band
//! scale, so the low bands are narrow where vowels live and the high bands
//! are wide where consonants hiss. Every band is a fourth-order bandpass:
//! two identical second-order sections in cascade, each widened so that the
//! pair keeps the band's intended -3 dB width.
//!
//! Everything here is plain arithmetic and never allocates, so the carrier
//! bank can be re-tuned on the audio thread when the formant shift moves.

use std::f64::consts::TAU;

/// Fewest bands a vocoder can have.
pub const MIN_BANDS: usize = 8;

/// Most bands a vocoder can have.
pub const MAX_BANDS: usize = 40;

/// Lower edge of the lowest band, in Hz.
const LOW_EDGE_HZ: f32 = 80.0;

/// Upper edge of the highest band, in Hz, when the sample rate allows it.
const HIGH_EDGE_HZ: f32 = 11_000.0;

/// Highest fraction of the sample rate a band edge or centre may reach.
const EDGE_LIMIT: f32 = 0.45;

/// Narrowest and widest band a layout may produce, as Q.
const Q_RANGE: (f32, f32) = (1.0, 40.0);

/// Two identical second-order bandpasses in cascade narrow the -3 dB width
/// by `sqrt(sqrt(2) - 1)`; each section's Q is scaled by this so that the
/// cascade has the width the layout asked for.
const CASCADE_Q_SCALE: f32 = 0.643_594_3;

/// Hz to Bark (Traunmüller's formula).
#[must_use]
pub fn hz_to_bark(hz: f32) -> f32 {
    26.81 * hz / (1960.0 + hz) - 0.53
}

/// Bark to Hz, the inverse of [`hz_to_bark`].
#[must_use]
pub fn bark_to_hz(bark: f32) -> f32 {
    1960.0 * (bark + 0.53) / (26.28 - bark)
}

/// Where the bands of one band count sit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    count: usize,
    centres: [f32; MAX_BANDS],
    qs: [f32; MAX_BANDS],
}

impl Layout {
    /// `count` bands (held within [`MIN_BANDS`]..=[`MAX_BANDS`]) spread
    /// evenly in Bark from 80 Hz up to 11 kHz, or up to 45% of the sample
    /// rate when that is lower.
    #[must_use]
    pub fn new(count: usize, sample_rate: f32) -> Self {
        let count = count.clamp(MIN_BANDS, MAX_BANDS);
        let top = HIGH_EDGE_HZ
            .min(sample_rate * EDGE_LIMIT)
            .max(LOW_EDGE_HZ * 2.0);
        let low = hz_to_bark(LOW_EDGE_HZ);
        let step = (hz_to_bark(top) - low) / count as f32;
        let mut centres = [0.0; MAX_BANDS];
        let mut qs = [0.0; MAX_BANDS];
        for band in 0..count {
            let lower = bark_to_hz((band as f32).mul_add(step, low));
            let upper = bark_to_hz((band as f32 + 1.0).mul_add(step, low));
            let centre = bark_to_hz((band as f32 + 0.5).mul_add(step, low));
            centres[band] = centre;
            qs[band] = (centre / (upper - lower)).clamp(Q_RANGE.0, Q_RANGE.1);
        }
        Self { count, centres, qs }
    }

    /// How many bands there are.
    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }

    /// The centre of every band, lowest first.
    #[must_use]
    pub fn centres(&self) -> &[f32] {
        &self.centres[..self.count]
    }

    /// The Q of every band (centre over -3 dB width), lowest first.
    #[must_use]
    pub fn qs(&self) -> &[f32] {
        &self.qs[..self.count]
    }
}

/// Coefficients of one second-order section, normalised so `a0` is 1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Coeffs {
    b0: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coeffs {
    /// One section of a band centred on `centre` Hz whose fourth-order
    /// cascade has quality `q`: a bandpass with unity gain at its centre.
    /// Out-of-range or non-finite inputs are held to something stable.
    #[must_use]
    pub fn band_section(centre: f32, q: f32, sample_rate: f32) -> Self {
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            f64::from(sample_rate)
        } else {
            48_000.0
        };
        let centre = if centre.is_finite() {
            f64::from(centre).clamp(1.0, rate * f64::from(EDGE_LIMIT))
        } else {
            1_000.0
        };
        let q = if q.is_finite() {
            f64::from(q.clamp(Q_RANGE.0, Q_RANGE.1) * CASCADE_Q_SCALE)
        } else {
            1.0
        };
        let omega = TAU * centre / rate;
        let alpha = omega.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: alpha / a0,
            b2: -alpha / a0,
            a1: -2.0 * omega.cos() / a0,
            a2: (1.0 - alpha) / a0,
        }
    }
}

/// The running state of one fourth-order band: two sections in transposed
/// direct form II, kept in `f64` so the narrow low bands stay clean.
#[derive(Debug, Clone, Copy, Default)]
pub struct Band4 {
    state: [[f64; 2]; 2],
}

impl Band4 {
    /// One sample through both sections with the same coefficients.
    pub fn process(&mut self, coeffs: &Coeffs, input: f64) -> f64 {
        let mut x = input;
        for [s1, s2] in &mut self.state {
            let y = coeffs.b0.mul_add(x, *s1);
            *s1 = (-coeffs.a1).mul_add(y, *s2);
            *s2 = coeffs.b2.mul_add(x, -coeffs.a2 * y);
            x = y;
        }
        x
    }

    /// Zero any state that has decayed into the denormal range or gone
    /// non-finite.
    pub fn flush(&mut self) {
        for value in self.state.iter_mut().flatten() {
            if !value.is_finite() || value.abs() < 1e-30 {
                *value = 0.0;
            }
        }
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.state = [[0.0; 2]; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain_at(coeffs: &Coeffs, hz: f32, rate: f32) -> f64 {
        let mut band = Band4::default();
        let mut peak = 0.0f64;
        let total = (rate as usize) / 2;
        for n in 0..total {
            let x = (TAU * f64::from(hz) * n as f64 / f64::from(rate)).sin();
            let y = band.process(coeffs, x);
            if n > total / 2 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn bark_round_trips() {
        for hz in [100.0, 440.0, 1_000.0, 4_000.0, 10_000.0] {
            let back = bark_to_hz(hz_to_bark(hz));
            assert!((back - hz).abs() / hz < 1e-4, "{hz} -> {back}");
        }
    }

    #[test]
    fn layouts_rise_and_stay_under_nyquist() {
        for rate in [8_000.0, 22_050.0, 44_100.0, 48_000.0, 96_000.0] {
            for count in MIN_BANDS..=MAX_BANDS {
                let layout = Layout::new(count, rate);
                assert_eq!(layout.count(), count);
                let centres = layout.centres();
                assert!(centres.windows(2).all(|pair| pair[1] > pair[0]));
                assert!(centres[0] > LOW_EDGE_HZ);
                assert!(*centres.last().unwrap_or(&0.0) < rate * EDGE_LIMIT);
                assert!(layout.qs().iter().all(|q| (1.0..=40.0).contains(q)));
            }
        }
    }

    #[test]
    fn out_of_range_counts_are_held() {
        assert_eq!(Layout::new(0, 48_000.0).count(), MIN_BANDS);
        assert_eq!(Layout::new(1_000, 48_000.0).count(), MAX_BANDS);
    }

    #[test]
    fn a_band_passes_its_centre_and_rejects_far_away() {
        let rate = 48_000.0;
        let coeffs = Coeffs::band_section(1_000.0, 4.0, rate);
        let centre = gain_at(&coeffs, 1_000.0, rate);
        assert!((centre - 1.0).abs() < 0.02, "{centre}");
        // A fourth-order band is far down two octaves away.
        assert!(gain_at(&coeffs, 4_000.0, rate) < 0.02);
        assert!(gain_at(&coeffs, 250.0, rate) < 0.02);
        // The cascade keeps the width the layout asked for: -3 dB at the
        // edges of a Q of 4 around 1 kHz.
        let half = 1_000.0 / 4.0 / 2.0;
        let edge = gain_at(&coeffs, 1_000.0f32.hypot(half) + half, rate);
        assert!(
            (edge - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.05,
            "{edge}"
        );
    }

    #[test]
    fn poisoned_design_inputs_stay_stable() {
        for coeffs in [
            Coeffs::band_section(f32::NAN, 4.0, 48_000.0),
            Coeffs::band_section(1_000.0, f32::INFINITY, 48_000.0),
            Coeffs::band_section(1_000.0, 4.0, f32::NAN),
            Coeffs::band_section(1e9, 0.0, 48_000.0),
        ] {
            let mut band = Band4::default();
            let mut out = 0.0;
            for n in 0..10_000 {
                out = band.process(&coeffs, if n == 0 { 1.0 } else { 0.0 });
            }
            assert!(out.is_finite() && out.abs() < 1e-3);
        }
    }
}
