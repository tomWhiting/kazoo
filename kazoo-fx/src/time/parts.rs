//! Parts the time family shares: tempo divisions, block bookkeeping, a
//! biquad, an anti-aliased soft clipper, an envelope follower, LFO shapes,
//! the mix laws and the output guard.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use crate::ParamSpec;
use crate::dsp::{DelayLine, flush};

/// The labels of a tempo-sync knob: `free` (use the time knob) and then the
/// note values, shortest first. `t` is a triplet, `d` a dotted note.
pub const SYNC_LABELS: &[&str] = &[
    "free", "1/32", "1/16t", "1/16", "1/16d", "1/8t", "1/8", "1/8d", "1/4t", "1/4", "1/4d", "1/2t",
    "1/2", "1/2d", "1/1",
];

/// Beats in each note value of [`SYNC_LABELS`] after `free`.
const DIVISION_BEATS: [f64; 14] = [
    0.125,
    1.0 / 6.0,
    0.25,
    0.375,
    1.0 / 3.0,
    0.5,
    0.75,
    2.0 / 3.0,
    1.0,
    1.5,
    4.0 / 3.0,
    2.0,
    3.0,
    4.0,
];

/// The slowest tempo a synced time is sized for.
pub const SLOWEST_BPM: f64 = 20.0;

/// The fastest tempo a synced time follows.
const FASTEST_BPM: f64 = 999.0;

/// The longest synced time: a whole note at [`SLOWEST_BPM`], in seconds.
pub const LONGEST_SYNCED_SECONDS: f32 = 12.0;

/// The length in seconds of sync step `step` at `bpm`, or `None` for `free`
/// (step 0) or a step past the end. The tempo is held between 20 and 999.
#[must_use]
pub fn synced_seconds(step: f32, bpm: f64) -> Option<f32> {
    let index = step.round();
    if index.is_nan() || index < 1.0 {
        return None;
    }
    let beats = DIVISION_BEATS.get(index as usize - 1)?;
    let bpm = if bpm.is_finite() {
        bpm.clamp(SLOWEST_BPM, FASTEST_BPM)
    } else {
        120.0
    };
    Some((beats * 60.0 / bpm) as f32)
}

/// A parameter value made ready to use: `None` for an index past `params`
/// or a value that is not finite, else the value clamped to its range.
#[must_use]
pub fn accept(params: &[ParamSpec], index: usize, value: f32) -> Option<f32> {
    if value.is_finite() {
        params.get(index).map(|spec| spec.clamp(value))
    } else {
        None
    }
}

/// Every parameter's default, in order.
#[must_use]
pub fn defaults<const N: usize>(params: &[ParamSpec; N]) -> [f32; N] {
    let mut values = [0.0; N];
    for (value, spec) in values.iter_mut().zip(params) {
        *value = spec.default;
    }
    values
}

/// An input sample made safe: NaN and infinity become silence, and anything
/// louder than +36 dBFS is held there so no filter state can overflow.
#[must_use]
pub const fn clean(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-64.0, 64.0)
    } else {
        0.0
    }
}

/// The output guard: unity up to full scale, then a smooth knee that never
/// passes 2.0 (+6 dBFS). A NaN comes out as silence.
#[must_use]
pub fn guard(sample: f32) -> f32 {
    if !sample.is_finite() {
        return 0.0;
    }
    let size = sample.abs();
    if size <= 1.0 {
        sample
    } else {
        (1.0 + (size - 1.0).tanh()).copysign(sample)
    }
}

/// How many frames a block holds: the shortest of the four slices.
#[must_use]
pub fn frames(input: &[&[f32]; 2], output: &[&mut [f32]; 2]) -> usize {
    input[0]
        .len()
        .min(input[1].len())
        .min(output[0].len())
        .min(output[1].len())
}

/// Silence both outputs from frame `from` onwards.
pub fn silence_from(output: &mut [&mut [f32]; 2], from: usize) {
    for channel in output.iter_mut() {
        if let Some(rest) = channel.get_mut(from..) {
            rest.fill(0.0);
        }
    }
}

/// Equal-power dry and wet gains for a mix knob from 0 (dry) to 1 (wet).
#[must_use]
pub fn equal_power(mix: f32) -> (f32, f32) {
    let angle = mix.clamp(0.0, 1.0) * FRAC_PI_2;
    (angle.cos(), angle.sin())
}

/// A triangle from a phase of 0 up to 1: +1 at 0, -1 at a half.
#[must_use]
pub fn triangle(phase: f32) -> f32 {
    4.0f32.mul_add((phase - 0.5).abs(), -1.0)
}

/// A sine from a phase of 0 up to 1.
#[must_use]
pub fn sine(phase: f32) -> f32 {
    (TAU * phase).sin()
}

/// A direct-form II transposed biquad with the usual designs, computed in
/// double precision: at high sample rates a low corner puts the poles so
/// close to the unit circle that single-precision coefficients and states
/// would add noise that rises with the rate.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new()
    }
}

impl Biquad {
    /// A filter that passes everything unchanged.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// One sample through the filter.
    pub fn process(&mut self, input: f32) -> f32 {
        let input = f64::from(input);
        let out = self.b0.mul_add(input, self.z1);
        self.z1 = self.b1.mul_add(input, (-self.a1).mul_add(out, self.z2));
        self.z2 = self.b2.mul_add(input, -self.a2 * out);
        flush_wide(&mut self.z1);
        flush_wide(&mut self.z2);
        out as f32
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// A resonant lowpass at `hz` with quality `q`, matched to the analogue
    /// response (see [`Matched`]), so it sounds the same at every rate.
    pub fn set_lowpass(&mut self, hz: f32, q: f32, sample_rate: f32) {
        let m = Matched::new(hz, q, sample_rate);
        let big = m.weighted() * m.q * m.q;
        let b1_squared = (m.a0_sq.mul_add(-m.phi0, big) / m.phi1).max(0.0);
        let b0 = 0.5 * (m.a0_sq.sqrt() + b1_squared.sqrt());
        let b1 = m.a0_sq.sqrt() - b0;
        self.set(b0, b1, 0.0, 1.0, m.a1, m.a2);
    }

    /// A lowpass at `hz` with quality `q` by the bilinear transform,
    /// prewarped to the corner (0.707 alone is a Butterworth; cascade
    /// sections with a Butterworth's qualities for a steeper one). Unlike the matched filters it is not the same at
    /// every rate: it closes to nothing at Nyquist, which is what a guard
    /// against folding off the top of the band wants, and with its corner
    /// set as a share of the rate it is flat within 0.01 dB to 15 kHz at
    /// any rate from 44.1 kHz. (A matched lowpass that close to Nyquist
    /// cannot follow the analogue curve, and bulges by up to 3 dB.)
    pub fn set_guard_lowpass(&mut self, hz: f32, q: f64, sample_rate: f32) {
        let rate = f64::from(sample_rate);
        let w = TAU64 * f64::from(hz).clamp(1.0, 0.49 * rate) / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q.max(0.1));
        let b0 = 0.5 * (1.0 - cos);
        self.set(b0, 2.0 * b0, b0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha);
    }

    /// A resonant highpass at `hz` with quality `q`, matched to the
    /// analogue response.
    pub fn set_highpass(&mut self, hz: f32, q: f32, sample_rate: f32) {
        let m = Matched::new(hz, q, sample_rate);
        let b0 = m.q * m.weighted().max(0.0).sqrt() / (4.0 * m.phi1);
        self.set(b0, -2.0 * b0, b0, 1.0, m.a1, m.a2);
    }

    /// A peaking bell of `db` at `hz`, matched to the analogue response.
    /// `q` is the bell's quality in the usual (RBJ) sense: the poles sit at
    /// `q · √gain`.
    pub fn set_peak(&mut self, hz: f32, q: f32, db: f32, sample_rate: f32) {
        let gain = 10f64.powf(f64::from(db) / 20.0);
        let q = if q.is_finite() { q.max(0.05) } else { 0.707 };
        let m = Matched::new(hz, (f64::from(q) * gain.sqrt()) as f32, sample_rate);
        let squared = gain * gain;
        let r1 = m.weighted() * squared;
        let r2 = 4.0f64.mul_add((m.phi0 - m.phi1) * m.a2_term, m.a1_sq - m.a0_sq) * squared;
        let big0 = m.a0_sq;
        let big2 = (r2.mul_add(-m.phi1, r1) - big0) / (4.0 * m.phi1 * m.phi1);
        let big1 = 4.0f64.mul_add((m.phi1 - m.phi0) * big2, r2 + big0).max(0.0);
        let w = 0.5 * (big0.sqrt() + big1.sqrt());
        let b0 = 0.5 * (w + w.mul_add(w, big2).max(0.0).sqrt());
        let b1 = 0.5 * (big0.sqrt() - big1.sqrt());
        let b2 = -big2 / (4.0 * b0);
        self.set(b0, b1, b2, 1.0, m.a1, m.a2);
    }

    /// A first-order shelf with its zero at `zero_hz` and its pole at
    /// `pole_hz`, unity at DC. With the zero below the pole it lifts the
    /// top by `pole / zero`; swapping the two gives its exact inverse.
    pub fn set_shelf(&mut self, zero_hz: f32, pole_hz: f32, sample_rate: f32) {
        let rate = f64::from(sample_rate);
        let nyquist = rate * 0.49;
        let zero = TAU64 * f64::from(zero_hz).max(1.0).min(nyquist);
        let pole = TAU64 * f64::from(pole_hz).max(1.0).min(nyquist);
        // Bilinear transform of (pole / zero)(s + zero) / (s + pole), with
        // the same unwarped constant for both corners so a swap inverts it.
        let k = 2.0 * rate;
        let scale = pole / zero;
        self.set(
            scale * (k + zero),
            scale * (zero - k),
            0.0,
            k + pole,
            pole - k,
            0.0,
        );
    }

    fn set(&mut self, b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) {
        let values = [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0];
        if values.iter().all(|value| value.is_finite()) {
            [self.b0, self.b1, self.b2, self.a1, self.a2] = values;
        }
    }
}

/// The shared terms of Martin Vicanek's matched second-order designs
/// ("Matched second order digital filters", 2016): poles placed by
/// impulse invariance and zeros solved so the magnitude matches the
/// analogue filter at DC, at the corner and at Nyquist. Unlike the bilinear
/// transform, this does not cramp the response toward Nyquist, so a
/// filter's top end sounds the same at 44.1 kHz as at 192 kHz: within half
/// a decibel to 15 kHz. Between the corner and Nyquist a biquad can only
/// approximate the analogue curve, whatever its zeros: a 12 kHz lowpass at
/// 44.1 kHz is 0.65 dB off at 18 kHz, where it is already 7 dB down, and a
/// corner within a few kilohertz of Nyquist cannot be matched at all (see
/// [`Biquad::set_guard_lowpass`]).
struct Matched {
    q: f64,
    a1: f64,
    a2: f64,
    a0_sq: f64,
    a1_sq: f64,
    a2_term: f64,
    phi0: f64,
    phi1: f64,
}

impl Matched {
    fn new(hz: f32, q: f32, sample_rate: f32) -> Self {
        let rate = f64::from(sample_rate).max(1.0);
        let hz = if hz.is_finite() {
            f64::from(hz).max(1.0).min(rate * 0.49)
        } else {
            1_000.0
        };
        let q = if q.is_finite() {
            f64::from(q).max(0.05)
        } else {
            0.707
        };
        let w0 = TAU64 * hz / rate;
        let damping = 0.5 / q;
        let decay = (-damping * w0).exp();
        let a1 = if damping <= 1.0 {
            -2.0 * decay * ((1.0 - damping * damping).sqrt() * w0).cos()
        } else {
            -2.0 * decay * ((damping * damping - 1.0).sqrt() * w0).cosh()
        };
        let a2 = decay * decay;
        let phi1 = (0.5 * w0).sin().powi(2);
        Self {
            q,
            a1,
            a2,
            a0_sq: (1.0 + a1 + a2).powi(2),
            a1_sq: (1.0 - a1 + a2).powi(2),
            a2_term: -4.0 * a2,
            phi0: 1.0 - phi1,
            phi1,
        }
    }

    /// `A0 φ0 + A1 φ1 + A2 φ2`.
    fn weighted(&self) -> f64 {
        let phi2 = 4.0 * self.phi0 * self.phi1;
        self.a2_term
            .mul_add(phi2, self.a0_sq.mul_add(self.phi0, self.a1_sq * self.phi1))
    }
}

const TAU64: f64 = std::f64::consts::TAU;

/// Zero a double-precision state that has decayed to nothing or gone
/// non-finite.
pub fn flush_wide(state: &mut f64) {
    if !state.is_finite() || state.abs() < 1e-30 {
        *state = 0.0;
    }
}

/// A `tanh` saturator with first-order antiderivative anti-aliasing: the
/// output is the average of `tanh` across each step of the input, which
/// takes most of the fold-back out of hard drive without oversampling. The
/// difference quotient is taken in double precision, since at high rates
/// the steps are small and single precision would add noise.
#[derive(Debug, Clone, Copy, Default)]
pub struct SoftClip {
    last: f64,
    last_integral: f64,
}

impl SoftClip {
    /// One sample through `tanh`, anti-aliased.
    pub fn process(&mut self, input: f32) -> f32 {
        let input = f64::from(clean(input));
        let integral = log_cosh(input);
        let step = input - self.last;
        let out = if step.abs() > 1e-6 {
            (integral - self.last_integral) / step
        } else {
            (0.5 * (input + self.last)).tanh()
        };
        self.last = input;
        self.last_integral = integral;
        out.clamp(-1.0, 1.0) as f32
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.last = 0.0;
        self.last_integral = 0.0;
    }
}

/// `ln(cosh(x))`, the integral of `tanh`, without overflow.
fn log_cosh(x: f64) -> f64 {
    let size = x.abs();
    size + (-2.0 * size).exp().ln_1p() - std::f64::consts::LN_2
}

/// A [`SoftClip`] with a ceiling: `ceiling · tanh(x / ceiling)`,
/// anti-aliased. Unity slope at zero, never past the ceiling.
#[derive(Debug, Clone, Copy, Default)]
pub struct Saturator {
    clip: SoftClip,
}

impl Saturator {
    /// One sample through the saturator. A ceiling that is not finite and
    /// positive is taken as 1.
    pub fn process(&mut self, input: f32, ceiling: f32) -> f32 {
        let ceiling = if ceiling.is_finite() && ceiling > 0.0 {
            ceiling
        } else {
            1.0
        };
        ceiling * self.clip.process(input / ceiling)
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.clip.reset();
    }
}

/// Follows the level of a signal: a rectifier and a one-pole with separate
/// attack and release times.
#[derive(Debug, Clone, Copy)]
pub struct Follower {
    level: f32,
    attack: f32,
    release: f32,
}

impl Default for Follower {
    fn default() -> Self {
        Self {
            level: 0.0,
            attack: 1.0,
            release: 1.0,
        }
    }
}

impl Follower {
    /// Set the attack and release time constants.
    pub fn set_times(&mut self, attack: f32, release: f32, sample_rate: f32) {
        self.attack = coefficient(attack, sample_rate);
        self.release = coefficient(release, sample_rate);
    }

    /// Follow one sample; returns the level.
    pub fn process(&mut self, input: f32) -> f32 {
        let size = input.abs();
        let coeff = if size > self.level {
            self.attack
        } else {
            self.release
        };
        self.level = (size - self.level).mul_add(coeff, self.level);
        flush(&mut self.level);
        self.level
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.level = 0.0;
    }
}

/// The one-pole coefficient for a time constant of `seconds`.
#[must_use]
pub fn coefficient(seconds: f32, sample_rate: f32) -> f32 {
    let samples = seconds * sample_rate;
    if samples.is_finite() && samples > 1.0 {
        1.0 - (-1.0 / samples).exp()
    } else {
        1.0
    }
}

/// Band-limited reads from a [`DelayLine`] at fractional positions: a
/// 16-tap Kaiser-windowed sinc, interpolated between 512 precomputed
/// phases. Hermite interpolation loses a decibel or two of top end at a
/// fractional position near 15 kHz at 44.1 kHz, and none at 192 kHz; this
/// is flat within 0.1 dB to a third of the sample rate at any position, so
/// a delay sounds the same at every rate.
#[derive(Debug, Clone, Default)]
pub struct Sinc {
    table: Vec<f32>,
}

/// Taps each side of a sinc read.
const SINC_TAPS: usize = 16;
const SINC_PHASES: usize = 512;
/// Where the sinc's passband ends, as a share of the sample rate.
const SINC_CUTOFF: f64 = 0.5;
const SINC_BETA: f64 = 8.0;
/// The shortest read a [`Sinc`] can make, in the [`DelayLine::read`] sense
/// (1.0 is the newest sample): it needs seven newer samples than the one it
/// reads at.
pub const SINC_SHORTEST: f64 = 8.0;

impl Sinc {
    /// Build the table. Allocates; call from `prepare`.
    #[must_use]
    pub fn new() -> Self {
        let half = (SINC_TAPS / 2) as f64;
        let norm = kaiser_i0(SINC_BETA);
        let mut table = vec![0.0f32; (SINC_PHASES + 1) * SINC_TAPS];
        for (phase, kernel) in table.chunks_mut(SINC_TAPS).enumerate() {
            let frac = phase as f64 / SINC_PHASES as f64;
            let mut weights = [0.0f64; SINC_TAPS];
            for (tap, weight) in weights.iter_mut().enumerate() {
                // The distance from the read point to this tap's sample.
                let x = tap as f64 - (half - 1.0) - frac;
                let edge = x / half;
                let window = if edge.abs() < 1.0 {
                    kaiser_i0(SINC_BETA * edge.mul_add(-edge, 1.0).sqrt()) / norm
                } else {
                    0.0
                };
                let arg = std::f64::consts::PI * 2.0 * SINC_CUTOFF * x;
                let sinc = if arg.abs() < 1e-12 {
                    1.0
                } else {
                    arg.sin() / arg
                };
                *weight = 2.0 * SINC_CUTOFF * sinc * window;
            }
            let total: f64 = weights.iter().sum();
            for (slot, weight) in kernel.iter_mut().zip(weights) {
                *slot = (weight / total) as f32;
            }
        }
        Self { table }
    }

    /// The sample `delay` samples back in `line`, where 1.0 is the newest
    /// (as [`DelayLine::read`]), held between [`SINC_SHORTEST`] and what
    /// the line holds. Returns silence until the table is built.
    #[must_use]
    pub fn read(&self, line: &DelayLine, delay: f64) -> f32 {
        if self.table.len() != (SINC_PHASES + 1) * SINC_TAPS {
            return 0.0;
        }
        let longest = (line.max_delay().saturating_sub(SINC_TAPS) as f64).max(SINC_SHORTEST);
        let delay = if delay.is_finite() {
            delay.clamp(SINC_SHORTEST, longest)
        } else {
            SINC_SHORTEST
        };
        let whole = delay.floor();
        let position = (delay - whole) * SINC_PHASES as f64;
        let phase = (position.floor() as usize).min(SINC_PHASES - 1);
        let blend = (position - phase as f64) as f32;
        let here = &self.table[phase * SINC_TAPS..(phase + 1) * SINC_TAPS];
        let next = &self.table[(phase + 1) * SINC_TAPS..(phase + 2) * SINC_TAPS];
        // Tap 7 is the sample `whole` back; lower taps are newer.
        let newest = whole as usize - (SINC_TAPS / 2 - 1);
        let mut sum = 0.0f32;
        for (tap, (a, b)) in here.iter().zip(next).enumerate() {
            let weight = (b - a).mul_add(blend, *a);
            sum = weight.mul_add(line.tap(newest + tap), sum);
        }
        sum
    }
}

/// The zeroth-order modified Bessel function, for the Kaiser window.
fn kaiser_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    for k in 1..40 {
        let ratio = x / (2.0 * f64::from(k));
        term *= ratio * ratio;
        sum += term;
    }
    sum
}

/// The bilinear transform's prewarped corner for `hz`: `tan(π hz / rate)`.
/// A first-order allpass `(a + z⁻¹) / (1 + a z⁻¹)` with
/// `a = (w - 1) / (w + 1)` turns -90° at `hz`.
#[must_use]
pub fn prewarp(hz: f32, sample_rate: f32) -> f32 {
    (PI * (hz / sample_rate).clamp(1e-5, 0.49)).tan()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_steps_follow_the_tempo() {
        assert_eq!(synced_seconds(0.0, 120.0), None);
        let quarter = synced_seconds(9.0, 120.0).unwrap_or(0.0);
        assert!((quarter - 0.5).abs() < 1e-6, "{quarter}");
        let whole = synced_seconds(14.0, 1.0).unwrap_or(0.0);
        assert!((whole - LONGEST_SYNCED_SECONDS).abs() < 1e-4, "{whole}");
        assert_eq!(synced_seconds(15.0, 120.0), None);
        assert_eq!(synced_seconds(f32::NAN, 120.0), None);
        assert_eq!(SYNC_LABELS.len(), DIVISION_BEATS.len() + 1);
    }

    #[test]
    fn a_sinc_read_is_exact_on_whole_samples_and_flat_between() {
        let sinc = Sinc::new();
        let mut line = DelayLine::new(4_096);
        let rate = 44_100.0f64;
        let tone = |n: usize| (std::f64::consts::TAU * 15_000.0 * n as f64 / rate).sin() as f32;
        for n in 0..3_000 {
            line.push(tone(n));
        }
        // The newest is n = 2999 at a delay of 1.0.
        for delay in [100.0f64, 250.25, 777.5, 1_500.75] {
            let at = 3_000.0 - delay;
            let wanted = (std::f64::consts::TAU * 15_000.0 * at / rate).sin();
            let got = f64::from(sinc.read(&line, delay));
            assert!((got - wanted).abs() < 0.02, "{delay}: {got} {wanted}");
        }
        assert!((sinc.read(&line, 10.0) - line.tap(10)).abs() < 1e-6);
        assert!(sinc.read(&line, f64::NAN).is_finite());
        assert!(Sinc::default().read(&line, 20.0).abs() < f32::EPSILON);
        assert!(sinc.read(&DelayLine::default(), 20.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_guard_never_passes_six_db() {
        for value in [0.5f32, 1.0, 1.5, 10.0, 1e30, -1e30] {
            let out = guard(value);
            assert!(out.abs() <= 2.0, "{value} {out}");
        }
        assert!((guard(0.7) - 0.7).abs() < f32::EPSILON);
        assert!(guard(f32::NAN).abs() < f32::EPSILON);
    }

    /// The filter's gain at `hz`, in decibels.
    fn response_db(filter: &Biquad, hz: f64, rate: f64) -> f64 {
        let w = TAU64 * hz / rate;
        let (re1, im1) = (w.cos(), -w.sin());
        let (re2, im2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num = (
            filter.b2.mul_add(re2, filter.b1.mul_add(re1, filter.b0)),
            filter.b2.mul_add(im2, filter.b1 * im1),
        );
        let den = (
            filter.a2.mul_add(re2, filter.a1.mul_add(re1, 1.0)),
            filter.a2.mul_add(im2, filter.a1 * im1),
        );
        20.0 * (num.0.hypot(num.1) / den.0.hypot(den.1)).log10()
    }

    /// Sets a filter's design at a rate.
    type Design = fn(&mut Biquad, f32);

    /// Within half a decibel of the same design at 384 kHz at every rate,
    /// but for a lowpass with its corner high up, which at 18 kHz, between
    /// its corner and a 44.1 kHz Nyquist, a biquad can only approximate
    /// (see [`Matched`]): 0.65 dB off there, so allowed one.
    #[test]
    fn matched_filters_sound_the_same_at_every_rate() {
        let designs: [(&str, Design, f64); 3] = [
            (
                "lowpass",
                |f, rate| f.set_lowpass(12_000.0, 0.707, rate),
                1.0,
            ),
            ("highpass", |f, rate| f.set_highpass(60.0, 0.707, rate), 0.5),
            ("peak", |f, rate| f.set_peak(108.0, 1.0, 3.0, rate), 0.5),
        ];
        for (name, design, top) in designs {
            let mut reference = Biquad::new();
            design(&mut reference, 384_000.0);
            for rate in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
                let mut filter = Biquad::new();
                design(&mut filter, rate);
                for hz in [30.0, 108.0, 1_000.0, 5_000.0, 12_000.0, 15_000.0, 18_000.0] {
                    let now = response_db(&filter, hz, f64::from(rate));
                    let then = response_db(&reference, hz, 384_000.0);
                    let allowed = if hz > 15_000.0 { top } else { 0.5 };
                    assert!(
                        (now - then).abs() < allowed,
                        "{name} {rate} {hz}: {now} {then}"
                    );
                }
            }
        }
    }

    #[test]
    fn shelves_invert_each_other() {
        let mut lift = Biquad::new();
        let mut cut = Biquad::new();
        lift.set_shelf(2_000.0, 6_000.0, 48_000.0);
        cut.set_shelf(6_000.0, 2_000.0, 48_000.0);
        let mut noise = crate::dsp::Noise::new(7);
        for _ in 0..4_800 {
            let x = noise.sample();
            let y = cut.process(lift.process(x));
            assert!((x - y).abs() < 1e-4, "{x} {y}");
        }
    }

    #[test]
    fn the_soft_clip_is_tanh_on_slow_signals() {
        let mut clip = SoftClip::default();
        let mut last = 0.0;
        for n in 0..1_000 {
            let x = n as f32 * 0.004;
            last = clip.process(x);
            assert!(last.is_finite());
        }
        assert!((last - 3.996f32.tanh()).abs() < 0.01, "{last}");
        assert!(clip.process(f32::NAN).is_finite());
    }
}
