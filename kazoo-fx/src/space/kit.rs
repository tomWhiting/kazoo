//! Pieces every effect in the space family shares.
//!
//! Block bookkeeping (how many frames to process, silencing the rest), input
//! cleaning, a soft safety ceiling, the equal-power wet/dry law, decay
//! maths, a knob store that glides every knob, a linear ramp that lands
//! exactly on its target, a Schroeder allpass whose inside can be tapped,
//! and a stereo peak limiter. Nothing here allocates except
//! [`TapAllpass::new`].

use std::f32::consts::{FRAC_PI_2, TAU};

use crate::ParamSpec;
use std::sync::Arc;

use crate::dsp::Smoothed;

use super::line::{Kernel, Line};

/// Samples between recomputations of derived coefficients (filter designs,
/// decay gains). Knob values themselves glide every sample.
pub const CONTROL: usize = 16;

/// The loudest input sample an effect takes in. Anything beyond +24 dBFS is
/// not music, and holding it here keeps every internal sum finite.
const INPUT_LIMIT: f32 = 16.0;

/// Natural log of 1000: a decay of 60 dB is a gain of `exp(-LN_1000)`.
const LN_1000: f32 = 6.907_755;

/// The lowest and highest sample rates the family runs at.
pub const MIN_RATE: f32 = 8_000.0;
pub const MAX_RATE: f32 = 384_000.0;

/// The sample rate to prepare for, or `None` when the one given is not one
/// the family can run at: not a number, or outside 8 kHz to 384 kHz.
/// Running at a rate other than the host's would put every delay, pitch and
/// filter in the wrong place, so an unusable rate leaves the effect silent
/// rather than quietly mistuned.
#[must_use]
pub fn usable_rate(sample_rate: f32) -> Option<f32> {
    if (MIN_RATE..=MAX_RATE).contains(&sample_rate) {
        Some(sample_rate)
    } else {
        None
    }
}

/// The shortest delay any space effect reads a [`crate::dsp::DelayLine`]
/// at. The line's four-point interpolation needs a sample on each side of
/// the read point, and below two samples of delay the newer neighbour is not
/// there yet. (The reverbs' own [`Line`]s have a floor of their own.)
pub const MIN_READ: f32 = 2.0;

/// The fastest a delay line's read point may move against its write point,
/// in samples per sample, when a knob changes its length: 2% of real time,
/// a pitch bend of at most about a third of a semitone on whatever is in
/// the line.
pub const MAX_SLEW: f32 = 0.02;

/// How many frames of this block to process: the shortest of the four
/// slices. Every output sample past that is silenced here, as the
/// [`crate::Effect::process`] contract asks.
pub fn frames(input: [&[f32]; 2], output: &mut [&mut [f32]; 2]) -> usize {
    let frames = input[0]
        .len()
        .min(input[1].len())
        .min(output[0].len())
        .min(output[1].len());
    for side in output.iter_mut() {
        side[frames..].fill(0.0);
    }
    frames
}

/// Silence both outputs completely (an effect that has not been prepared).
pub fn silence(output: &mut [&mut [f32]; 2]) {
    for side in output.iter_mut() {
        side.fill(0.0);
    }
}

/// An input sample made safe: NaN and infinity become silence, absurd
/// levels are held at +24 dBFS, and anything below -400 dBFS (a denormal
/// or nearly) is silence too, so it never starts a slow denormal tail.
#[must_use]
pub fn clean(sample: f32) -> f32 {
    if sample.is_finite() && sample.abs() >= 1e-20 {
        sample.clamp(-INPUT_LIMIT, INPUT_LIMIT)
    } else {
        0.0
    }
}

/// A soft safety ceiling: untouched up to full scale, then bending smoothly
/// (the slope is continuous at the knee) toward +6 dBFS, which it never
/// reaches. Used on wet signals and inside feedback loops that must stay
/// bounded whatever the knobs say; never on the dry signal, which passes
/// through the space effects untouched. A non-finite sample comes out
/// silent.
#[must_use]
pub fn ceiling(sample: f32) -> f32 {
    soft_limit(sample, 1.0, 2.0)
}

/// Untouched up to `knee`, then bending smoothly (slope continuous at the
/// knee) toward `top`, which it never reaches. A non-finite sample comes
/// out silent.
#[must_use]
pub fn soft_limit(sample: f32, knee: f32, top: f32) -> f32 {
    let size = sample.abs();
    if size <= knee {
        sample
    } else if size.is_finite() {
        let room = top - knee;
        room.mul_add(((size - knee) / room).tanh(), knee)
            .copysign(sample)
    } else {
        0.0
    }
}

/// Dry and wet gains for `mix` (0 all dry, 1 all wet) under the equal-power
/// law, so a sweep of the knob keeps the loudness even.
#[must_use]
pub fn equal_power(mix: f32) -> (f32, f32) {
    let angle = mix.clamp(0.0, 1.0) * FRAC_PI_2;
    (angle.cos(), angle.sin())
}

/// The gain that, applied once every `period` seconds, decays a signal by
/// 60 dB in `rt60` seconds. Held below 1 so no loop built on it can grow.
#[must_use]
pub fn decay_gain(period: f32, rt60: f32) -> f32 {
    if !(period.is_finite() && rt60.is_finite()) || rt60 <= 0.0 || period <= 0.0 {
        return 0.0;
    }
    (-LN_1000 * period / rt60).exp().min(0.999_99)
}

/// A pole radius that decays by 60 dB in `rt60` seconds at `sample_rate`.
/// Unlike [`decay_gain`] it is not held back from one (a ten-second decay at
/// 192 kHz needs a radius within a few millionths of it), only kept below.
#[must_use]
pub fn decay_radius(rt60: f32, sample_rate: f32) -> f32 {
    let samples = rt60 * sample_rate;
    if !samples.is_finite() || samples <= 0.0 {
        return 0.0;
    }
    (-LN_1000 / samples).exp().min(1.0 - f32::EPSILON)
}

/// Whether two sets of values are identical, bit for bit (a NaN never
/// matches a number), for skipping a redesign when nothing has moved.
#[must_use]
pub fn unchanged(now: &[f32], before: &[f32]) -> bool {
    now.len() == before.len()
        && now
            .iter()
            .zip(before)
            .all(|(a, b)| a.to_bits() == b.to_bits())
}

/// One cycle of a sine, from a phase of 0 up to 1.
#[must_use]
pub fn sine(phase: f32) -> f32 {
    (TAU * phase).sin()
}

/// Mid/side width on a stereo pair: 0 folds to mono, 1 leaves it as it is.
#[must_use]
pub fn widen(left: f32, right: f32, width: f32) -> (f32, f32) {
    let mid = 0.5 * (left + right);
    let side = 0.5 * (left - right) * width;
    (mid + side, mid - side)
}

/// An effect's knobs: each one's target, clamped to its range, and a value
/// gliding toward it every sample so no knob move clicks.
#[derive(Debug, Clone, Copy)]
pub struct Knobs<const N: usize> {
    specs: &'static [ParamSpec; N],
    glide: [Smoothed; N],
}

impl<const N: usize> Knobs<N> {
    /// Every knob at its default, settled.
    #[must_use]
    pub fn new(specs: &'static [ParamSpec; N]) -> Self {
        Self {
            specs,
            glide: std::array::from_fn(|i| Smoothed::new(specs[i].default)),
        }
    }

    /// Set each knob's glide time (seconds) for `sample_rate` and settle
    /// every knob on its target.
    pub fn prepare(&mut self, sample_rate: f32, seconds: &[f32; N]) {
        for (glide, &time) in self.glide.iter_mut().zip(seconds) {
            glide.set_time(time, sample_rate);
            glide.snap(glide.target());
        }
    }

    /// Turn knob `index` to `value`, clamped to its range. A NaN or an
    /// index past the end is ignored.
    pub fn set(&mut self, index: usize, value: f32) {
        if value.is_nan() {
            return;
        }
        if let (Some(spec), Some(glide)) = (self.specs.get(index), self.glide.get_mut(index)) {
            glide.set(spec.clamp(value));
        }
    }

    /// One sample's glide for every knob.
    pub fn step(&mut self) {
        for glide in &mut self.glide {
            glide.step();
        }
    }

    /// Knob `index`'s gliding value.
    #[must_use]
    pub const fn get(&self, index: usize) -> f32 {
        self.glide[index].value()
    }

    /// Knob `index`'s target: where a stepped knob actually sits.
    #[must_use]
    pub const fn target(&self, index: usize) -> f32 {
        self.glide[index].target()
    }
}

/// A value that follows its target no faster than a set speed, for the
/// lengths of delay lines: a knob that stretches a line changes the pitch of
/// what is in it, and a speed limit caps that bend however far or fast the
/// knob moves.
#[derive(Debug, Clone, Copy)]
pub struct Slew {
    value: f32,
    target: f32,
    speed: f32,
}

impl Slew {
    /// Settled at `value`, moving at most `speed` per sample.
    #[must_use]
    pub const fn new(value: f32, speed: f32) -> Self {
        Self {
            value,
            target: value,
            speed,
        }
    }

    /// The largest step per sample.
    pub fn set_speed(&mut self, speed: f32) {
        if speed.is_finite() && speed > 0.0 {
            self.speed = speed;
        }
    }

    /// Where to head. A NaN or infinity is ignored.
    pub const fn set(&mut self, target: f32) {
        if target.is_finite() {
            self.target = target;
        }
    }

    /// Jump to `value` at once.
    pub const fn snap(&mut self, value: f32) {
        if value.is_finite() {
            self.value = value;
            self.target = value;
        }
    }

    /// One sample's move; returns the new value.
    pub fn next(&mut self) -> f32 {
        let gap = self.target - self.value;
        self.value += gap.clamp(-self.speed, self.speed);
        self.value
    }

    /// The value now.
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    /// Whether it has arrived.
    #[cfg(test)]
    #[must_use]
    pub const fn settled(&self) -> bool {
        self.value.to_bits() == self.target.to_bits()
    }
}

/// A value that moves in a straight line to its target over a set number of
/// samples and then sits exactly on it. Used for switches (freeze, model
/// changes, filters in or out) where a one-pole glide would never quite
/// arrive.
#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    value: f32,
    target: f32,
    step: f32,
}

impl Ramp {
    /// Settled at `value`.
    #[must_use]
    pub const fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
        }
    }

    /// Head for `target`, arriving in `samples` samples (at least one).
    pub fn set(&mut self, target: f32, samples: f32) {
        if !target.is_finite() {
            return;
        }
        self.target = target;
        let samples = if samples.is_finite() {
            samples.max(1.0)
        } else {
            1.0
        };
        self.step = (target - self.value).abs() / samples;
    }

    /// Jump to `value` at once.
    pub const fn snap(&mut self, value: f32) {
        if value.is_finite() {
            self.value = value;
            self.target = value;
            self.step = 0.0;
        }
    }

    /// One sample's move; returns the new value.
    pub fn next(&mut self) -> f32 {
        if self.value < self.target {
            self.value = (self.value + self.step).min(self.target);
        } else if self.value > self.target {
            self.value = (self.value - self.step).max(self.target);
        }
        self.value
    }

    /// The value now.
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value
    }

    /// The target.
    #[must_use]
    pub const fn target(&self) -> f32 {
        self.target
    }
}

/// A Schroeder allpass (the same maths as [`crate::dsp::Allpass`], with its
/// state flushed of denormals) on a windowed-sinc [`Line`], whose inside can
/// also be read at any point, as the Dattorro plate's output taps need.
#[derive(Debug, Clone, Default)]
pub struct TapAllpass {
    line: Line,
}

impl TapAllpass {
    /// Room for `max_samples` of delay, read through `kernel`. Allocates.
    #[must_use]
    pub fn new(max_samples: usize, kernel: &Arc<Kernel>) -> Self {
        Self {
            line: Line::new(max_samples, kernel),
        }
    }

    /// One sample through, delayed by `delay` samples with coefficient
    /// `gain` (held within ±0.99).
    pub fn process(&mut self, input: f32, delay: f32, gain: f32) -> f32 {
        let gain = gain.clamp(-0.99, 0.99);
        let delayed = self.line.read(delay);
        let mut into = gain.mul_add(delayed, input);
        crate::dsp::flush(&mut into);
        self.line.push(into);
        (-gain).mul_add(into, delayed)
    }

    /// The inside of the allpass `delay` samples back.
    #[must_use]
    pub fn tap(&self, delay: f32) -> f32 {
        self.line.read(delay)
    }

    /// Silence it.
    pub fn clear(&mut self) {
        self.line.clear();
    }
}

/// A stereo-linked peak limiter without look-ahead: a fast attack pulls the
/// gain down as soon as the level passes the threshold, a slow release lets
/// it back up. What its attack lets through is caught by [`ceiling`].
#[derive(Debug, Clone, Copy)]
pub struct Limiter {
    gain: f32,
    attack: f32,
    release: f32,
    threshold: f32,
}

impl Limiter {
    /// A limiter holding peaks near `threshold` (linear) at `sample_rate`.
    #[must_use]
    pub fn new(threshold: f32, sample_rate: f32) -> Self {
        Self {
            gain: 1.0,
            attack: 1.0 - (-1.0 / (0.000_5 * sample_rate)).exp(),
            release: 1.0 - (-1.0 / (0.15 * sample_rate)).exp(),
            threshold,
        }
    }

    /// The pair, limited.
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let peak = left.abs().max(right.abs());
        let wanted = if peak > self.threshold {
            self.threshold / peak
        } else {
            1.0
        };
        let rate = if wanted < self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain = (wanted - self.gain).mul_add(rate, self.gain);
        if !self.gain.is_finite() {
            self.gain = 1.0;
        }
        (ceiling(left * self.gain), ceiling(right * self.gain))
    }

    /// Back to full gain.
    pub const fn reset(&mut self) {
        self.gain = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slew_never_moves_faster_than_its_speed_and_lands_exactly() {
        let mut slew = Slew::new(0.0, 0.1);
        slew.set(1.0);
        let mut last = 0.0;
        for _ in 0..10 {
            let now = slew.next();
            assert!(now - last <= 0.1 + 1e-6);
            last = now;
        }
        assert!(slew.settled() && (slew.value() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn unusable_rates_are_refused() {
        assert!(usable_rate(4_000.0).is_none());
        assert!(usable_rate(768_000.0).is_none());
        assert!(usable_rate(f32::NAN).is_none());
        assert!(usable_rate(44_100.0).is_some());
    }

    #[test]
    fn a_soft_limit_is_transparent_to_its_knee() {
        assert!((soft_limit(1.4, 1.5, 2.0) - 1.4).abs() < f32::EPSILON);
        assert!(soft_limit(100.0, 1.5, 2.0) <= 2.0);
        let slope = (soft_limit(1.501, 1.5, 2.0) - soft_limit(1.5, 1.5, 2.0)) / 0.001;
        assert!((slope - 1.0).abs() < 0.01, "{slope}");
    }

    #[test]
    fn the_ceiling_is_transparent_below_full_scale_and_bounded_above() {
        assert!((ceiling(0.7) - 0.7).abs() < f32::EPSILON);
        assert!((ceiling(-1.0) + 1.0).abs() < f32::EPSILON);
        assert!(ceiling(1_000.0) <= 2.0);
        assert!(ceiling(-1_000.0) >= -2.0);
        assert!(ceiling(f32::NAN).abs() < f32::EPSILON);
        // The slope is continuous at the knee.
        let slope = (ceiling(1.001) - ceiling(1.0)) / 0.001;
        assert!((slope - 1.0).abs() < 0.01, "{slope}");
    }

    #[test]
    fn a_ramp_lands_exactly() {
        let mut ramp = Ramp::new(0.0);
        ramp.set(1.0, 10.0);
        for _ in 0..10 {
            ramp.next();
        }
        assert!((ramp.value() - 1.0).abs() < f32::EPSILON);
        ramp.set(0.0, 4.0);
        for _ in 0..5 {
            ramp.next();
        }
        assert!(ramp.value().abs() < f32::EPSILON);
    }

    #[test]
    fn decay_gain_gives_sixty_decibels_in_rt60() {
        let gain = decay_gain(0.1, 2.0);
        // Twenty periods of 0.1 s make 2 s: 60 dB down.
        let total = gain.powi(20);
        assert!((total - 0.001).abs() < 1e-5, "{total}");
        assert!(decay_gain(0.1, f32::NAN).abs() < f32::EPSILON);
    }

    #[test]
    fn the_limiter_holds_a_loud_signal_down() {
        let mut limiter = Limiter::new(1.0, 48_000.0);
        let mut peak = 0.0f32;
        for n in 0..48_000 {
            let x = 8.0 * sine(n as f32 * 440.0 / 48_000.0);
            let (l, r) = limiter.process(x, x);
            if n > 4_800 {
                peak = peak.max(l.abs()).max(r.abs());
            }
        }
        assert!(peak < 1.2, "{peak}");
    }
}
