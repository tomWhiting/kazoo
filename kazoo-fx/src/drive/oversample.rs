//! Oversampling to a target rate, with linear-phase halfband stages.
//!
//! Distortion makes harmonics, and any harmonic above half the sample rate
//! folds back down as an inharmonic whine. The circuit models therefore run
//! at a high internal rate: the signal is interpolated up, distorted, then
//! filtered and decimated back down, so the harmonics that would fold are
//! removed before they can.
//!
//! What matters is the internal rate, not the factor, so each effect picks
//! the smallest power of two that takes the host's rate to its target
//! (at least [`TARGET_RATE`], 176.4 kHz): 4x at 44.1 and 48 kHz, 2x at
//! 88.2 and 96 kHz, and none at 176.4 and 192 kHz, where the host's own
//! rate already is the oversampled rate. A model therefore sounds and
//! aliases much the same at every common host rate, and costs the same.
//! A model that needs more (a hard folder, a fuzz) asks for a multiple of
//! the target. The factor stops at 16x, so below 11.025 kHz the internal
//! rate falls short of [`TARGET_RATE`], and below 44.1 kHz short of four
//! times it.
//!
//! Each 2x step is a halfband FIR: a windowed-sinc lowpass at a quarter of
//! the higher rate. Every other tap of a halfband is zero and the centre
//! tap is exactly one half, so it splits into two phases, one a plain
//! delay, and costs half the multiplies of an ordinary FIR. The window is
//! Kaiser's, with its beta chosen for the stopband.
//!
//! - The first stage (1x to 2x and back) does the real work, and is cut
//!   to the host: flat within 0.001 dB to 20 kHz and 90 dB down from the
//!   mirror of 20 kHz about the host's Nyquist (Kaiser beta 9), so nothing
//!   between 20 kHz and Nyquist can fold into the audible band. That takes
//!   127 taps at 44.1 kHz, 71 at 48 kHz and 23 at 96 kHz; being linear
//!   phase, it delays by half its length, so it is kept no longer than it
//!   must be.
//! - Later stages only have to reject what lies above three quarters of
//!   the band below them: 31 taps and beta 9.5 give about 93 dB there.
//!
//! Sources: Crochiere and Rabiner, "Interpolation and decimation of digital
//! signals" (1981), for polyphase halfbands; Kaiser's window formulae.

use std::f64::consts::PI;

/// The lowest internal rate a model runs at: 176.4 kHz, four times
/// 44.1 kHz.
pub const TARGET_RATE: f32 = 176_400.0;

/// The most a model is ever oversampled.
pub const MAX_FACTOR: usize = 16;

/// How many 2x stages [`MAX_FACTOR`] takes.
const MAX_STAGES: usize = 4;

/// The smallest power of two (up to [`MAX_FACTOR`]) that takes
/// `base_rate` to at least `target`.
#[must_use]
pub fn factor_for(base_rate: f32, target: f32) -> usize {
    let mut factor = 1;
    while factor < MAX_FACTOR && base_rate * (factor as f32) < target * 0.999 {
        factor *= 2;
    }
    factor
}

/// The most even-indexed taps a stage can have.
const MAX_TAPS: usize = 64;

/// A delay history read newest first as one contiguous slice: every sample
/// is written twice, a buffer length apart.
#[derive(Debug, Clone, Copy)]
struct History {
    samples: [f32; 2 * MAX_TAPS],
    newest: usize,
    len: usize,
}

impl History {
    const fn new(len: usize) -> Self {
        Self {
            samples: [0.0; 2 * MAX_TAPS],
            newest: 0,
            len: if len == 0 {
                1
            } else if len > MAX_TAPS {
                MAX_TAPS
            } else {
                len
            },
        }
    }

    const fn push(&mut self, sample: f32) {
        self.newest = if self.newest == 0 {
            self.len - 1
        } else {
            self.newest - 1
        };
        self.samples[self.newest] = sample;
        self.samples[self.newest + self.len] = sample;
    }

    fn recent(&self) -> &[f32] {
        &self.samples[self.newest..self.newest + self.len]
    }

    const fn clear(&mut self) {
        self.samples = [0.0; 2 * MAX_TAPS];
    }
}

/// One halfband 2x stage, both directions: interpolating up and
/// decimating down keep separate histories.
#[derive(Debug, Clone, Copy)]
struct Halfband {
    /// The even-indexed taps `h[0], h[2], … h[4m + 2]`, which sum to one
    /// half; the centre tap `h[2m + 1]` is one half and the other odd taps
    /// are zero.
    taps: [f32; MAX_TAPS],
    count: usize,
    /// `m`: the delay, in low-rate samples, of the centre tap's phase.
    centre: usize,
    up: History,
    down_centre: History,
    down_taps: History,
}

impl Halfband {
    /// A `4m + 3`-tap halfband with Kaiser `beta`.
    fn new(m: usize, beta: f64) -> Self {
        let count = (2 * m + 2).min(MAX_TAPS);
        let length = (4 * m + 3) as f64;
        let centre = (2 * m + 1) as f64;
        let mut taps = [0.0f64; MAX_TAPS];
        for (i, tap) in taps.iter_mut().enumerate().take(count) {
            let index = 2.0 * i as f64;
            let offset = index - centre;
            let sinc = (PI * offset / 2.0).sin() / (PI * offset);
            let place = 2.0 * index / (length - 1.0) - 1.0;
            let window = bessel_i0(beta * (1.0 - place * place).max(0.0).sqrt()) / bessel_i0(beta);
            *tap = sinc * window;
        }
        let total: f64 = taps.iter().sum();
        let scale = if total.abs() > 1e-12 {
            0.5 / total
        } else {
            1.0
        };
        Self {
            taps: taps.map(|tap| (tap * scale) as f32),
            count,
            centre: m,
            up: History::new(count),
            down_centre: History::new(count),
            down_taps: History::new(count),
        }
    }

    fn dot(&self, history: &History) -> f32 {
        self.taps[..self.count]
            .iter()
            .zip(history.recent())
            .fold(0.0, |sum, (tap, sample)| tap.mul_add(*sample, sum))
    }

    /// One low-rate sample in, two high-rate samples out.
    fn up(&mut self, sample: f32) -> [f32; 2] {
        self.up.push(sample);
        let filtered = 2.0 * self.dot(&self.up);
        let delayed = self.up.recent()[self.centre];
        [filtered, delayed]
    }

    /// Two high-rate samples in, one low-rate sample out.
    fn down(&mut self, pair: [f32; 2]) -> f32 {
        self.down_centre.push(pair[0]);
        self.down_taps.push(pair[1]);
        0.5f32.mul_add(
            self.down_centre.recent()[self.centre],
            self.dot(&self.down_taps),
        )
    }

    const fn clear(&mut self) {
        self.up.clear();
        self.down_centre.clear();
        self.down_taps.clear();
    }
}

/// The zeroth-order modified Bessel function of the first kind, by its
/// power series (which converges fast for the betas used here).
fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
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

/// The first stage for a host at `base_rate`: just long enough that
/// everything up to 20 kHz (or 0.45 of the host's rate, if that is lower)
/// passes flat and everything from its mirror about the host's Nyquist is
/// 90 dB down. At 44.1 kHz that takes 127 taps, at 48 kHz 71, and at
/// 88.2 kHz and up the 23-tap minimum: the shorter the stage, the less it
/// delays.
fn first_stage(base_rate: f32) -> Halfband {
    let base = f64::from(base_rate);
    let pass = 20_000f64.min(0.45 * base);
    let transition = (2.0f64.mul_add(-pass, base) / (2.0 * base)).max(0.02);
    let taps = STOPBAND_DB / (2.285 * 2.0 * PI * transition) + 1.0;
    let m = ((taps - 3.0) / 4.0).ceil().clamp(5.0, 31.0) as usize;
    Halfband::new(m, FIRST_BETA)
}

/// The first stage's stopband, in the Kaiser formula's terms (90 dB less
/// its 8), and the window's beta for it.
const STOPBAND_DB: f64 = 82.0;
const FIRST_BETA: f64 = 8.96;

/// The least a [`Pad`] delays by, when it delays at all: its allpass is
/// kept between two and a half and three and a half samples, where a
/// third-order Thiran's delay is flattest.
const LEAST_PAD: f64 = 2.5;

/// The most whole samples a [`Pad`] holds back before its allpass: a pad
/// is never more than one host sample past [`LEAST_PAD`], and the line
/// holds the sample going in as well.
const PAD_LEN: usize = MAX_FACTOR + 1;

/// A delay of a fractional number of samples: a short line of whole
/// samples, then a third-order Thiran allpass for the rest, between two
/// and a half and three and a half samples. The allpass leaves every level
/// alone. Inside an oversampler (176.4 kHz and up) its delay is within
/// 2.3e-4 of a sample of the target at 15 kHz and 1.2e-3 at 20 kHz, the
/// worst being a pad just under three and a half. Run at a 44.1 kHz host
/// rate, as the crusher's is, it is within 0.02 of a sample at 15 kHz and
/// 0.06 at 20 kHz. Its floor of two and a half samples costs up to three
/// and a half samples of latency at the host rate (0.08 ms at 44.1 kHz),
/// and a sixteenth of that inside a 16x oversampler; a first-order
/// allpass would cost one sample less but be 0.07 of a sample off at
/// 20 kHz even at 176.4 kHz. Only [`whole_latency`] makes one.
///
/// Source: J.-P. Thiran, "Recursive digital filters with maximally flat
/// group delay" (IEEE Trans. Circuit Theory, 1971).
#[derive(Debug, Clone, Copy)]
pub struct Pad {
    line: [f32; PAD_LEN],
    /// Where the next sample goes in `line`.
    at: usize,
    /// How many whole samples `line` holds back.
    whole: usize,
    /// The allpass's coefficients `a1`, `a2`, `a3`: it is
    /// `(a3 + a2 z^-1 + a1 z^-2 + z^-3) / (1 + a1 z^-1 + a2 z^-2 + a3 z^-3)`.
    coefficients: [f64; 3],
    /// The allpass's last three inputs and outputs, newest first.
    inputs: [f64; 3],
    outputs: [f64; 3],
    /// Whether there is anything to delay at all.
    active: bool,
}

impl Default for Pad {
    /// No delay at all.
    fn default() -> Self {
        Self {
            line: [0.0; PAD_LEN],
            at: 0,
            whole: 0,
            coefficients: [0.0; 3],
            inputs: [0.0; 3],
            outputs: [0.0; 3],
            active: false,
        }
    }
}

impl Pad {
    /// A pad of `samples`, at least [`LEAST_PAD`] and at most
    /// [`MAX_FACTOR`] past it, silent.
    fn new(samples: f64) -> Self {
        let samples = samples.clamp(LEAST_PAD, LEAST_PAD + MAX_FACTOR as f64);
        let whole = (samples - LEAST_PAD).floor();
        let d = samples - whole;
        // a_k = (-1)^k C(3, k) Π_{n=0..3} (d - 3 + n) / (d - 3 + k + n),
        // the signed binomial passed in.
        let coefficient = |k: f64, binomial: f64| {
            (0..=3).fold(binomial, |product, n| {
                let n = f64::from(n);
                product * (d - 3.0 + n) / (d - 3.0 + k + n)
            })
        };
        Self {
            whole: whole as usize,
            coefficients: [
                coefficient(1.0, -3.0),
                coefficient(2.0, 3.0),
                coefficient(3.0, -1.0),
            ],
            active: true,
            ..Self::default()
        }
    }

    /// One sample in, one sample out, the pad's delay later. The allpass
    /// is recursive, so a non-finite sample goes in as silence and a
    /// non-finite or denormal output is flushed: one bad sample must not
    /// poison it for good.
    pub fn push(&mut self, sample: f32) -> f32 {
        if !self.active {
            return sample;
        }
        self.line[self.at] = if sample.is_finite() { sample } else { 0.0 };
        let delayed = self.line[(self.at + PAD_LEN - self.whole) % PAD_LEN];
        self.at = (self.at + 1) % PAD_LEN;
        let x = f64::from(delayed);
        let [a1, a2, a3] = self.coefficients;
        let [x1, x2, x3] = self.inputs;
        let [y1, y2, y3] = self.outputs;
        let mut y = a3.mul_add(x - y3, a2.mul_add(x1 - y2, a1.mul_add(x2 - y1, x3)));
        if !y.is_finite() || y.abs() < 1e-30 {
            y = 0.0;
        }
        self.inputs = [x, x1, x2];
        self.outputs = [y, y1, y2];
        y as f32
    }

    /// Silence.
    pub const fn clear(&mut self) {
        self.line = [0.0; PAD_LEN];
        self.at = 0;
        self.inputs = [0.0; 3];
        self.outputs = [0.0; 3];
    }
}

/// The whole number of samples a delay of `delay` is padded out to, and
/// the pad that takes it there, running at `factor` samples to the
/// host's one: the next whole sample, or a later one when the next is too
/// close for the allpass (under [`LEAST_PAD`] at the pad's rate). No pad at
/// all when `delay` is already whole. `factor` is an oversampler's, from 1
/// to [`MAX_FACTOR`]; the pad cannot reach further than that factor needs,
/// so anything outside is held to that range.
#[must_use]
pub fn whole_latency(delay: f64, factor: usize) -> (usize, Pad) {
    let delay = delay.max(0.0);
    let factor = factor.clamp(1, MAX_FACTOR) as f64;
    let mut whole = (delay - 1e-9).ceil().max(0.0);
    let short = (whole - delay) * factor;
    if short <= 1e-9 {
        return (whole as usize, Pad::default());
    }
    if short < LEAST_PAD {
        whole += ((LEAST_PAD - short) / factor).ceil();
    }
    (whole as usize, Pad::new((whole - delay) * factor))
}

/// Oversampling for one channel, by 1, 2, 4, 8 or 16, padded so that the
/// round trip, with whatever delay the work done at the high rate adds,
/// is a whole number of host samples.
#[derive(Debug, Clone, Copy)]
pub struct Oversampler {
    /// The sharp stage next to the base rate first, then the lenient ones.
    stages: [Halfband; MAX_STAGES],
    /// How many stages are in use.
    active: usize,
    /// The pad at the high rate, before the way back down.
    pad: Pad,
    /// The whole round trip, in host samples.
    latency: usize,
}

impl Default for Oversampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Oversampler {
    /// Every stage designed and silent, oversampling by 1 until told
    /// otherwise; the first stage is cut for a 48 kHz host until
    /// [`Self::configure`] cuts it for the real one.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stages: [
                first_stage(48_000.0),
                Halfband::new(7, 9.5),
                Halfband::new(7, 9.5),
                Halfband::new(7, 9.5),
            ],
            active: 0,
            pad: Pad::default(),
            latency: 0,
        }
    }

    /// Oversample by `factor` (1, 2, 4, 8 or 16; anything else is rounded
    /// down to one of those) from a host at `base_rate`, with the first
    /// stage cut to the host's needs, for work at the high rate that itself
    /// delays by `extra` samples there (the half samples of antiderivative
    /// anti-aliasing): the round trip is padded out to a whole number of
    /// host samples. Silences the stages.
    pub fn configure(&mut self, factor: usize, base_rate: f32, extra: f64) {
        self.stages[0] = first_stage(base_rate);
        self.active = match factor {
            0 | 1 => 0,
            2 | 3 => 1,
            4..=7 => 2,
            8..=15 => 3,
            _ => MAX_STAGES,
        };
        let factor = self.factor();
        let delay = self.round_trip() + extra.max(0.0) / factor as f64;
        (self.latency, self.pad) = whole_latency(delay, factor);
        self.reset();
    }

    /// The factor in use.
    #[must_use]
    pub const fn factor(&self) -> usize {
        1 << self.active
    }

    /// How many host samples the whole round trip delays by, padding and
    /// the work at the high rate included: a whole number.
    #[must_use]
    pub const fn latency(&self) -> usize {
        self.latency
    }

    /// How many host samples the filters alone delay by. Each stage's
    /// interpolating half delays by its centre tap, `2m + 1` samples at its
    /// higher rate, and its decimating half by one sample less (it reads
    /// the later of each pair), so a stage costs `4m + 1` samples at its
    /// higher rate.
    fn round_trip(&self) -> f64 {
        self.stages[..self.active]
            .iter()
            .enumerate()
            .map(|(depth, stage)| (4 * stage.centre + 1) as f64 / f64::from(2u32 << depth))
            .sum()
    }

    /// Run `core` on every oversampled sample of `sample`, in time order,
    /// and return the decimated result.
    pub fn process(&mut self, sample: f32, mut core: impl FnMut(f32) -> f32) -> f32 {
        if self.active == 0 {
            return self.pad.push(core(sample));
        }
        let mut buffer = self.expand(sample);
        for value in &mut buffer[..self.factor()] {
            *value = core(*value);
        }
        self.reduce(buffer)
    }

    /// Interpolate one sample up: the first [`Self::factor`] entries are
    /// the oversampled samples, in time order.
    pub fn expand(&mut self, sample: f32) -> [f32; MAX_FACTOR] {
        let mut buffer = [0.0f32; MAX_FACTOR];
        let mut spare = [0.0f32; MAX_FACTOR];
        buffer[0] = sample;
        let mut len = 1;
        for stage in &mut self.stages[..self.active] {
            for (i, value) in buffer[..len].iter().enumerate() {
                let [early, late] = stage.up(*value);
                spare[2 * i] = early;
                spare[2 * i + 1] = late;
            }
            len *= 2;
            buffer[..len].copy_from_slice(&spare[..len]);
        }
        buffer
    }

    /// Filter and decimate the first [`Self::factor`] entries of `buffer`
    /// back to one sample.
    pub fn reduce(&mut self, mut buffer: [f32; MAX_FACTOR]) -> f32 {
        let mut spare = [0.0f32; MAX_FACTOR];
        let mut len = self.factor();
        for value in &mut buffer[..len] {
            *value = self.pad.push(*value);
        }
        for stage in self.stages[..self.active].iter_mut().rev() {
            len /= 2;
            for i in 0..len {
                spare[i] = stage.down([buffer[2 * i], buffer[2 * i + 1]]);
            }
            buffer[..len].copy_from_slice(&spare[..len]);
        }
        buffer[0]
    }

    /// Silence every stage.
    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.clear();
        }
        self.pad.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, rate: f64, n: usize) -> impl Iterator<Item = f32> {
        (0..n).map(move |i| (2.0 * PI * hz * i as f64 / rate).sin() as f32)
    }

    fn level_at(samples: &[f32], hz: f64, rate: f64) -> f64 {
        let n = samples.len() as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, sample) in samples.iter().enumerate() {
            let window = 0.5f64.mul_add(-(2.0 * PI * i as f64 / n).cos(), 0.5);
            let angle = 2.0 * PI * hz * i as f64 / rate;
            let weighted = f64::from(*sample) * window;
            re = weighted.mul_add(angle.cos(), re);
            im = weighted.mul_add(angle.sin(), im);
        }
        re.hypot(im) * 4.0 / n
    }

    #[test]
    fn the_factor_meets_the_target() {
        assert_eq!(factor_for(44_100.0, TARGET_RATE), 4);
        assert_eq!(factor_for(48_000.0, TARGET_RATE), 4);
        assert_eq!(factor_for(88_200.0, TARGET_RATE), 2);
        assert_eq!(factor_for(96_000.0, TARGET_RATE), 2);
        assert_eq!(factor_for(176_400.0, TARGET_RATE), 1);
        assert_eq!(factor_for(192_000.0, TARGET_RATE), 1);
        assert_eq!(factor_for(44_100.0, 2.0 * TARGET_RATE), 8);
        assert_eq!(factor_for(48_000.0, 4.0 * TARGET_RATE), 16);
        assert_eq!(factor_for(8_000.0, TARGET_RATE), MAX_FACTOR);
    }

    /// Every factor the oversampler is asked for is the one it runs at.
    #[test]
    fn every_factor_is_taken_as_asked() {
        let mut oversampler = Oversampler::new();
        for (asked, runs) in [
            (1, 1),
            (2, 2),
            (3, 2),
            (4, 4),
            (8, 8),
            (12, 8),
            (16, 16),
            (64, 16),
        ] {
            oversampler.configure(asked, 48_000.0, 0.0);
            assert_eq!(oversampler.factor(), runs, "asked for {asked}");
        }
    }

    #[test]
    fn up_and_down_is_transparent_in_band() {
        let mut os = Oversampler::new();
        let rate = 48_000.0;
        for factor in [1, 2, 4, 8, 16] {
            os.configure(factor, rate as f32, 0.0);
            assert_eq!(os.factor(), factor);
            for hz in [100.0, 1_000.0, 10_000.0, 19_000.0] {
                os.reset();
                let mut calls = 0;
                let out: Vec<f32> = tone(hz, rate, 16_384)
                    .map(|s| {
                        os.process(s, |x| {
                            calls += 1;
                            x
                        })
                    })
                    .collect();
                assert_eq!(calls, 16_384 * factor);
                let level = level_at(&out[4_096..], hz, rate);
                assert!(
                    (level - 1.0).abs() < 0.01,
                    "x{factor}: {hz} Hz came back at {level}"
                );
            }
        }
    }

    /// Every image an upsampler leaves, for a tone at `hz` from a host at
    /// `rate`, at every factor: the copies at multiples of the host rate
    /// either side of the tone.
    #[test]
    fn interpolation_images_are_deeply_rejected() {
        for rate in [44_100.0, 48_000.0, 96_000.0] {
            for factor in [2, 4, 8, 16] {
                let mut os = Oversampler::new();
                os.configure(factor, rate as f32, 0.0);
                let mut high = Vec::new();
                for s in tone(5_000.0, rate, 16_384) {
                    os.process(s, |x| {
                        high.push(x);
                        x
                    });
                }
                let high_rate = rate * factor as f64;
                let settled = &high[8_192 * factor..];
                let wanted = level_at(settled, 5_000.0, high_rate);
                assert!((wanted - 1.0).abs() < 0.01, "x{factor} at {rate}: {wanted}");
                for k in 1..factor {
                    let host = k as f64 * rate;
                    for image in [host - 5_000.0, host + 5_000.0] {
                        let level = level_at(settled, image, high_rate);
                        assert!(
                            level < 1e-4,
                            "x{factor} at {rate}: image at {image} Hz is {level}"
                        );
                    }
                }
            }
        }
    }

    /// The round trip's delay is what `latency` says: an impulse comes out
    /// peaked there.
    #[test]
    fn latency_is_the_impulse_delay() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
            for factor in [1, 2, 4, 8, 16] {
                let mut os = Oversampler::new();
                os.configure(factor, rate, 0.0);
                let out: Vec<f32> = (0..400)
                    .map(|n| os.process(if n == 0 { 1.0 } else { 0.0 }, |x| x))
                    .collect();
                let (peak, _) = out.iter().enumerate().fold((0, 0.0f32), |best, (i, s)| {
                    if s.abs() > best.1 { (i, s.abs()) } else { best }
                });
                // The delay is whole, so the peak sits right on it.
                let latency = os.latency();
                assert_eq!(peak, latency, "x{factor} at {rate}");
            }
        }
    }

    /// The phase delay of `out` against a sine at `hz` and `rate` that
    /// started at phase zero, measured from `from` on and taken as `lag`
    /// samples plus what is returned: projected onto the sine and cosine
    /// `lag` samples back, so level changes (an antiderivative's droop) do
    /// not count.
    fn delay_off(out: &[f32], hz: f64, rate: f64, from: usize, lag: usize) -> f64 {
        let w = 2.0 * PI * hz / rate;
        // Least squares for y = a sin + b cos over the span: the two are not
        // quite orthogonal over a span that is not a whole number of cycles.
        let (mut ys, mut yc, mut ss, mut sc, mut cc) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, y) in out.iter().enumerate().skip(from) {
            let phase = w * (i - lag) as f64;
            let (sin, cos) = phase.sin_cos();
            let y = f64::from(*y);
            ys = y.mul_add(sin, ys);
            yc = y.mul_add(cos, yc);
            ss = sin.mul_add(sin, ss);
            sc = sin.mul_add(cos, sc);
            cc = cos.mul_add(cos, cc);
        }
        let det = ss.mul_add(cc, -(sc * sc));
        let a = ys.mul_add(cc, -(yc * sc)) / det;
        let b = yc.mul_add(ss, -(ys * sc)) / det;
        // y = g sin(w (i - lag - e)) = g (cos(w e) sin - sin(w e) cos).
        (-b).atan2(a) / w
    }

    /// With the work at the high rate adding its own delay (none, and a
    /// half, one and a half, two and a half and five samples there, made as
    /// antiderivatives make them, by averaging each sample with the one
    /// before), the round trip is padded out to exactly the whole number of
    /// host samples `latency` declares: at 1 kHz and 15 kHz the output's
    /// phase is the input's that many samples back, to within a
    /// thousandth of a sample, at every rate and factor that reaches the
    /// rates the pad runs at.
    #[test]
    fn the_padded_delay_is_exactly_whole() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
            for factor in [1, 2, 4, 8, 16] {
                if rate * (factor as f32) < TARGET_RATE {
                    continue;
                }
                for halves in [0, 1, 3, 5, 10] {
                    let mut os = Oversampler::new();
                    os.configure(factor, rate, f64::from(halves) / 2.0);
                    let latency = os.latency();
                    for hz in [1_000.0, 15_000.0] {
                        let n = 8_000;
                        os.reset();
                        let mut previous = [0.0f32; 10];
                        let out: Vec<f32> = tone(hz, f64::from(rate), n)
                            .map(|x| {
                                os.process(x, |mut v| {
                                    for last in &mut previous[..halves as usize] {
                                        let now = v;
                                        v = 0.5 * (v + *last);
                                        *last = now;
                                    }
                                    v
                                })
                            })
                            .collect();
                        let off = delay_off(&out, hz, f64::from(rate), n / 2, latency);
                        assert!(
                            off.abs() < 1e-3,
                            "x{factor} at {rate}, {halves} halves, {hz} Hz: {off:.5} samples off {latency}"
                        );
                    }
                }
            }
        }
    }

    /// The whole latency is the next whole sample, or a later one when the
    /// pad would be under two and a half samples at its own rate; a delay
    /// already whole is left alone.
    #[test]
    fn whole_latency_pads_forward() {
        for (delay, factor, whole) in [
            (0.0, 4, 0),
            (3.0, 4, 3),
            (2.25, 4, 3),
            (2.5, 4, 4),
            (2.8, 16, 3),
            (2.9, 16, 4),
            (2.95, 16, 4),
            (2.9, 1, 6),
            (7.5, 1, 10),
        ] {
            let (got, pad) = whole_latency(delay, factor);
            assert_eq!(got, whole, "{delay} at x{factor}");
            let short = (whole as f64 - delay) * factor as f64;
            assert_eq!(pad.active, short > 1e-9, "{delay} at x{factor}");
            assert!(!pad.active || short >= LEAST_PAD, "{delay} at x{factor}");
        }
    }

    /// A non-finite sample through a pad comes out as silence and leaves
    /// it working: a tone after it comes through at full level.
    #[test]
    fn a_pad_survives_a_non_finite_sample() {
        let mut pad = Pad::new(3.2);
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for _ in 0..8 {
                assert!(pad.push(bad).is_finite());
            }
            let out: Vec<f32> = tone(1_000.0, 48_000.0, 4_800)
                .map(|x| pad.push(x))
                .collect();
            let peak = out[2_400..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((peak - 1.0).abs() < 1e-3, "{bad}: {peak}");
        }
    }

    /// A pad delays a 15 kHz tone at 176.4 kHz by what it was made for, to
    /// within 2.3e-4 of a sample, across its whole range, the worst case
    /// (an allpass just under three and a half samples) included.
    #[test]
    fn a_pad_delays_by_what_it_was_made_for() {
        let rate = 176_400.0;
        let steps = (0..=320).map(|k| 2.5 + f64::from(k) / 20.0);
        for samples in steps.chain([3.499, 10.499, 18.499]) {
            let mut pad = Pad::new(samples);
            let n = 8_000;
            let out: Vec<f32> = tone(15_000.0, rate, n).map(|x| pad.push(x)).collect();
            let whole = samples.floor() as usize;
            let off = delay_off(&out, 15_000.0, rate, n / 2, whole) - (samples - whole as f64);
            assert!(off.abs() < 2.3e-4, "{samples}: {off:.6}");
        }
    }
}
