//! The master chain: the only enforced taste, and it adds no colour.
//!
//! The sum of the outs goes through a high-pass that takes out DC and
//! subsonics, then a look-ahead true-peak limiter that holds it to −1 dBTP.
//!
//! - **High-pass:** a second-order Butterworth at 10 Hz, a topology-
//!   preserving state-variable filter in double precision, so it is flat
//!   within 0.1 dB from 30 Hz up at every rate and adds no noise of its own.
//! - **Limiter:** the peak between samples is estimated by 8× interpolation
//!   (a windowed-sinc polyphase kernel), taken from both channels at once,
//!   and turned into the gain each sample needs. A running minimum over the
//!   1.5 ms look-ahead and a 30 ms hold, a 150 ms release, and a moving
//!   average over the look-ahead make a gain curve that is smooth, steady
//!   through a bass note, and already down by the time the peak arrives, so
//!   nothing is ever clipped: a quiet signal passes with a gain of exactly
//!   1. The chain delays the sound by the look-ahead plus 48 frames.
//! - **Guard:** a sample that is not a number becomes silence, and nothing
//!   leaves beyond full scale. Neither engages on a healthy signal.

use std::f64::consts::PI;

use crate::SUB_BLOCK;
use crate::dsp::Block;

/// The limiter's ceiling: −1 dBTP.
pub const CEILING: f32 = 0.891_250_9;

/// The ceiling the detector aims for, a little under [`CEILING`] so the
/// 4× estimate of the peak between samples, which can read a touch low, and
/// the gain moving across a peak never carry the true peak over it.
const TARGET: f64 = 0.870_963_6;

/// The high-pass corner.
const HIGH_PASS_HZ: f64 = 10.0;

/// How far ahead the limiter looks, which is also how long its gain takes
/// to come down.
const LOOKAHEAD_SECONDS: f64 = 0.0015;

/// How long the limiter holds its gain down after a peak before it starts
/// to release: longer than half a cycle of a 20 Hz bass note, so the gain
/// does not ripple with the waveform (which is distortion).
const HOLD_SECONDS: f64 = 0.03;

/// Limiter release time constant.
const RELEASE_SECONDS: f64 = 0.15;

/// Taps per phase of the peak interpolator: enough for the kernel to stay
/// flat to within a few percent of the top of the band, where a naive,
/// aliased waveform (a bit crusher, a hard clip) puts much of its peak.
const TAPS: usize = 96;

/// Points looked at between two samples: the peak is estimated at eight
/// times the rate. At 4× a tone near the top of the band can peak an eighth
/// of a sample from the nearest point looked at and read half a decibel
/// low; at 8× it reads less than 0.15 dB low.
const PHASES: usize = 7;

/// The detector's delay behind the input: the interpolator needs `TAPS / 2`
/// samples after the point it estimates, and each sample's need also looks
/// at the interval after it.
const DETECTOR_DELAY: usize = TAPS / 2;

/// Double-precision state-variable high-pass, Butterworth (Q = 1/√2).
#[derive(Debug, Clone, Copy)]
struct HighPass {
    g: f64,
    k: f64,
    s1: f64,
    s2: f64,
}

impl HighPass {
    fn new(sample_rate: f64) -> Self {
        Self {
            g: (PI * HIGH_PASS_HZ / sample_rate).tan(),
            k: std::f64::consts::SQRT_2,
            s1: 0.0,
            s2: 0.0,
        }
    }

    fn process(&mut self, x: f64) -> f64 {
        let Self { g, k, .. } = *self;
        let high = (x - (g + k).mul_add(self.s1, self.s2)) / g.mul_add(g + k, 1.0);
        let band = g.mul_add(high, self.s1);
        self.s1 = g.mul_add(high, band);
        let low = g.mul_add(band, self.s2);
        self.s2 = g.mul_add(band, low);
        for state in [&mut self.s1, &mut self.s2] {
            if !state.is_finite() || state.abs() < 1e-30 {
                *state = 0.0;
            }
        }
        high
    }

    const fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

/// Lanes the detector's sums run in, independent of each other so they
/// vectorise.
const LANES: usize = 8;

/// The interpolation kernel: for each point between samples, the weight of
/// each of the `TAPS` samples around it. Single precision is plenty for
/// finding a peak, and twice as fast.
fn kernel() -> [[f32; TAPS]; PHASES] {
    // Kaiser window, beta 10: about 100 dB of sidelobe rejection.
    const BETA: f64 = 10.0;
    let half = (TAPS / 2) as f64;
    let mut kernel = [[0.0; TAPS]; PHASES];
    for (phase, row) in kernel.iter_mut().enumerate() {
        let mut weights = [0.0_f64; TAPS];
        // Below PHASES + 1: exact.
        let mu = (phase + 1) as f64 / (PHASES + 1) as f64;
        for (tap, weight) in weights.iter_mut().enumerate() {
            // Tap 0 is the oldest sample: the point sits between taps
            // TAPS/2 - 1 and TAPS/2.
            let t = mu + half - 1.0 - tap as f64;
            let sinc = (PI * t).sin() / (PI * t);
            let ratio = (t / half).clamp(-1.0, 1.0);
            let window = bessel_i0(BETA * ratio.mul_add(-ratio, 1.0).sqrt()) / bessel_i0(BETA);
            *weight = sinc * window;
        }
        let sum: f64 = weights.iter().sum();
        for (slot, weight) in row.iter_mut().zip(weights) {
            *slot = (weight / sum) as f32;
        }
    }
    kernel
}

/// The dot product of two `TAPS`-long rows, in [`LANES`] independent sums.
fn dot(weights: &[f32; TAPS], samples: &[f32]) -> f32 {
    let mut lanes = [0.0_f32; LANES];
    for (w, x) in weights.chunks_exact(LANES).zip(samples.chunks_exact(LANES)) {
        for lane in 0..LANES {
            lanes[lane] = w[lane].mul_add(x[lane], lanes[lane]);
        }
    }
    lanes.iter().sum()
}

/// The largest magnitude in `samples`, in [`LANES`] independent maxima.
fn loudest(samples: &[f32]) -> f32 {
    let mut lanes = [0.0_f32; LANES];
    for chunk in samples.chunks_exact(LANES) {
        for lane in 0..LANES {
            lanes[lane] = lanes[lane].max(chunk[lane].abs());
        }
    }
    lanes.iter().fold(0.0, |m, v| m.max(*v))
}

/// The zeroth-order modified Bessel function of the first kind.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let quarter = x * x / 4.0;
    for k in 1..64_u32 {
        let k = f64::from(k);
        term *= quarter / (k * k);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// The look-ahead true-peak limiter for a stereo pair.
#[derive(Debug)]
struct Limiter {
    kernel: [[f32; TAPS]; PHASES],
    /// The most any point between samples can exceed the largest of the
    /// `TAPS` samples around it: the kernel's largest sum of magnitudes.
    bound: f32,
    /// The last `TAPS` input samples of each channel, written twice, `TAPS`
    /// apart, so the window is one slice: `history_at + 1 ..= history_at +
    /// TAPS`, oldest first.
    history: [[f32; 2 * TAPS]; 2],
    history_at: usize,
    /// The peak over the interval that ended at the previous estimate.
    last_interval: f64,
    /// Look-ahead window, in frames.
    window: usize,
    /// Audio waiting for its gain: `DETECTOR_DELAY + window - 1` frames.
    delay: Box<[[f64; 2]]>,
    delay_at: usize,
    /// Running minimum of the needed gain over the window and the hold
    /// after it: a monotonic queue of (frame, gain).
    minimum: Box<[(u64, f64)]>,
    minimum_head: usize,
    minimum_len: usize,
    frame: u64,
    /// The released gain, and the moving average over the window.
    released: f64,
    release: f64,
    average: Box<[f64]>,
    average_at: usize,
    average_sum: f64,
}

impl Limiter {
    fn new(sample_rate: f64) -> Self {
        let window = ((LOOKAHEAD_SECONDS * sample_rate).round() as usize).max(1);
        let hold = ((HOLD_SECONDS * sample_rate).round() as usize).max(1);
        let kernel = kernel();
        let bound = kernel
            .iter()
            .map(|weights| weights.iter().map(|w| w.abs()).sum::<f32>())
            .fold(1.0, f32::max);
        Self {
            kernel,
            bound,
            history: [[0.0; 2 * TAPS]; 2],
            history_at: 0,
            last_interval: 0.0,
            window,
            delay: vec![[0.0; 2]; DETECTOR_DELAY + window - 1 + 1].into_boxed_slice(),
            delay_at: 0,
            minimum: vec![(0, 1.0); window + hold].into_boxed_slice(),
            minimum_head: 0,
            minimum_len: 0,
            frame: 0,
            released: 1.0,
            release: 1.0 - (-1.0 / (RELEASE_SECONDS * sample_rate)).exp(),
            average: vec![1.0; window].into_boxed_slice(),
            average_at: 0,
            // At most a few thousand: exact.
            average_sum: window as f64,
        }
    }

    /// Frames between a sample going in and coming out.
    const fn latency(&self) -> usize {
        DETECTOR_DELAY + self.window - 1
    }

    /// The peak, over both channels, of the interval ending at the sample
    /// `TAPS / 2` frames back (the sample itself included).
    fn interval_peak(&self) -> f64 {
        let start = self.history_at + 1;
        let windows = [
            &self.history[0][start..start + TAPS],
            &self.history[1][start..start + TAPS],
        ];
        let mut peak = 0.0_f32;
        let mut largest = 0.0_f32;
        for window in windows {
            peak = peak.max(window[TAPS / 2].abs());
            largest = largest.max(loudest(window));
        }
        if f64::from(largest * self.bound) <= TARGET {
            // Nothing between these samples can reach the ceiling: skip the
            // interpolation, which is most of the limiter's work.
            return f64::from(peak);
        }
        for window in windows {
            for weights in &self.kernel {
                peak = peak.max(dot(weights, window).abs());
            }
        }
        f64::from(peak)
    }

    fn push_minimum(&mut self, gain: f64) {
        let capacity = self.minimum.len();
        // Drop from the back everything no smaller than the new gain.
        while self.minimum_len > 0 {
            let back = (self.minimum_head + self.minimum_len - 1) % capacity;
            if self.minimum[back].1 >= gain {
                self.minimum_len -= 1;
            } else {
                break;
            }
        }
        // Drop from the front what has left the window.
        while self.minimum_len > 0 {
            let (frame, _) = self.minimum[self.minimum_head];
            // At most a few thousand frames: exact.
            if frame + capacity as u64 <= self.frame {
                self.minimum_head = (self.minimum_head + 1) % capacity;
                self.minimum_len -= 1;
            } else {
                break;
            }
        }
        let slot = (self.minimum_head + self.minimum_len) % capacity;
        self.minimum[slot] = (self.frame, gain);
        self.minimum_len += 1;
    }

    fn process(&mut self, left: f64, right: f64) -> (f64, f64) {
        self.history_at = (self.history_at + 1) % TAPS;
        for (channel, sample) in [left, right].into_iter().enumerate() {
            // Only for finding the peak: single precision is plenty.
            self.history[channel][self.history_at] = sample as f32;
            self.history[channel][self.history_at + TAPS] = sample as f32;
        }
        let interval = self.interval_peak();
        let peak = interval.max(self.last_interval);
        self.last_interval = interval;
        let needed = if peak > TARGET { TARGET / peak } else { 1.0 };
        self.push_minimum(needed);
        self.frame += 1;
        let minimum = self.minimum[self.minimum_head].1;
        self.released = if minimum < self.released {
            minimum
        } else {
            (minimum - self.released).mul_add(self.release, self.released)
        };
        self.average_sum += self.released - self.average[self.average_at];
        self.average[self.average_at] = self.released;
        self.average_at += 1;
        if self.average_at == self.average.len() {
            self.average_at = 0;
            // Re-add from scratch once a window, so rounding never drifts.
            self.average_sum = self.average.iter().sum();
        }
        // Window is at most a few thousand frames: exact.
        let gain = (self.average_sum / self.window as f64).min(1.0);
        let len = self.delay.len();
        self.delay[self.delay_at] = [left, right];
        self.delay_at = (self.delay_at + 1) % len;
        // The oldest frame, `latency` frames ago: the one this gain is for.
        let [l, r] = self.delay[self.delay_at];
        (l * gain, r * gain)
    }

    fn reset(&mut self) {
        self.history = [[0.0; 2 * TAPS]; 2];
        self.history_at = 0;
        self.last_interval = 0.0;
        self.delay.fill([0.0; 2]);
        self.delay_at = 0;
        self.minimum_head = 0;
        self.minimum_len = 0;
        self.frame = 0;
        self.released = 1.0;
        self.average.fill(1.0);
        self.average_at = 0;
        // At most a few thousand: exact.
        self.average_sum = self.window as f64;
    }
}

/// The master chain for a stereo pair.
#[derive(Debug)]
pub struct Master {
    high_pass: [HighPass; 2],
    limiter: Limiter,
}

impl Master {
    /// A master chain at `sample_rate`. This allocates the look-ahead
    /// buffers: build it on the control side.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
            f64::from(sample_rate)
        } else {
            48_000.0
        };
        Self {
            high_pass: [HighPass::new(rate); 2],
            limiter: Limiter::new(rate),
        }
    }

    /// Frames between a sample going in and coming out.
    #[must_use]
    pub const fn latency(&self) -> usize {
        self.limiter.latency()
    }

    /// Run the chain over one sub-block of the master sum, in place.
    pub fn process(&mut self, left: &mut Block, right: &mut Block) {
        for frame in 0..SUB_BLOCK {
            let [high_left, high_right] = &mut self.high_pass;
            let l = high_left.process(f64::from(guard(left[frame])));
            let r = high_right.process(f64::from(guard(right[frame])));
            let (l, r) = self.limiter.process(l, r);
            left[frame] = guard(l as f32).clamp(-1.0, 1.0);
            right[frame] = guard(r as f32).clamp(-1.0, 1.0);
        }
    }

    /// Forget all state.
    pub fn reset(&mut self) {
        for filter in &mut self.high_pass {
            filter.reset();
        }
        self.limiter.reset();
    }
}

/// Silence for a sample that is not a number.
#[inline]
const fn guard(sample: f32) -> f32 {
    kazoo_core::sanitize_sample(sample)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::f64::consts::TAU;

    fn run(rate: f32, frames: usize, signal: impl Fn(usize) -> f64) -> (Master, Vec<f32>) {
        let mut master = Master::new(rate);
        let mut out = Vec::with_capacity(frames + SUB_BLOCK);
        let mut frame = 0;
        while out.len() < frames {
            let mut left = [0.0; SUB_BLOCK];
            for (i, sample) in left.iter_mut().enumerate() {
                *sample = signal(frame + i) as f32;
            }
            let mut right = left;
            master.process(&mut left, &mut right);
            out.extend_from_slice(&left);
            frame += SUB_BLOCK;
        }
        out.truncate(frames);
        (master, out)
    }

    /// Amplitude of the sinusoid at `hz` in `x`, by least squares.
    fn amplitude(x: &[f32], hz: f64, rate: f64) -> f64 {
        let (mut ss, mut cc, mut sc, mut xs, mut xc) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            let (s, c) = (TAU * hz * i as f64 / rate).sin_cos();
            let v = f64::from(*v);
            ss = s.mul_add(s, ss);
            cc = c.mul_add(c, cc);
            sc = s.mul_add(c, sc);
            xs = v.mul_add(s, xs);
            xc = v.mul_add(c, xc);
        }
        let det = ss.mul_add(cc, -(sc * sc));
        let a = xs.mul_add(cc, -(xc * sc)) / det;
        let b = xc.mul_add(ss, -(xs * sc)) / det;
        a.hypot(b)
    }

    /// The largest magnitude of `x` reconstructed between its samples at
    /// eight times the rate, with a long windowed-sinc kernel: what a DAC
    /// puts out. Only the stretches next to loud samples are searched, as
    /// nothing else can come near the peak.
    fn true_peak(x: &[f32]) -> f64 {
        const HALF: usize = 64;
        let loudest = x.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        let mut peak = f64::from(loudest);
        for n in HALF..x.len() - HALF {
            if x[n].abs().max(x[n + 1].abs()) < loudest * 0.7 {
                continue;
            }
            for step in 1..8_u32 {
                let mu = f64::from(step) / 8.0;
                let mut value = 0.0;
                for tap in 0..2 * HALF {
                    // Tap `HALF - 1` is sample n, tap `HALF` sample n + 1.
                    let t = mu + (HALF - 1) as f64 - tap as f64;
                    let sinc = (PI * t).sin() / (PI * t);
                    let window = 0.5f64.mul_add((PI * t / HALF as f64).cos(), 0.5);
                    let sample = f64::from(x[n + 1 + tap - HALF]);
                    value = (sinc * window).mul_add(sample, value);
                }
                peak = peak.max(value.abs());
            }
        }
        peak
    }

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    const RATES: [f32; 3] = [48_000.0, 96_000.0, 192_000.0];

    /// A test signal: the sample at each frame.
    type Signal = Box<dyn Fn(usize) -> f64>;

    #[test]
    fn it_is_flat_from_thirty_hertz_at_every_rate() {
        for rate in RATES {
            let fs = f64::from(rate);
            for hz in [30.0, 50.0, 100.0, 1_000.0, 10_000.0, 20_000.0] {
                let settle = (fs * 1.5) as usize;
                let (_, out) = run(rate, settle + fs as usize / 2, |i| {
                    0.1 * (TAU * hz * i as f64 / fs).sin()
                });
                let gain = db(amplitude(&out[settle..], hz, fs) / 0.1);
                assert!(gain.abs() < 0.1, "{rate} Hz rate, {hz} Hz: {gain:.3} dB");
            }
        }
    }

    #[test]
    fn it_removes_dc_and_subsonics() {
        for rate in RATES {
            let fs = f64::from(rate);
            let (_, out) = run(rate, (fs * 3.0) as usize, |_| 0.5);
            let tail = &out[(fs * 2.5) as usize..];
            assert!(tail.iter().all(|s| s.abs() < 1e-4), "{rate}");
            let (_, out) = run(rate, (fs * 3.0) as usize, |i| {
                0.5 * (TAU * 2.0 * i as f64 / fs).sin()
            });
            let gain = db(amplitude(&out[(fs * 1.5) as usize..], 2.0, fs) / 0.5);
            assert!(gain < -27.0, "{rate}: 2 Hz at {gain:.1} dB");
        }
    }

    #[test]
    fn quiet_music_passes_untouched() {
        // Below the ceiling the limiter's gain is exactly 1: the output is
        // the high-passed input, delayed, to single precision.
        for rate in RATES {
            let fs = f64::from(rate);
            let signal = |i: usize| {
                let t = i as f64 / fs;
                0.4f64.mul_add((TAU * 440.0 * t).sin(), 0.3 * (TAU * 5_123.0 * t).sin())
            };
            let frames = (fs * 1.0) as usize;
            let (master, out) = run(rate, frames, signal);
            let mut reference = HighPass::new(fs);
            let expected: Vec<f64> = (0..frames)
                .map(|i| reference.process(f64::from(signal(i) as f32)))
                .collect();
            let latency = master.latency();
            for i in latency..frames {
                let error = (f64::from(out[i]) - expected[i - latency]).abs();
                assert!(error < 1e-6, "{rate}: frame {i} off by {error}");
            }
        }
    }

    #[test]
    fn the_true_peak_stays_under_minus_one_dbtp() {
        // The cases that carried the old limiter over its ceiling.
        for rate in [48_000.0f32, 192_000.0] {
            let fs = f64::from(rate);
            let cases: [(&str, Signal); 5] = [
                (
                    "sine 11.025 kHz +6 dB",
                    Box::new(move |i| 2.0 * (TAU * 11_025.0 * i as f64 / fs + 0.785).sin()),
                ),
                (
                    "sine 7 kHz 0 dB",
                    Box::new(move |i| (TAU * 7_000.3 * i as f64 / fs + 0.3).sin()),
                ),
                (
                    "naive saw 1 kHz x2",
                    Box::new(move |i| {
                        let phase = (1_000.0 * i as f64 / fs).fract();
                        2.0 * phase.mul_add(2.0, -1.0)
                    }),
                ),
                (
                    "square 3.1 kHz x1.5",
                    Box::new(move |i| {
                        if (3_100.0 * i as f64 / fs).fract() < 0.5 {
                            1.5
                        } else {
                            -1.5
                        }
                    }),
                ),
                (
                    "bursts",
                    Box::new(move |i| {
                        let t = i as f64 / fs;
                        let burst = if (t * 3.0).fract() < 0.1 { 4.0 } else { 0.2 };
                        burst * (TAU * 9_000.0 * t).sin()
                    }),
                ),
            ];
            for (name, signal) in cases {
                let settle = (fs * 0.5) as usize;
                let (_, out) = run(rate, settle + (fs * 0.35) as usize, signal);
                let tail = &out[settle..];
                let sample_peak = tail.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
                assert!(
                    sample_peak <= CEILING,
                    "{rate} {name}: sample peak {sample_peak}"
                );
                let peak = db(true_peak(tail));
                assert!(peak <= -1.0, "{rate} {name}: {peak:.2} dBTP");
            }
        }
    }

    #[test]
    fn loud_music_is_limited_without_distortion() {
        // A steady tone 6 dB over the ceiling comes out at the ceiling with
        // its harmonics far below it: the gain is steady, not clipping.
        let rate = 48_000.0;
        let fs = f64::from(rate);
        let (_, out) = run(rate, (fs * 2.0) as usize, |i| {
            2.0 * (TAU * 1_000.0 * i as f64 / fs).sin()
        });
        let tail = &out[fs as usize..];
        let fundamental = amplitude(tail, 1_000.0, fs);
        assert!(
            db(fundamental) > -1.6 && db(fundamental) <= -1.0,
            "{}",
            db(fundamental)
        );
        for harmonic in 2..=5 {
            let level = db(amplitude(tail, 1_000.0 * f64::from(harmonic), fs) / fundamental);
            assert!(level < -80.0, "harmonic {harmonic}: {level:.1} dBc");
        }
    }

    #[test]
    fn nonsense_is_silenced() {
        let (mut master, out) = run(48_000.0, 320, |i| {
            if i % 2 == 0 { f64::NAN } else { f64::INFINITY }
        });
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        master.reset();
    }
}
