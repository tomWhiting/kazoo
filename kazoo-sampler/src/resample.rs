//! High-quality sample-rate conversion: a Kaiser-windowed sinc.
//!
//! Each output sample is the input convolved with a low-pass sinc centred on
//! the output's exact position in the input. The position is kept as an
//! exact rational (the two rates reduced by their greatest common divisor),
//! so a long file never drifts. The sinc is windowed by a Kaiser window
//! (β = 9, about 90 dB of stopband rejection) spanning 48 zero crossings
//! either side, and read from a finely tabulated copy with linear
//! interpolation between table points.
//!
//! The cutoff sits at 92 % of the lower of the two Nyquist frequencies, so
//! everything up to about 20 kHz passes flat at 48 kHz and nothing above
//! the new Nyquist folds back when the rate drops. This runs off the audio
//! thread only: it allocates its table and its output.

use std::f64::consts::PI;

use crate::{Error, Result};

/// Zero crossings of the sinc kept either side of the centre.
pub const ZERO_CROSSINGS: usize = 48;

/// Table points per zero crossing.
const STEPS: usize = 256;

/// The Kaiser window's shape: higher trades a wider transition band for
/// deeper stopband rejection.
const BETA: f64 = 9.0;

/// Where the cutoff sits, as a fraction of the lower Nyquist frequency.
const PASSBAND: f64 = 0.92;

/// A converter from one rate to another, reusable across channels.
#[derive(Debug, Clone)]
pub struct Resampler {
    /// Input rate, reduced by the common divisor.
    from: u64,
    /// Output rate, reduced by the common divisor.
    to: u64,
    /// Cutoff as a fraction of the input's Nyquist frequency.
    cutoff: f64,
    /// Input samples read either side of the centre.
    reach: i64,
    /// The windowed sinc from 0 to `ZERO_CROSSINGS`, `STEPS` points per
    /// crossing, plus two guard points.
    table: Vec<f32>,
}

impl Resampler {
    /// A converter from `from` to `to` samples per second (or any pair of
    /// positive whole numbers in that ratio). Allocates its table.
    pub fn new(from: u32, to: u32) -> Result<Self> {
        if from == 0 || to == 0 {
            return Err(Error::BadAudio {
                reason: format!("cannot convert between rates {from} and {to}"),
            });
        }
        let divisor = gcd(u64::from(from), u64::from(to));
        let (from, to) = (u64::from(from) / divisor, u64::from(to) / divisor);
        let cutoff = PASSBAND * (to as f64 / from as f64).min(1.0);
        let reach = (ZERO_CROSSINGS as f64 / cutoff).ceil() as i64 + 1;
        Ok(Self {
            from,
            to,
            cutoff,
            reach,
            table: kernel_table(),
        })
    }

    /// How many output samples `input_len` input samples become.
    #[must_use]
    pub fn output_len(&self, input_len: usize) -> usize {
        let scaled = (input_len as u128) * u128::from(self.to);
        usize::try_from(scaled.div_ceil(u128::from(self.from))).unwrap_or(usize::MAX)
    }

    /// Convert one channel. Samples before the start and after the end are
    /// taken as silence. Allocates the output.
    #[must_use]
    pub fn process(&self, input: &[f32]) -> Vec<f32> {
        if self.from == self.to {
            return input.to_vec();
        }
        let out_len = self.output_len(input.len());
        let mut output = Vec::with_capacity(out_len);
        let last = input.len() as i64 - 1;
        let mut numerator: u128 = 0;
        for _ in 0..out_len {
            let whole = (numerator / u128::from(self.to)) as i64;
            let frac = (numerator % u128::from(self.to)) as f64 / self.to as f64;
            let first = (whole - self.reach + 1).max(0);
            let end = (whole + self.reach).min(last);
            let mut acc = 0.0f64;
            let mut index = first;
            while index <= end {
                let distance = ((index - whole) as f64 - frac).abs() * self.cutoff;
                acc += f64::from(input[index as usize]) * self.window(distance);
                index += 1;
            }
            output.push((acc * self.cutoff) as f32);
            numerator += u128::from(self.from);
        }
        output
    }

    /// The windowed sinc at `distance` zero crossings from the centre.
    fn window(&self, distance: f64) -> f64 {
        let position = distance * STEPS as f64;
        let index = position as usize;
        if index >= ZERO_CROSSINGS * STEPS {
            return 0.0;
        }
        let frac = position - index as f64;
        let a = f64::from(self.table[index]);
        let b = f64::from(self.table[index + 1]);
        (b - a).mul_add(frac, a)
    }
}

/// Taps either side of the centre of the half-band decimator.
const HALVER_REACH: usize = 48;

/// The decimator's cutoff, as a fraction of the input's Nyquist frequency
/// (the output's Nyquist is 0.5).
const HALVER_CUTOFF: f64 = 0.44;

/// A decimator by two: low-pass below the new Nyquist frequency, then keep
/// every other sample. Used to build the band-limited copies a voice reads
/// when pitched up; much cheaper than [`Resampler`] for this one ratio.
#[derive(Debug, Clone)]
pub(crate) struct Halver {
    taps: Vec<f32>,
}

impl Halver {
    /// A decimator with a Kaiser-windowed sinc of `2 * HALVER_REACH + 1`
    /// taps. Allocates.
    pub(crate) fn new() -> Self {
        let norm = bessel_i0(BETA);
        let reach = HALVER_REACH as f64;
        let mut taps: Vec<f64> = (0..=2 * HALVER_REACH)
            .map(|i| {
                let x = i as f64 - reach;
                let arg = PI * HALVER_CUTOFF * x;
                let sinc = if i == HALVER_REACH {
                    1.0
                } else {
                    arg.sin() / arg
                };
                let ratio = x / (reach + 1.0);
                sinc * bessel_i0(BETA * ratio.mul_add(-ratio, 1.0).max(0.0).sqrt()) / norm
            })
            .collect();
        let sum: f64 = taps.iter().sum();
        for tap in &mut taps {
            *tap /= sum;
        }
        Self {
            taps: taps.into_iter().map(|tap| tap as f32).collect(),
        }
    }

    /// Half as many samples, band-limited: output `m` sits at input `2m`.
    /// Allocates the output.
    pub(crate) fn process(&self, input: &[f32]) -> Vec<f32> {
        let reach = HALVER_REACH as i64;
        let last = input.len() as i64 - 1;
        (0..input.len().div_ceil(2))
            .map(|m| {
                let centre = 2 * m as i64;
                let first = (centre - reach).max(0);
                let end = (centre + reach).min(last);
                let mut acc = 0.0f32;
                let mut index = first;
                while index <= end {
                    let tap = self.taps[(index - centre + reach) as usize];
                    acc = tap.mul_add(input[index as usize], acc);
                    index += 1;
                }
                acc
            })
            .collect()
    }
}

/// Convert one channel from `from` to `to` samples per second.
pub fn resample(input: &[f32], from: u32, to: u32) -> Result<Vec<f32>> {
    Ok(Resampler::new(from, to)?.process(input))
}

/// The windowed sinc tabulated from the centre outwards.
fn kernel_table() -> Vec<f32> {
    let points = ZERO_CROSSINGS * STEPS;
    let norm = bessel_i0(BETA);
    (0..points + 2)
        .map(|i| {
            if i >= points {
                return 0.0;
            }
            let x = i as f64 / STEPS as f64;
            let sinc = if i == 0 {
                1.0
            } else {
                (PI * x).sin() / (PI * x)
            };
            let ratio = x / ZERO_CROSSINGS as f64;
            let window = bessel_i0(BETA * ratio.mul_add(-ratio, 1.0).max(0.0).sqrt()) / norm;
            (sinc * window) as f32
        })
        .collect()
}

/// The zeroth-order modified Bessel function of the first kind, by its
/// power series (converges fast for the arguments a Kaiser window uses).
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let mut sum = 1.0;
    let mut term = 1.0;
    let mut k = 1.0;
    while term > sum * 1e-17 {
        term *= (half / k) * (half / k);
        sum += term;
        k += 1.0;
    }
    sum
}

/// Greatest common divisor.
const fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let rest = a % b;
        a = b;
        b = rest;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, rate: u32, seconds: f64) -> Vec<f32> {
        let len = (f64::from(rate) * seconds) as usize;
        (0..len)
            .map(|n| (0.5 * (2.0 * PI * hz * n as f64 / f64::from(rate)).sin()) as f32)
            .collect()
    }

    /// Amplitude of the `hz` component of `signal` (Goertzel over a Hann
    /// window, scaled so a full-length sine of amplitude A reads A).
    fn amplitude(signal: &[f32], hz: f64, rate: u32) -> f64 {
        let len = signal.len() as f64;
        let omega = 2.0 * PI * hz / f64::from(rate);
        let (mut re, mut im, mut window_sum) = (0.0, 0.0, 0.0);
        for (n, &x) in signal.iter().enumerate() {
            let window = 0.5 - 0.5 * (2.0 * PI * n as f64 / len).cos();
            window_sum += window;
            let v = f64::from(x) * window;
            re += v * (omega * n as f64).cos();
            im -= v * (omega * n as f64).sin();
        }
        2.0 * re.hypot(im) / window_sum
    }

    /// The frequency of `signal`, from its upward zero crossings,
    /// interpolated between samples.
    fn frequency(signal: &[f32], rate: u32) -> f64 {
        let mut crossings = Vec::new();
        for n in 1..signal.len() {
            let (a, b) = (f64::from(signal[n - 1]), f64::from(signal[n]));
            if a < 0.0 && b >= 0.0 {
                crossings.push((n - 1) as f64 + a / (a - b));
            }
        }
        let span = crossings[crossings.len() - 1] - crossings[0];
        (crossings.len() - 1) as f64 * f64::from(rate) / span
    }

    /// The middle of a signal, clear of the filter's edges.
    fn interior(signal: &[f32]) -> &[f32] {
        let edge = signal.len() / 8;
        &signal[edge..signal.len() - edge]
    }

    #[test]
    fn equal_rates_copy_exactly() {
        let input = sine(440.0, 48_000, 0.1);
        assert_eq!(resample(&input, 48_000, 48_000).unwrap(), input);
    }

    #[test]
    fn lengths_follow_the_ratio() {
        let resampler = Resampler::new(44_100, 48_000).unwrap();
        assert_eq!(resampler.output_len(44_100), 48_000);
        assert_eq!(resampler.output_len(1), 2);
        assert_eq!(resampler.output_len(0), 0);
        assert!(Resampler::new(0, 48_000).is_err());
    }

    #[test]
    fn a_sine_keeps_its_frequency_and_level() {
        for (from, to) in [
            (44_100, 48_000),
            (48_000, 44_100),
            (22_050, 96_000),
            (96_000, 32_000),
        ] {
            let input = sine(1_000.0, from, 1.0);
            let output = resample(&input, from, to).unwrap();
            let hz = frequency(interior(&output), to);
            assert!((hz - 1_000.0).abs() < 0.01, "{from} -> {to}: {hz} Hz");
            let level = amplitude(interior(&output), 1_000.0, to);
            assert!((level - 0.5).abs() < 0.005, "{from} -> {to}: level {level}");
        }
    }

    #[test]
    fn downsampling_does_not_alias() {
        // 30 kHz at 96 kHz would fold to 18 kHz at 48 kHz.
        let input = sine(30_000.0, 96_000, 0.5);
        let output = resample(&input, 96_000, 48_000).unwrap();
        let folded = amplitude(interior(&output), 18_000.0, 48_000);
        assert!(folded < 0.5 * 1e-4, "alias at {folded}");
        let peak = interior(&output).iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak < 1e-3, "residue peak {peak}");
    }

    #[test]
    fn upsampling_leaves_no_images() {
        // 15 kHz at 44.1 kHz images at 29.1 kHz when upsampled badly.
        let input = sine(15_000.0, 44_100, 0.5);
        let output = resample(&input, 44_100, 96_000).unwrap();
        let wanted = amplitude(interior(&output), 15_000.0, 96_000);
        let image = amplitude(interior(&output), 29_100.0, 96_000);
        assert!((wanted - 0.5).abs() < 0.01, "{wanted}");
        assert!(image < 0.5 * 1e-4, "image at {image}");
    }

    #[test]
    fn halving_keeps_the_band_and_drops_what_would_fold() {
        let halver = Halver::new();
        let low = sine(4_000.0, 48_000, 0.5);
        let kept = halver.process(&low);
        assert_eq!(kept.len(), low.len() / 2);
        let level = amplitude(interior(&kept), 4_000.0, 24_000);
        assert!((level - 0.5).abs() < 0.005, "{level}");
        // 15 kHz at 48 kHz folds to 9 kHz at 24 kHz.
        let high = sine(15_000.0, 48_000, 0.5);
        let folded = amplitude(interior(&halver.process(&high)), 9_000.0, 24_000);
        assert!(folded < 0.5 * 1e-3, "{folded}");
    }

    #[test]
    fn the_kernel_is_a_proper_low_pass() {
        let table = kernel_table();
        assert!((table[0] - 1.0).abs() < 1e-6);
        // Zero at every crossing but the centre.
        for crossing in 1..ZERO_CROSSINGS {
            assert!(table[crossing * STEPS].abs() < 1e-6);
        }
        assert!(bessel_i0(0.0).eq(&1.0));
        assert_eq!(Halver::new().taps.len(), 2 * HALVER_REACH + 1);
        assert_eq!(gcd(44_100, 48_000), 300);
    }
}
