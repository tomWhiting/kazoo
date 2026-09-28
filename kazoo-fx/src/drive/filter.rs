//! Linear filters for the drive family, designed the analogue way.
//!
//! A circuit's linear parts (coupling capacitors, an op-amp's gain network,
//! a tone stack) are written as a transfer function in `s` straight from
//! their component values, then mapped to a digital filter with the
//! bilinear transform, `s = c (1 - z⁻¹) / (1 + z⁻¹)`. For a circuit run at
//! its oversampled rate `c` is simply twice that rate (the warping is
//! slight so far above the audio band); filters that are designed by corner
//! frequency (a cabinet, a compressor's sidechain) pre-warp the corner so
//! it lands where it was asked for.
//!
//! Everything here runs in f64: a third-order tone stack at 192 kHz has
//! poles close to 1, where f32 loses the plot.

use std::f64::consts::PI;

use super::kit::flush64;

/// An analogue transfer function up to third order,
/// `(b0 + b1 s + b2 s² + b3 s³) / (a0 + a1 s + a2 s² + a3 s³)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Analogue {
    /// Numerator: `b[k]` multiplies `s^k`.
    pub b: [f64; 4],
    /// Denominator: `a[k]` multiplies `s^k`.
    pub a: [f64; 4],
}

impl Analogue {
    /// A first-order lowpass with its corner at `omega` rad/s.
    #[must_use]
    pub fn lowpass1(omega: f64) -> Self {
        Self {
            b: [1.0, 0.0, 0.0, 0.0],
            a: [1.0, 1.0 / omega, 0.0, 0.0],
        }
    }

    /// A first-order highpass with its corner at `omega` rad/s.
    #[must_use]
    pub fn highpass1(omega: f64) -> Self {
        Self {
            b: [0.0, 1.0 / omega, 0.0, 0.0],
            a: [1.0, 1.0 / omega, 0.0, 0.0],
        }
    }

    /// A second-order lowpass at `omega` rad/s with quality `q`.
    #[must_use]
    pub fn lowpass2(omega: f64, q: f64) -> Self {
        Self {
            b: [1.0, 0.0, 0.0, 0.0],
            a: [1.0, 1.0 / (q * omega), 1.0 / (omega * omega), 0.0],
        }
    }

    /// A second-order highpass at `omega` rad/s with quality `q`.
    #[must_use]
    pub fn highpass2(omega: f64, q: f64) -> Self {
        Self {
            b: [0.0, 0.0, 1.0 / (omega * omega), 0.0],
            a: [1.0, 1.0 / (q * omega), 1.0 / (omega * omega), 0.0],
        }
    }

    /// A second-order peak (or dip) of linear `gain` at `omega` rad/s.
    #[must_use]
    pub fn peak(omega: f64, q: f64, gain: f64) -> Self {
        let root = gain.max(1e-6).sqrt();
        Self {
            b: [1.0, root / (q * omega), 1.0 / (omega * omega), 0.0],
            a: [1.0, 1.0 / (root * q * omega), 1.0 / (omega * omega), 0.0],
        }
    }

    /// The highest power of `s` in use.
    fn order(&self) -> usize {
        (0..4)
            .rev()
            .find(|&k| self.a[k].abs() > 0.0 || self.b[k].abs() > 0.0)
            .unwrap_or(0)
    }
}

/// `hz` as the pre-warped analogue corner (rad/s) that the bilinear
/// transform with `c = 2 rate` maps back onto `hz`. Held below Nyquist.
#[must_use]
pub fn warp(hz: f64, rate: f64) -> f64 {
    let hz = hz.clamp(1.0, rate * 0.49);
    2.0 * rate * (PI * hz / rate).tan()
}

/// The coefficients of `(1 - x)^minus (1 + x)^plus`, lowest power first.
fn binomial(minus: usize, plus: usize) -> [f64; 4] {
    let mut poly = [1.0, 0.0, 0.0, 0.0];
    let factors = std::iter::repeat_n(-1.0f64, minus).chain(std::iter::repeat_n(1.0f64, plus));
    for sign in factors {
        for i in (1..4).rev() {
            poly[i] = sign.mul_add(poly[i - 1], poly[i]);
        }
    }
    poly
}

/// A digital filter up to third order in direct form I.
#[derive(Debug, Clone, Copy)]
pub struct Iir3 {
    b: [f64; 4],
    a: [f64; 4],
    /// The last three inputs and outputs, newest first: direct form I, so
    /// the state is the signal itself and a coefficient change mid-note
    /// (a knob sweeping) acts on real past samples rather than on
    /// internal sums built from the old coefficients.
    inputs: [f64; 3],
    outputs: [f64; 3],
}

impl Default for Iir3 {
    fn default() -> Self {
        Self::new()
    }
}

impl Iir3 {
    /// A filter that passes everything unchanged.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            b: [1.0, 0.0, 0.0, 0.0],
            a: [1.0, 0.0, 0.0, 0.0],
            inputs: [0.0; 3],
            outputs: [0.0; 3],
        }
    }

    /// Become `analogue` through the bilinear transform with constant `c`
    /// (twice the sample rate). The state is kept, so a knob sweep does not
    /// click. A transfer function that will not map (a zero or non-finite
    /// leading denominator) leaves the filter as it was.
    pub fn design(&mut self, analogue: &Analogue, c: f64) {
        let order = analogue.order();
        let mut b = [0.0; 4];
        let mut a = [0.0; 4];
        let mut power = 1.0;
        for k in 0..=order {
            let poly = binomial(k, order - k);
            for i in 0..=order {
                b[i] = (analogue.b[k] * power).mul_add(poly[i], b[i]);
                a[i] = (analogue.a[k] * power).mul_add(poly[i], a[i]);
            }
            power *= c;
        }
        let lead = a[0];
        if lead.abs() < 1e-300 || !lead.is_finite() {
            return;
        }
        let scaled_b = b.map(|value| value / lead);
        let scaled_a = a.map(|value| value / lead);
        if scaled_b
            .iter()
            .chain(&scaled_a)
            .all(|value| value.is_finite())
        {
            self.b = scaled_b;
            self.a = scaled_a;
        }
    }

    /// One sample through the filter.
    pub fn process(&mut self, input: f64) -> f64 {
        let [b0, b1, b2, b3] = self.b;
        let [_, a1, a2, a3] = self.a;
        let [x1, x2, x3] = self.inputs;
        let [y1, y2, y3] = self.outputs;
        let feed = b3.mul_add(x3, b2.mul_add(x2, b1.mul_add(x1, b0 * input)));
        let back = a3.mul_add(y3, a2.mul_add(y2, a1 * y1));
        let mut out = feed - back;
        flush64(&mut out);
        self.inputs = [input, x1, x2];
        self.outputs = [out, y1, y2];
        out
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.inputs = [0.0; 3];
        self.outputs = [0.0; 3];
    }

    /// The magnitude of the response at `hz` for sample rate `rate`.
    #[cfg(test)]
    #[must_use]
    pub fn magnitude(&self, hz: f64, rate: f64) -> f64 {
        let w = 2.0 * PI * hz / rate;
        let eval = |c: &[f64; 4]| {
            let (mut re, mut im) = (0.0, 0.0);
            for (k, coeff) in c.iter().enumerate() {
                let angle = -(k as f64) * w;
                re = coeff.mul_add(angle.cos(), re);
                im = coeff.mul_add(angle.sin(), im);
            }
            re.hypot(im)
        };
        eval(&self.b) / eval(&self.a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_designed_lowpass_lands_its_corner() {
        let rate = 48_000.0;
        let mut filter = Iir3::new();
        filter.design(
            &Analogue::lowpass2(warp(1_000.0, rate), std::f64::consts::FRAC_1_SQRT_2),
            2.0 * rate,
        );
        let corner = filter.magnitude(1_000.0, rate);
        assert!(
            (corner - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-3,
            "{corner}"
        );
        assert!((filter.magnitude(10.0, rate) - 1.0).abs() < 1e-3);
        assert!(filter.magnitude(20_000.0, rate) < 0.01);
    }

    #[test]
    fn a_first_order_highpass_blocks_dc() {
        let rate = 192_000.0;
        let mut filter = Iir3::new();
        filter.design(&Analogue::highpass1(2.0 * PI * 20.0), 2.0 * rate);
        let mut out = 1.0;
        for _ in 0..200_000 {
            out = filter.process(1.0);
        }
        assert!(out.abs() < 1e-3, "{out}");
    }

    #[test]
    fn a_peak_reaches_its_gain() {
        let rate = 48_000.0;
        let mut filter = Iir3::new();
        filter.design(&Analogue::peak(warp(2_000.0, rate), 1.0, 2.0), 2.0 * rate);
        assert!((filter.magnitude(2_000.0, rate) - 2.0).abs() < 1e-3);
    }

    #[test]
    fn nonsense_leaves_the_filter_alone() {
        let mut filter = Iir3::new();
        filter.design(
            &Analogue {
                b: [f64::NAN; 4],
                a: [0.0; 4],
            },
            96_000.0,
        );
        assert!((filter.process(0.5) - 0.5).abs() < 1e-12);
    }
}
