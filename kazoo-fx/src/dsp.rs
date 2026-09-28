//! Small building blocks every family shares.
//!
//! Parameter smoothing, delay lines, one-pole filters, a DC blocker, a
//! Schroeder allpass, a phasor and a noise source. Everything here is
//! real-time safe except the constructors and `resize` methods, which
//! allocate and belong in [`crate::Effect::prepare`].

use std::f32::consts::TAU;

/// A value that glides toward its target with a one-pole curve, so a knob
/// jump never clicks.
#[derive(Debug, Clone, Copy)]
pub struct Smoothed {
    // Stepped in f64: in f32 a slow glide at a high rate stalls short of
    // its target once each step falls below the value's resolution.
    value: f64,
    target: f64,
    coeff: f64,
}

impl Smoothed {
    /// Start settled at `value`.
    #[must_use]
    pub const fn new(value: f32) -> Self {
        Self {
            value: value as f64,
            target: value as f64,
            coeff: 1.0,
        }
    }

    /// Glide with a time constant of `seconds` at `sample_rate`. Zero or
    /// less (or a NaN) jumps straight to the target.
    pub fn set_time(&mut self, seconds: f32, sample_rate: f32) {
        let samples = f64::from(seconds) * f64::from(sample_rate);
        self.coeff = if samples.is_finite() && samples > 1.0 {
            -(-1.0 / samples).exp_m1()
        } else {
            1.0
        };
    }

    /// Where to glide to. A NaN or infinity is ignored.
    pub const fn set(&mut self, target: f32) {
        if target.is_finite() {
            self.target = target as f64;
        }
    }

    /// Jump to `value` at once.
    pub const fn snap(&mut self, value: f32) {
        if value.is_finite() {
            self.value = value as f64;
            self.target = value as f64;
        }
    }

    /// One sample's step toward the target; returns the new value.
    pub fn step(&mut self) -> f32 {
        self.value = (self.target - self.value).mul_add(self.coeff, self.value);
        // Land exactly once within f32 resolution of the target, so a
        // caller comparing against it sees it arrive.
        if (self.target - self.value).abs() <= self.target.abs().max(1e-30) * 1e-9 {
            self.value = self.target;
        }
        self.value as f32
    }

    /// The current value, without stepping.
    #[must_use]
    pub const fn value(&self) -> f32 {
        self.value as f32
    }

    /// The target.
    #[must_use]
    pub const fn target(&self) -> f32 {
        self.target as f32
    }
}

/// A circular delay line read at fractional positions with cubic (Hermite)
/// interpolation.
#[derive(Debug, Clone, Default)]
pub struct DelayLine {
    buffer: Vec<f32>,
    mask: usize,
    write: usize,
}

impl DelayLine {
    /// Room for at least `max_samples` of delay. Allocates.
    #[must_use]
    pub fn new(max_samples: usize) -> Self {
        let mut line = Self::default();
        line.resize(max_samples);
        line
    }

    /// Resize to hold at least `max_samples` of delay (plus the four samples
    /// interpolation needs) and clear it. Allocates.
    pub fn resize(&mut self, max_samples: usize) {
        let size = (max_samples.saturating_add(4)).next_power_of_two();
        self.buffer = vec![0.0; size];
        self.mask = size - 1;
        self.write = 0;
    }

    /// The longest delay [`Self::read`] can reach, in samples.
    #[must_use]
    pub fn max_delay(&self) -> usize {
        self.buffer.len().saturating_sub(4)
    }

    /// Silence the line.
    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
    }

    /// Write the next sample. A NaN or infinity is written as silence.
    pub fn push(&mut self, sample: f32) {
        if self.buffer.is_empty() {
            return;
        }
        self.buffer[self.write] = if sample.is_finite() { sample } else { 0.0 };
        self.write = (self.write + 1) & self.mask;
    }

    /// The sample written `delay` samples ago (1.0 = the last one pushed),
    /// interpolated. Clamped to what the line holds.
    #[must_use]
    pub fn read(&self, delay: f32) -> f32 {
        if self.buffer.is_empty() {
            return 0.0;
        }
        let delay = if delay.is_finite() {
            delay.min(self.max_delay() as f32).max(1.0)
        } else {
            1.0
        };
        let whole = delay.floor();
        let frac = delay - whole;
        let base = self.write.wrapping_sub(whole as usize);
        let at = |offset: usize| self.buffer[base.wrapping_add(offset) & self.mask];
        let x1 = at(0);
        let x2 = at(usize::MAX);
        // Between one and two samples ago there is no newer neighbour for
        // Hermite (one sample newer than the last push is the oldest sample
        // in the ring), so read linearly there.
        if whole < 2.0 {
            return frac.mul_add(x2 - x1, x1);
        }
        // Newest to oldest around the read point: x0 is one sample newer
        // than x1 (the sample `whole` ago), x2 and x3 older.
        let x0 = at(1);
        let x3 = at(usize::MAX - 1);
        hermite(x0, x1, x2, x3, frac)
    }

    /// The sample written exactly `delay` samples ago, uninterpolated.
    #[must_use]
    pub fn tap(&self, delay: usize) -> f32 {
        if self.buffer.is_empty() {
            return 0.0;
        }
        let delay = delay.clamp(1, self.max_delay().max(1));
        self.buffer[self.write.wrapping_sub(delay) & self.mask]
    }
}

/// Four-point, third-order Hermite interpolation between `x1` (at 0) and
/// `x2` (at 1), with `x0` before and `x3` after.
#[must_use]
pub fn hermite(x0: f32, x1: f32, x2: f32, x3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (x2 - x0);
    let c2 = (-0.5f32).mul_add(x3, (-2.5f32).mul_add(x1, 2.0f32.mul_add(x2, x0)));
    let c3 = 0.5f32.mul_add(x3 - x0, 1.5 * (x1 - x2));
    c3.mul_add(t, c2).mul_add(t, c1).mul_add(t, x1)
}

/// A one-pole lowpass (and, by subtraction, highpass) filter.
#[derive(Debug, Clone, Copy, Default)]
pub struct OnePole {
    state: f32,
    coeff: f32,
}

impl OnePole {
    /// Set the corner to `hz` at `sample_rate`.
    pub fn set_cutoff(&mut self, hz: f32, sample_rate: f32) {
        let hz = if hz.is_finite() {
            hz.min(sample_rate * 0.49).max(1.0)
        } else {
            1000.0
        };
        let coeff = -(-TAU * hz / sample_rate).exp_m1();
        self.coeff = if coeff.is_finite() {
            coeff.clamp(0.0, 1.0)
        } else {
            1.0
        };
    }

    /// The lowpassed sample.
    pub fn lowpass(&mut self, input: f32) -> f32 {
        self.state = (input - self.state).mul_add(self.coeff, self.state);
        flush(&mut self.state);
        self.state
    }

    /// The highpassed sample (input minus its lowpass).
    pub fn highpass(&mut self, input: f32) -> f32 {
        input - self.lowpass(input)
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// Removes DC offset: a first-order highpass at about 10 Hz.
#[derive(Debug, Clone, Copy)]
pub struct DcBlocker {
    last_in: f32,
    last_out: f32,
    pole: f32,
}

impl DcBlocker {
    /// A blocker for `sample_rate`.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let pole = if sample_rate.is_finite() && sample_rate > 0.0 {
            1.0 - TAU * 10.0 / sample_rate
        } else {
            0.995
        };
        Self {
            last_in: 0.0,
            last_out: 0.0,
            pole,
        }
    }

    /// The input with its DC taken out.
    pub fn process(&mut self, input: f32) -> f32 {
        let out = self.pole.mul_add(self.last_out, input - self.last_in);
        self.last_in = input;
        self.last_out = out;
        flush(&mut self.last_out);
        out
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.last_in = 0.0;
        self.last_out = 0.0;
    }
}

/// A Schroeder allpass: a delay line with feedback and feedforward of
/// `gain`, for diffusion and phase.
#[derive(Debug, Clone, Default)]
pub struct Allpass {
    line: DelayLine,
}

impl Allpass {
    /// An allpass able to delay up to `max_samples`. Allocates.
    #[must_use]
    pub fn new(max_samples: usize) -> Self {
        Self {
            line: DelayLine::new(max_samples),
        }
    }

    /// One sample through the allpass, delayed by `delay` samples
    /// (fractional) with coefficient `gain` (held within ±0.99).
    pub fn process(&mut self, input: f32, delay: f32, gain: f32) -> f32 {
        let gain = gain.clamp(-0.99, 0.99);
        let delayed = self.line.read(delay);
        let mut into = gain.mul_add(delayed, input);
        flush(&mut into);
        self.line.push(into);
        (-gain).mul_add(into, delayed)
    }

    /// Silence it.
    pub fn clear(&mut self) {
        self.line.clear();
    }
}

/// A phase that runs from 0 up to 1 and wraps: the heart of every LFO.
#[derive(Debug, Clone, Copy, Default)]
pub struct Phasor {
    // Held in f64: in f32 the step of a slow LFO at a high rate is rounded
    // unevenly across the cycle, so it runs fast and a triangle leans.
    phase: f64,
}

impl Phasor {
    /// Advance by `hz` at `sample_rate`; returns the phase before the step.
    pub fn next(&mut self, hz: f32, sample_rate: f32) -> f32 {
        let now = self.phase as f32;
        let step = f64::from(hz) / f64::from(sample_rate);
        if step.is_finite() {
            self.phase = (self.phase + step).rem_euclid(1.0);
        }
        now
    }

    /// The phase now, 0 up to 1.
    #[must_use]
    pub const fn phase(&self) -> f32 {
        self.phase as f32
    }

    /// Jump to `phase` (wrapped into 0 up to 1).
    pub fn set(&mut self, phase: f32) {
        if phase.is_finite() {
            self.phase = f64::from(phase).rem_euclid(1.0);
        }
    }
}

/// A small, fast, deterministic noise source (xorshift32).
#[derive(Debug, Clone, Copy)]
pub struct Noise {
    state: u32,
}

impl Noise {
    /// Seeded noise; a zero seed is replaced, since xorshift would stick.
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 0x9E37_79B9 } else { seed },
        }
    }

    /// The next raw 32-bit value.
    pub const fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// The next sample, evenly spread over -1 up to 1.
    pub fn sample(&mut self) -> f32 {
        // The top 24 bits fill an f32 mantissa exactly.
        let unit = (self.next_u32() >> 8) as f32 / 16_777_216.0;
        unit.mul_add(2.0, -1.0)
    }
}

/// Decibels to linear gain.
#[must_use]
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Linear gain to decibels; silence is -120 dB.
#[must_use]
pub fn gain_to_db(gain: f32) -> f32 {
    if gain > 1e-6 {
        20.0 * gain.log10()
    } else {
        -120.0
    }
}

/// Zero a state variable that has decayed into the denormal range (or gone
/// non-finite), so feedback paths neither slow to a crawl nor stay poisoned.
pub fn flush(state: &mut f32) {
    if !state.is_finite() || state.abs() < 1e-20 {
        *state = 0.0;
    }
}

/// The lowest sample rate an effect agrees to run at.
pub const MIN_RATE: f32 = 8_000.0;
/// The highest sample rate an effect agrees to run at.
pub const MAX_RATE: f32 = 768_000.0;

/// A host sample rate made safe to design and allocate for.
///
/// A NaN, an infinity, zero or a negative rate becomes 48 kHz, and anything
/// else is held between [`MIN_RATE`] and [`MAX_RATE`]. Every effect's `prepare`
/// passes its rate through this, so a garbage rate can neither panic nor
/// ask for gigabytes.
#[must_use]
pub fn sane_rate(rate: f32) -> f32 {
    if rate.is_finite() && rate > 0.0 {
        rate.clamp(MIN_RATE, MAX_RATE)
    } else {
        48_000.0
    }
}

/// Silence non-finite samples in place.
pub fn sanitise(block: &mut [f32]) {
    for sample in block {
        if !sample.is_finite() {
            *sample = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delay_line_returns_what_went_in_that_long_ago() {
        let mut line = DelayLine::new(64);
        for n in 0..32 {
            line.push(n as f32);
        }
        // The last pushed is 31: one sample ago.
        assert!((line.read(1.0) - 31.0).abs() < 1e-4);
        assert!((line.read(10.0) - 22.0).abs() < 1e-4);
        assert!((line.read(10.5) - 21.5).abs() < 1e-3);
        assert!((line.tap(5) - 27.0).abs() < 1e-6);
    }

    #[test]
    fn a_short_delay_never_reaches_round_to_the_oldest_sample() {
        let mut line = DelayLine::new(8);
        // Wrap the ring several times so its oldest sample is far from zero.
        for n in 0..100 {
            line.push(n as f32);
        }
        for tenth in 10..=20 {
            let delay = tenth as f32 / 10.0;
            let want = 100.0 - delay;
            let got = line.read(delay);
            assert!((got - want).abs() < 1e-4, "{delay}: {got} != {want}");
        }
    }

    #[test]
    fn an_allpass_does_not_carry_subnormals() {
        let mut allpass = Allpass::new(16);
        allpass.process(1.0, 3.0, 0.7);
        for _ in 0..20_000 {
            allpass.process(0.0, 3.0, 0.7);
        }
        for delay in 1..=16 {
            let held = allpass.line.tap(delay);
            assert!(held == 0.0 || held.is_normal(), "{delay}: {held:e}");
        }
    }

    #[test]
    fn a_slow_phasor_keeps_time_at_high_rates() {
        let rate = 384_000.0;
        let hz = 0.02;
        let mut phasor = Phasor::default();
        let steps = (rate / hz) as usize / 4;
        for _ in 0..steps {
            phasor.next(hz, rate);
        }
        // A quarter cycle in, the phase is a quarter.
        assert!((phasor.phase() - 0.25).abs() < 1e-4, "{}", phasor.phase());
    }

    #[test]
    fn a_slow_glide_reaches_its_target_at_high_rates() {
        let mut smoothed = Smoothed::new(0.0);
        smoothed.set_time(0.5, 192_000.0);
        smoothed.set(0.123_456);
        for _ in 0..192_000 * 40 {
            smoothed.step();
        }
        assert!(
            (smoothed.value() - 0.123_456).abs() == 0.0,
            "{}",
            smoothed.value()
        );
    }

    #[test]
    fn empty_and_tiny_things_do_not_panic() {
        let line = DelayLine::default();
        assert!(line.read(3.0).abs() < f32::EPSILON);
        let mut short = DelayLine::new(0);
        short.push(1.0);
        assert!(short.read(0.5).is_finite());
        let mut pole = OnePole::default();
        pole.set_cutoff(1000.0, 1.5);
        pole.set_cutoff(1000.0, 0.0);
        pole.set_cutoff(1000.0, -3.0);
        assert!(pole.lowpass(1.0).is_finite());
    }

    #[test]
    fn a_garbage_rate_becomes_a_sane_one() {
        assert!((sane_rate(f32::NAN) - 48_000.0).abs() < f32::EPSILON);
        assert!((sane_rate(0.0) - 48_000.0).abs() < f32::EPSILON);
        assert!((sane_rate(-44_100.0) - 48_000.0).abs() < f32::EPSILON);
        assert!((sane_rate(0.3) - MIN_RATE).abs() < f32::EPSILON);
        assert!((sane_rate(1e9) - MAX_RATE).abs() < f32::EPSILON);
        assert!((sane_rate(96_000.0) - 96_000.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_delay_line_refuses_poison() {
        let mut line = DelayLine::new(8);
        line.push(f32::NAN);
        line.push(f32::INFINITY);
        assert!(line.read(1.0).abs() < f32::EPSILON);
        assert!(line.read(f32::NAN).is_finite());
    }

    #[test]
    fn smoothing_reaches_its_target_and_ignores_nan() {
        let mut value = Smoothed::new(0.0);
        value.set_time(0.001, 48_000.0);
        value.set(1.0);
        value.set(f32::NAN);
        for _ in 0..2_000 {
            value.step();
        }
        assert!((value.value() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn an_allpass_keeps_energy() {
        let mut allpass = Allpass::new(128);
        let mut energy = 0.0;
        allpass.process(1.0, 37.0, 0.6);
        energy += 1.0f32;
        let mut out_energy = 0.0;
        for _ in 0..20_000 {
            let out = allpass.process(0.0, 37.0, 0.6);
            out_energy += out * out;
        }
        // The first output sample was -gain times the impulse.
        out_energy += 0.36;
        assert!((out_energy - energy).abs() < 0.02, "{out_energy}");
    }

    #[test]
    fn noise_stays_in_range_and_is_not_stuck() {
        let mut noise = Noise::new(0);
        let mut last = noise.sample();
        let mut changed = 0;
        for _ in 0..1_000 {
            let now = noise.sample();
            assert!((-1.0..=1.0).contains(&now));
            if (now - last).abs() > f32::EPSILON {
                changed += 1;
            }
            last = now;
        }
        assert!(changed > 990);
    }

    #[test]
    fn dc_is_removed() {
        let mut blocker = DcBlocker::new(48_000.0);
        let mut out = 1.0;
        for _ in 0..48_000 {
            out = blocker.process(0.5);
        }
        assert!(out.abs() < 1e-3, "{out}");
    }
}
