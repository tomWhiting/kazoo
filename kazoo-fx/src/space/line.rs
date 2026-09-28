//! A delay line read between samples through a windowed sinc, for the
//! reverbs' tanks.
//!
//! A reverb tank reads its delay lines between samples (the lengths are
//! scaled to the size knob and the sample rate, and some are swept), and
//! every pass round the loop goes through those reads again. A four-point
//! Hermite read, like [`crate::dsp::DelayLine`]'s, loses about a decibel at
//! half Nyquist on every read between samples, so a tank that reads a line a dozen times a
//! pass would lose its treble dozens of times faster at 44.1 kHz than at
//! 192 kHz, where the same treble sits much lower in the band. These lines
//! read through a Kaiser-windowed sinc instead (the textbook band-limited
//! interpolator, Smith and Gossett's "flexible sampling-rate conversion",
//! 1984): exact on a whole sample and, between them, flat to within a
//! tenth of a decibel up to 20 kHz (or 80% of Nyquist, 17.6 kHz at
//! 44.1 kHz), so the tank's treble decays the same at every rate.
//!
//! The kernel is as long as the rate needs: thirty-two points at 44.1 or
//! 48 kHz, sixteen at 88.2 or 96 kHz, eight above (where 20 kHz sits low in
//! the band), so the cost in real time stays about the same at every rate.
//! It is tabulated at 256 fractions and blended linearly between them; the
//! table is built once in `prepare` and shared by every line of an effect.

use std::sync::Arc;

/// The most samples either side of the read point any kernel uses.
const MOST_HALF: usize = 16;
/// Fractions tabulated between one sample and the next.
const PHASES: usize = 256;
/// The kernel window's beta.
const BETA: f64 = 7.0;

/// The shortest delay a [`Line`] reads at: the longest kernel's newest tap
/// must be one already written.
pub const MIN_SINC_READ: f32 = MOST_HALF as f32;

/// The tabulated interpolation kernel.
#[derive(Debug)]
pub struct Kernel {
    /// Samples either side of the read point.
    half: usize,
    /// For fraction `p / PHASES`, the `2 half` weights starting at
    /// `p * 2 half`: weight `k` goes to the sample `k - (half - 1)` samples
    /// older than the whole part of the delay.
    weights: Vec<f32>,
}

/// The zeroth-order modified Bessel function of the first kind.
fn bessel_i0(x: f64) -> f64 {
    let quarter = 0.25 * x * x;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..64 {
        let k = f64::from(k);
        term *= quarter / (k * k);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

impl Kernel {
    /// Build the table for `rate`. Allocates.
    #[must_use]
    pub fn new(rate: f32) -> Arc<Self> {
        let half = if rate <= 50_000.0 {
            MOST_HALF
        } else if rate <= 100_000.0 {
            MOST_HALF / 2
        } else {
            MOST_HALF / 4
        };
        let taps = 2 * half;
        let norm = bessel_i0(BETA);
        let mut weights = Vec::with_capacity((PHASES + 1) * taps);
        for p in 0..=PHASES {
            let fraction = p as f64 / PHASES as f64;
            let row: Vec<f64> = (0..taps)
                .map(|k| {
                    let x = k as f64 - (half - 1) as f64 - fraction;
                    let sinc = if x.abs() < 1e-12 {
                        1.0
                    } else {
                        let angle = std::f64::consts::PI * x;
                        angle.sin() / angle
                    };
                    let ratio = x / half as f64;
                    sinc * bessel_i0(BETA * (1.0 - ratio * ratio).max(0.0).sqrt()) / norm
                })
                .collect();
            // Unity gain at DC for every fraction.
            let sum: f64 = row.iter().sum();
            weights.extend(row.iter().map(|weight| (weight / sum) as f32));
        }
        Arc::new(Self { half, weights })
    }
}

/// A circular delay line with windowed-sinc reads.
#[derive(Debug, Clone, Default)]
pub struct Line {
    buffer: Vec<f32>,
    mask: usize,
    write: usize,
    kernel: Option<Arc<Kernel>>,
}

impl Line {
    /// Room for at least `max_samples` of delay, read through `kernel`.
    /// Allocates.
    #[must_use]
    pub fn new(max_samples: usize, kernel: &Arc<Kernel>) -> Self {
        let size = max_samples
            .saturating_add(2 * MOST_HALF + 4)
            .next_power_of_two();
        Self {
            buffer: vec![0.0; size],
            mask: size - 1,
            write: 0,
            kernel: Some(Arc::clone(kernel)),
        }
    }

    /// The longest delay [`Self::read`] can reach.
    fn max_delay(&self) -> f32 {
        self.buffer.len().saturating_sub(2 * MOST_HALF + 2) as f32
    }

    /// Write the next sample. A NaN or infinity is written as silence.
    pub fn push(&mut self, sample: f32) {
        if self.buffer.is_empty() {
            return;
        }
        self.buffer[self.write] = if sample.is_finite() { sample } else { 0.0 };
        self.write = (self.write + 1) & self.mask;
    }

    /// The sample written exactly `delay` samples ago (1 is the newest),
    /// held to what the line holds.
    #[must_use]
    pub fn tap(&self, delay: usize) -> f32 {
        if self.buffer.is_empty() {
            return 0.0;
        }
        let delay = delay.clamp(1, self.buffer.len() - 1);
        self.buffer[self.write.wrapping_sub(delay) & self.mask]
    }

    /// The signal `delay` samples ago, between samples, held between
    /// [`MIN_SINC_READ`] and what the line holds.
    #[must_use]
    pub fn read(&self, delay: f32) -> f32 {
        let Some(kernel) = &self.kernel else {
            return 0.0;
        };
        if self.buffer.is_empty() {
            return 0.0;
        }
        let delay = if delay.is_finite() {
            delay.clamp(MIN_SINC_READ, self.max_delay().max(MIN_SINC_READ))
        } else {
            MIN_SINC_READ
        };
        let whole = delay.floor();
        let position = (delay - whole) * PHASES as f32;
        let phase = (position as usize).min(PHASES - 1);
        let blend = position - phase as f32;
        let taps = 2 * kernel.half;
        let early = &kernel.weights[phase * taps..(phase + 1) * taps];
        let late = &kernel.weights[(phase + 1) * taps..(phase + 2) * taps];
        // The newest tap is half - 1 samples newer than the whole delay.
        let newest = self.write.wrapping_sub(whole as usize - (kernel.half - 1));
        let mut sum = 0.0f32;
        for (k, (&early, &late)) in early.iter().zip(late).enumerate() {
            let sample = self.buffer[newest.wrapping_sub(k) & self.mask];
            sum = sample.mul_add(blend.mul_add(late - early, early), sum);
        }
        sum
    }

    /// Silence the line.
    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_on_a_whole_sample_is_that_sample() {
        let kernel = Kernel::new(48_000.0);
        let mut line = Line::new(64, &kernel);
        for n in 0..64 {
            line.push(n as f32);
        }
        // The newest is 63, one sample ago.
        assert!((line.read(20.0) - 44.0).abs() < 1e-3, "{}", line.read(20.0));
        assert!((line.read(30.5) - 33.5).abs() < 1e-3, "{}", line.read(30.5));
    }

    #[test]
    fn a_read_between_samples_keeps_the_treble() {
        // A tone at 80% of Nyquist read half way between samples: its level
        // (in power, since the samples of so high a tone miss its peaks)
        // holds to within a tenth of a decibel.
        let kernel = Kernel::new(48_000.0);
        let mut line = Line::new(256, &kernel);
        let omega = 0.8 * std::f32::consts::PI;
        let (mut written, mut read) = (0.0f64, 0.0f64);
        for n in 0..4_000 {
            let x = (omega * n as f32).sin();
            line.push(x);
            if n > 100 {
                let y = line.read(40.5);
                written = f64::from(x).mul_add(f64::from(x), written);
                read = f64::from(y).mul_add(f64::from(y), read);
            }
        }
        let level = 10.0 * (read / written).log10();
        assert!(level.abs() < 0.1, "{level}");
    }

    #[test]
    fn every_rate_s_kernel_is_flat_to_the_top_of_the_band() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
            let kernel = Kernel::new(rate);
            let mut line = Line::new(1_024, &kernel);
            // 20 kHz, or 80% of Nyquist where that is lower.
            let top = 20_000.0f32.min(0.4 * rate);
            let omega = std::f32::consts::TAU * top / rate;
            let (mut written, mut read) = (0.0f64, 0.0f64);
            for n in 0..20_000 {
                let x = (omega * n as f32).sin();
                line.push(x);
                if n > 200 {
                    let y = line.read(60.5);
                    written = f64::from(x).mul_add(f64::from(x), written);
                    read = f64::from(y).mul_add(f64::from(y), read);
                }
            }
            let level = 10.0 * (read / written).log10();
            assert!(level.abs() < 0.1, "{rate} Hz: {level} dB at {top} Hz");
        }
    }

    #[test]
    fn poison_and_bad_delays_are_refused() {
        let kernel = Kernel::new(48_000.0);
        let mut line = Line::new(32, &kernel);
        line.push(f32::NAN);
        line.push(f32::INFINITY);
        assert!(line.read(f32::NAN).is_finite());
        assert!(line.read(1.0e9).is_finite());
        assert!(Line::default().read(10.0).abs() < f32::EPSILON);
    }
}
