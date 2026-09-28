//! Parts the lofi effects share.
//!
//! Worn media fail in the same few ways, so the machines here are built
//! from the same few parts:
//!
//! - [`Biquad`] and [`Butterworth`]: the filters every tape path, groove and
//!   converter is made of.
//! - [`Oversampler2`] and [`Oversampler4`]: half-band oversampling around a
//!   nonlinearity, so saturation does not alias.
//! - [`Wobble`]: the transport's speed error, built from the rotating parts
//!   that cause it.
//! - [`Dropouts`]: moments where the tape lifts off the head.
//! - [`Hiss`]: band-shaped noise.
//! - [`Magnetic`]: Jiles–Atherton tape magnetisation with a bias blend.
//! - [`head_loss`] and [`head_corner`]: gap, azimuth and spacing loss at a
//!   playback head.
//!
//! Everything here is real-time safe except the `new` and `prepare`
//! functions that size buffers.

use std::f32::consts::{FRAC_1_SQRT_2, PI, TAU};

use crate::dsp::{Noise, OnePole, db_to_gain, flush};

/// Samples between control-rate updates: knobs that move filter
/// coefficients are glided at this rate, audio-rate gains every sample.
pub const CONTROL: usize = 32;

/// The sample rate to work at: `sample_rate` held within 8 kHz and
/// 768 kHz, or 48 kHz if it is not a number at all.
#[must_use]
pub const fn sane_rate(sample_rate: f32) -> f32 {
    if sample_rate.is_finite() {
        sample_rate.clamp(8_000.0, 768_000.0)
    } else {
        48_000.0
    }
}

/// `x`, or silence if it is not finite.
#[must_use]
pub const fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Hold a sample under +6 dBFS: unity up to full scale, then a soft knee
/// that approaches but never passes 1.999. Non-finite samples become silence.
#[must_use]
pub fn ceiling(x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    let size = x.abs();
    if size <= 1.0 {
        x
    } else {
        let over = size - 1.0;
        (0.999f32 * over / (1.0 + over) + 1.0).copysign(x)
    }
}

/// The loudest input an effect is given, +36 dBFS: anything beyond is held
/// here so no internal sum can overflow.
pub const HEADROOM: f32 = 64.0;

/// Run `tick` over one block as [`crate::Effect::process`] describes: the
/// shortest slice sets the length, non-finite inputs arrive as silence and
/// huge ones are held to [`HEADROOM`], every output goes through
/// [`ceiling`] and the rest of each output is silenced.
pub fn run_block(
    input: [&[f32]; 2],
    output: [&mut [f32]; 2],
    mut tick: impl FnMut(f32, f32) -> (f32, f32),
) {
    let [in_left, in_right] = input;
    let [out_left, out_right] = output;
    let len = in_left
        .len()
        .min(in_right.len())
        .min(out_left.len())
        .min(out_right.len());
    for (((left, right), out_l), out_r) in in_left
        .iter()
        .zip(in_right)
        .zip(out_left.iter_mut())
        .zip(out_right.iter_mut())
    {
        let (wet_l, wet_r) = tick(
            finite(*left).clamp(-HEADROOM, HEADROOM),
            finite(*right).clamp(-HEADROOM, HEADROOM),
        );
        *out_l = ceiling(wet_l);
        *out_r = ceiling(wet_r);
    }
    out_left[len..].fill(0.0);
    out_right[len..].fill(0.0);
}

/// Decibels to gain, remembering the last answer: a smoothed knob sits
/// still most of the time, so the power is rarely taken.
#[derive(Debug, Clone, Copy)]
pub struct Decibels {
    db: f32,
    gain: f32,
}

impl Decibels {
    /// Starting at 0 dB.
    #[must_use]
    pub const fn new() -> Self {
        Self { db: 0.0, gain: 1.0 }
    }

    /// `db` as linear gain.
    pub fn gain(&mut self, db: f32) -> f32 {
        if db.to_bits() != self.db.to_bits() {
            self.db = db;
            self.gain = db_to_gain(db);
        }
        self.gain
    }
}

/// A value worked out every `period` samples and ramped between: for
/// gains that follow an envelope, where the working out is costly and a
/// millisecond or less of lag is not.
#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    from: f32,
    to: f32,
    left: usize,
    period: usize,
}

impl Ramp {
    /// Settled at `value`, working out every `period` samples.
    #[must_use]
    pub const fn new(value: f32, period: usize) -> Self {
        Self {
            from: value,
            to: value,
            left: 0,
            period: if period == 0 { 1 } else { period },
        }
    }

    /// Settle at `value` and restart the schedule.
    pub const fn snap(&mut self, value: f32) {
        self.from = value;
        self.to = value;
        self.left = 0;
    }

    /// This sample's value; `work` gives the next target when one is due.
    pub fn next(&mut self, work: impl FnOnce() -> f32) -> f32 {
        if self.left == 0 {
            self.from = self.to;
            self.to = finite(work());
            self.left = self.period;
        }
        self.left -= 1;
        lerp(
            self.from,
            self.to,
            1.0 - self.left as f32 / self.period as f32,
        )
    }
}

/// Linear interpolation from `a` to `b` by `t`.
#[must_use]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    (b - a).mul_add(t, a)
}

/// Evenly spread over 0 up to 1.
pub fn unit(noise: &mut Noise) -> f32 {
    (noise.next_u32() >> 8) as f32 / 16_777_216.0
}

/// True with probability `p` (held within 0 and 1).
pub fn chance(noise: &mut Noise, p: f32) -> bool {
    unit(noise) < p
}

/// A second-order filter section (direct form II transposed), computed in
/// double precision so low corners at high sample rates stay clean.
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

/// `hz` held inside the band a filter at `sample_rate` can reach, as the
/// cosine and sine of its angular frequency.
fn angle(hz: f32, sample_rate: f32) -> (f64, f64) {
    let rate = f64::from(sane_rate(sample_rate));
    let hz = if hz.is_finite() {
        f64::from(hz).clamp(1.0, rate * 0.49)
    } else {
        1_000.0
    };
    let w = std::f64::consts::TAU * hz / rate;
    (w.cos(), w.sin())
}

/// A Q held to something a filter can use.
fn sane_q(q: f32) -> f64 {
    if q.is_finite() {
        f64::from(q).clamp(0.05, 40.0)
    } else {
        std::f64::consts::FRAC_1_SQRT_2
    }
}

/// Decibels held to ±48 as a shelf or peak amplitude (square root of the
/// power gain).
fn shelf_amp(db: f32) -> f64 {
    let db = if db.is_finite() {
        f64::from(db).clamp(-48.0, 48.0)
    } else {
        0.0
    };
    10f64.powf(db / 40.0)
}

impl Biquad {
    /// A section that passes its input unchanged.
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

    fn set(&mut self, b: [f64; 3], a: [f64; 3]) {
        let [b0, b1, b2] = b;
        let [a0, a1, a2] = a;
        let all = [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0];
        if all.iter().all(|c| c.is_finite()) {
            [self.b0, self.b1, self.b2, self.a1, self.a2] = all;
        }
    }

    /// A lowpass at `hz` with resonance `q`.
    pub fn lowpass(&mut self, hz: f32, q: f32, sample_rate: f32) {
        let (cos, sin) = angle(hz, sample_rate);
        let alpha = sin / (2.0 * sane_q(q));
        let b1 = 1.0 - cos;
        self.set(
            [b1 / 2.0, b1, b1 / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        );
    }

    /// A highpass at `hz` with resonance `q`.
    pub fn highpass(&mut self, hz: f32, q: f32, sample_rate: f32) {
        let (cos, sin) = angle(hz, sample_rate);
        let alpha = sin / (2.0 * sane_q(q));
        let b0 = f64::midpoint(1.0, cos);
        self.set([b0, -2.0 * b0, b0], [1.0 + alpha, -2.0 * cos, 1.0 - alpha]);
    }

    /// A bandpass at `hz` with bandwidth set by `q`, unity at the centre.
    pub fn bandpass(&mut self, hz: f32, q: f32, sample_rate: f32) {
        let (cos, sin) = angle(hz, sample_rate);
        let alpha = sin / (2.0 * sane_q(q));
        self.set([alpha, 0.0, -alpha], [1.0 + alpha, -2.0 * cos, 1.0 - alpha]);
    }

    /// A peaking bell of `db` at `hz`.
    pub fn peak(&mut self, hz: f32, q: f32, db: f32, sample_rate: f32) {
        let (cos, sin) = angle(hz, sample_rate);
        let alpha = sin / (2.0 * sane_q(q));
        let amp = shelf_amp(db);
        self.set(
            [
                alpha.mul_add(amp, 1.0),
                -2.0 * cos,
                (-alpha).mul_add(amp, 1.0),
            ],
            [1.0 + alpha / amp, -2.0 * cos, 1.0 - alpha / amp],
        );
    }

    /// A high shelf of `db` above `hz` (shelf slope 1).
    pub fn high_shelf(&mut self, hz: f32, db: f32, sample_rate: f32) {
        let (cos, sin) = angle(hz, sample_rate);
        let amp = shelf_amp(db);
        let root = 2.0 * amp.sqrt() * sin * std::f64::consts::FRAC_1_SQRT_2;
        let (up, down) = (amp + 1.0, amp - 1.0);
        self.set(
            [
                amp * (down.mul_add(cos, up) + root),
                -2.0 * amp * up.mul_add(cos, down),
                amp * (down.mul_add(cos, up) - root),
            ],
            [
                down.mul_add(-cos, up) + root,
                2.0 * up.mul_add(-cos, down),
                down.mul_add(-cos, up) - root,
            ],
        );
    }

    /// One sample through the section.
    pub fn process(&mut self, input: f32) -> f32 {
        let x = f64::from(input);
        let y = self.b0.mul_add(x, self.z1);
        self.z1 = self.b1.mul_add(x, (-self.a1).mul_add(y, self.z2));
        self.z2 = self.b2.mul_add(x, -self.a2 * y);
        if !y.is_finite() || !self.z1.is_finite() || !self.z2.is_finite() {
            self.reset();
            return 0.0;
        }
        if self.z1.abs() < 1e-30 {
            self.z1 = 0.0;
        }
        if self.z2.abs() < 1e-30 {
            self.z2 = 0.0;
        }
        y as f32
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Its gain for a steady input.
    #[must_use]
    pub fn dc_gain(&self) -> f32 {
        let den = 1.0 + self.a1 + self.a2;
        let gain = (self.b0 + self.b1 + self.b2) / den;
        if gain.is_finite() { gain as f32 } else { 0.0 }
    }

    /// Set the state to where a steady `input` would have left it, so a
    /// constant (a carrier, an offset) passes without a start-up transient.
    /// Returns the steady output.
    pub fn settle(&mut self, input: f32) -> f32 {
        let x = f64::from(finite(input));
        let y = f64::from(self.dc_gain()) * x;
        self.z2 = self.b2.mul_add(x, -self.a2 * y);
        self.z1 = self.b1.mul_add(x, (-self.a1).mul_add(y, self.z2));
        y as f32
    }
}

/// The section Qs of Butterworth filters of order 2, 4, 6 and 8.
const BUTTERWORTH_Q: [&[f32]; 4] = [
    &[FRAC_1_SQRT_2],
    &[0.541_196_1, 1.306_563],
    &[0.517_638_1, FRAC_1_SQRT_2, 1.931_851_6],
    &[0.509_795_6, 0.601_344_9, 0.899_976_2, 2.562_915_4],
];

/// A Butterworth lowpass or highpass of order 2, 4, 6 or 8: maximally flat,
/// the plainest steep filter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Butterworth {
    stages: [Biquad; 4],
    count: usize,
}

impl Butterworth {
    /// `sections` of 1 up to 4 give order 2 up to 8.
    fn qs(sections: usize) -> &'static [f32] {
        BUTTERWORTH_Q[sections.clamp(1, 4) - 1]
    }

    /// A lowpass of `sections` × 2 poles at `hz`.
    pub fn lowpass(&mut self, sections: usize, hz: f32, sample_rate: f32) {
        let qs = Self::qs(sections);
        self.count = qs.len();
        for (stage, q) in self.stages.iter_mut().zip(qs) {
            stage.lowpass(hz, *q, sample_rate);
        }
    }

    /// A highpass of `sections` × 2 poles at `hz`.
    pub fn highpass(&mut self, sections: usize, hz: f32, sample_rate: f32) {
        let qs = Self::qs(sections);
        self.count = qs.len();
        for (stage, q) in self.stages.iter_mut().zip(qs) {
            stage.highpass(hz, *q, sample_rate);
        }
    }

    /// One sample through every section.
    pub fn process(&mut self, input: f32) -> f32 {
        let count = self.count;
        self.stages
            .iter_mut()
            .take(count)
            .fold(input, |x, stage| stage.process(x))
    }

    /// Forget the past.
    pub fn reset(&mut self) {
        for stage in &mut self.stages {
            stage.reset();
        }
    }

    /// Set every section to where a steady `input` would have left it.
    pub fn settle(&mut self, input: f32) {
        let count = self.count;
        self.stages
            .iter_mut()
            .take(count)
            .fold(input, |x, stage| stage.settle(x));
    }
}

/// Non-zero odd taps on each side of a half-band filter's centre.
const HALF_TAPS: usize = 16;
/// History kept by each half of the oversampler (a power of two).
const HISTORY: usize = 64;

/// The zeroth-order modified Bessel function, for the Kaiser window.
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..64u32 {
        term *= half / f64::from(k);
        let add = term * term;
        sum += add;
        if add < sum * 1e-16 {
            break;
        }
    }
    sum
}

/// Doubles the rate around a nonlinearity and brings it back down, through
/// a 63-tap Kaiser-windowed half-band filter each way (about 80 dB of image
/// and alias rejection, flat to 0.42 of the base rate).
#[derive(Debug, Clone, Copy)]
pub struct Oversampler2 {
    taps: [f32; HALF_TAPS],
    up: [f32; HISTORY],
    up_at: usize,
    down: [f32; HISTORY],
    down_at: usize,
}

impl Default for Oversampler2 {
    fn default() -> Self {
        Self::new()
    }
}

impl Oversampler2 {
    /// Base-rate samples of delay the round trip adds.
    pub const LATENCY: f32 = 30.5;

    /// A cleared oversampler with its filter designed.
    #[must_use]
    pub fn new() -> Self {
        let reach = (2 * HALF_TAPS - 1) as f64;
        let beta = 7.86;
        let norm = bessel_i0(beta);
        let mut taps = [0.0f64; HALF_TAPS];
        for (j, tap) in taps.iter_mut().enumerate() {
            let n = (2 * j + 1) as f64;
            let sinc = (std::f64::consts::FRAC_PI_2 * n).sin() / (std::f64::consts::PI * n);
            let ratio = n / reach;
            let window = bessel_i0(beta * ratio.mul_add(-ratio, 1.0).max(0.0).sqrt()) / norm;
            *tap = sinc * window;
        }
        // A half-band filter's odd taps sum to a quarter on each side, so
        // the whole filter has unity gain at DC.
        let total: f64 = taps.iter().sum();
        let mut out = [0.0f32; HALF_TAPS];
        for (dst, tap) in out.iter_mut().zip(taps) {
            *dst = (tap * 0.25 / total) as f32;
        }
        Self {
            taps: out,
            up: [0.0; HISTORY],
            up_at: 0,
            down: [0.0; HISTORY],
            down_at: 0,
        }
    }

    /// One base-rate sample through `shape` run at twice the rate.
    pub fn process(&mut self, input: f32, mut shape: impl FnMut(f32) -> f32) -> f32 {
        let mask = HISTORY - 1;
        self.up_at = (self.up_at + 1) & mask;
        self.up[self.up_at] = finite(input);
        let past = |k: usize| self.up[self.up_at.wrapping_sub(k) & mask];
        let mut even = 0.0f32;
        for (j, tap) in self.taps.iter().enumerate() {
            even = tap.mul_add(past(15 - j) + past(16 + j), even);
        }
        let first = shape(2.0 * even);
        let second = shape(past(15));
        self.push_down(first);
        self.push_down(second);
        let back = |k: usize| self.down[self.down_at.wrapping_sub(k) & mask];
        let mut out = 0.5 * back(31);
        for (j, tap) in self.taps.iter().enumerate() {
            out = tap.mul_add(back(32 + 2 * j) + back(30 - 2 * j), out);
        }
        out
    }

    const fn push_down(&mut self, sample: f32) {
        self.down_at = (self.down_at + 1) & (HISTORY - 1);
        self.down[self.down_at] = finite(sample);
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.up = [0.0; HISTORY];
        self.down = [0.0; HISTORY];
        self.up_at = 0;
        self.down_at = 0;
    }
}

/// Two half-band stages: four times the rate around a nonlinearity.
#[derive(Debug, Clone, Copy, Default)]
pub struct Oversampler4 {
    outer: Oversampler2,
    inner: Oversampler2,
}

impl Oversampler4 {
    /// Base-rate samples of delay the round trip adds.
    pub const LATENCY: f32 = Oversampler2::LATENCY * 1.5;

    /// One base-rate sample through `shape` run at four times the rate.
    pub fn process(&mut self, input: f32, mut shape: impl FnMut(f32) -> f32) -> f32 {
        let Self { outer, inner } = self;
        outer.process(input, |x| inner.process(x, &mut shape))
    }

    /// Forget the past.
    pub const fn reset(&mut self) {
        self.outer.reset();
        self.inner.reset();
    }
}

/// A value that wanders between -1 and 1: every so often it picks a new
/// random target and glides there.
#[derive(Debug, Clone, Copy, Default)]
pub struct Drift {
    value: f32,
    target: f32,
    coeff: f32,
    left: u32,
}

impl Drift {
    /// Settled at zero, choosing its first target on the first sample.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            value: 0.0,
            target: 0.0,
            coeff: 0.0,
            left: 0,
        }
    }

    /// One sample, picking a new target from `noise` about every `seconds`.
    pub fn next(&mut self, noise: &mut Noise, seconds: f32, sample_rate: f32) -> f32 {
        if self.left == 0 {
            self.target = noise.sample();
            let period = (seconds * sample_rate * unit(noise).mul_add(1.0, 0.5)).max(1.0);
            self.left = period as u32;
            self.coeff = 1.0 - (-3.0 / period).exp();
        }
        self.left -= 1;
        self.value = (self.target - self.value).mul_add(self.coeff, self.value);
        self.value
    }
}

/// One rotating part's share of a transport's speed error.
#[derive(Debug, Clone, Copy)]
pub struct Partial {
    /// How often it comes round at nominal speed, Hz.
    pub hz: f32,
    /// Its share of the whole depth; a table's weights sum to 1.
    pub weight: f32,
    /// How far its rate wanders either side, as a fraction of `hz`.
    pub wander: f32,
    /// Its lowest amplitude as a fraction of its highest: 1 for a steady
    /// eccentric part, lower for a part that slips and catches.
    pub steady: f32,
}

/// The most parts a [`Wobble`] follows.
const MAX_PARTIALS: usize = 6;

#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    phase: f32,
    rate: Drift,
    size: Drift,
}

/// A transport's speed error, built from the parts that cause it.
///
/// Every capstan, pinch roller, reel and platter that is not perfectly round
/// or perfectly smooth turns its imperfection into a periodic speed error at
/// its own rotation rate. Each part here is a sinusoid at that rate whose
/// rate and size wander a little, so the sum has the lumpy narrow-band
/// spectrum a flutter meter shows rather than a clean LFO. Weighted as in
/// published wow-and-flutter analyses: slow reel and platter components for
/// wow, capstan and roller components from about 4 to 20 Hz for flutter.
///
/// The output is the delay, in seconds, a playback head sees: a part
/// contributing speed error `d · cos(φ)` shifts time by
/// `d · sin(φ) / (2π f)`, so `depth` is the peak fractional speed error and
/// a steady tone read through that delay deviates in frequency by at most
/// `depth`.
#[derive(Debug, Clone, Copy)]
pub struct Wobble {
    table: &'static [Partial],
    voices: [Voice; MAX_PARTIALS],
    noise: Noise,
    seed: u32,
    from: f32,
    to: f32,
    left: usize,
    primed: bool,
}

impl Wobble {
    /// A transport made of the parts in `table` (at most six), with its own
    /// random `seed`.
    #[must_use]
    pub fn new(table: &'static [Partial], seed: u32) -> Self {
        let table = &table[..table.len().min(MAX_PARTIALS)];
        let mut wobble = Self {
            table,
            voices: [Voice::default(); MAX_PARTIALS],
            noise: Noise::new(seed),
            seed,
            from: 0.0,
            to: 0.0,
            left: 0,
            primed: false,
        };
        wobble.reset();
        wobble
    }

    /// Restart every part from its seeded position.
    pub fn reset(&mut self) {
        self.noise = Noise::new(self.seed);
        for voice in &mut self.voices {
            *voice = Voice {
                phase: unit(&mut self.noise),
                ..Voice::default()
            };
        }
        self.from = 0.0;
        self.to = 0.0;
        self.left = 0;
        self.primed = false;
    }

    /// The farthest the delay can swing either side of centre at `depth`
    /// when the parts turn at `rate` times their nominal speed, seconds.
    #[must_use]
    pub fn reach(&self, depth: f32, rate: f32) -> f32 {
        let rate = rate.max(1e-3);
        self.table
            .iter()
            .map(|part| depth.abs() * part.weight / (TAU * part.hz * (1.0 - part.wander) * rate))
            .sum()
    }

    /// One sample of delay offset, seconds. `rate` scales every part's
    /// rotation (a faster tape speed turns the capstan faster); `advance` is
    /// how far through the medium this sample moves, as a fraction of normal
    /// (a slowing platter), which slows the parts without changing how far
    /// they displace the programme.
    ///
    /// The parts are worked out every [`CONTROL`] samples and the delay is
    /// interpolated in between: the fastest part turns at a few tens of
    /// hertz, far below that rate, so the curve is unchanged and the cost
    /// is a thirty-second of it.
    pub fn next(&mut self, depth: f32, rate: f32, advance: f32, sample_rate: f32) -> f32 {
        if !self.primed {
            self.primed = true;
            self.to = self.point(depth, rate, 0.0, sample_rate);
        }
        if self.left == 0 {
            self.from = self.to;
            self.to = self.point(depth, rate, advance * CONTROL as f32, sample_rate);
            self.left = CONTROL;
        }
        self.left -= 1;
        let t = 1.0 - self.left as f32 / CONTROL as f32;
        lerp(self.from, self.to, t)
    }

    /// Turn every part `samples` further on and sum their delays, seconds.
    fn point(&mut self, depth: f32, rate: f32, samples: f32, sample_rate: f32) -> f32 {
        let rate = rate.max(1e-3);
        let control_rate = sample_rate / CONTROL as f32;
        let mut delay = 0.0;
        for (voice, part) in self.voices.iter_mut().zip(self.table) {
            let wander = voice.rate.next(&mut self.noise, 1.7, control_rate);
            let hz = part.hz * rate * part.wander.mul_add(wander, 1.0);
            let size = voice.size.next(&mut self.noise, 0.8, control_rate);
            let amp = (1.0 - part.steady).mul_add(size.mul_add(0.5, 0.5), part.steady);
            let step = hz * samples / sample_rate;
            if step.is_finite() {
                voice.phase = (voice.phase + step).rem_euclid(1.0);
            }
            delay += depth * part.weight * amp * (TAU * voice.phase).sin() / (TAU * hz);
        }
        finite(delay)
    }
}

/// Moments where the tape lifts off the head: a missing oxide patch, a
/// crease, dust. Each one is a dip in level that takes the treble with it
/// (spacing loss grows with frequency), arriving at random.
#[derive(Debug, Clone, Copy)]
pub struct Dropouts {
    noise: Noise,
    seed: u32,
    left: u32,
    depth: f32,
    level: f32,
    edge: f32,
    edge_rate: f32,
}

impl Dropouts {
    /// Seeded dropouts.
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self {
            noise: Noise::new(seed),
            seed,
            left: 0,
            depth: 0.0,
            level: 0.0,
            edge: 1.0,
            edge_rate: 0.0,
        }
    }

    /// Clear any dropout under way and restart the random sequence.
    pub const fn reset(&mut self) {
        *self = Self::new(self.seed);
    }

    /// One sample of loss, 0 (none) up to `depth`: dropouts arrive at
    /// `per_second` on average, each lasting about `seconds`, with a 3 ms
    /// edge so they never click.
    pub fn next(&mut self, per_second: f32, depth: f32, seconds: f32, sample_rate: f32) -> f32 {
        if self.left > 0 {
            self.left -= 1;
        } else {
            self.depth = 0.0;
            if chance(&mut self.noise, per_second / sample_rate) {
                let length = seconds * sample_rate * unit(&mut self.noise).mul_add(1.4, 0.3);
                self.left = length.clamp(1.0, 10.0 * sample_rate) as u32;
                self.depth = depth.clamp(0.0, 1.0) * unit(&mut self.noise).mul_add(0.6, 0.4);
            }
        }
        if self.edge_rate.to_bits() != sample_rate.to_bits() {
            self.edge_rate = sample_rate;
            self.edge = 1.0 - (-1.0 / (0.003 * sample_rate).max(1.0)).exp();
        }
        self.level = (self.depth - self.level).mul_add(self.edge, self.level);
        flush(&mut self.level);
        self.level
    }
}

/// Noise shaped into a band: white noise through a first-order highpass and
/// two first-order lowpasses, scaled so its RMS is about 1 whatever the
/// band.
#[derive(Debug, Clone, Copy)]
pub struct Hiss {
    noise: Noise,
    seed: u32,
    high: OnePole,
    low: OnePole,
    lower: OnePole,
    scale: f32,
}

impl Hiss {
    /// Seeded hiss, unshaped until [`Self::band`] is called.
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self {
            noise: Noise::new(seed),
            seed,
            high: OnePole::default(),
            low: OnePole::default(),
            lower: OnePole::default(),
            scale: 1.7,
        }
    }

    /// Shape the hiss between `low_hz` and `high_hz`.
    pub fn band(&mut self, low_hz: f32, high_hz: f32, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        let top = high_hz.min(rate * 0.49);
        self.high.set_cutoff(low_hz, rate);
        self.low.set_cutoff(top, rate);
        self.lower.set_cutoff(top, rate);
        // Two first-order poles pass noise like a brick wall at about 0.8
        // of their corner.
        let width = 0.8f32.mul_add(top, -low_hz.max(1.0)).max(rate * 0.005);
        // White noise over ±1 has an RMS of 1/√3 spread over half the rate.
        self.scale = (3.0 * rate * 0.5 / width).sqrt().min(40.0);
    }

    /// The next sample.
    pub fn next(&mut self) -> f32 {
        let white = self.noise.sample();
        let banded = self
            .lower
            .lowpass(self.low.lowpass(self.high.highpass(white)));
        banded * self.scale
    }

    /// Restart the random sequence and clear the filters.
    pub const fn reset(&mut self) {
        self.noise = Noise::new(self.seed);
        self.high.reset();
        self.low.reset();
        self.lower.reset();
    }
}

/// Chowdhury's Jiles–Atherton constants for audio tape, divided through by
/// the saturation magnetisation so the magnetisation runs from -1 to 1:
/// the anhysteretic shape `a`, the loop width `k` (coercivity), the
/// reversible share `c` and the domain coupling `alpha`.
const JA_A: f64 = 2.2e4 / 3.5e5;
const JA_K: f64 = 2.7e4 / 3.5e5;
const JA_C: f64 = 1.7e-1;
const JA_ALPHA: f64 = 1.6e-3;

/// The Langevin function `coth q − 1/q` and its slope.
fn langevin(q: f64) -> (f64, f64) {
    if q.abs() < 1e-2 {
        let q2 = q * q;
        (q * (1.0 / 3.0 - q2 / 45.0), 1.0 / 3.0 - q2 / 15.0)
    } else {
        let t = q.tanh();
        let coth = 1.0 / t;
        // 1/sinh² = coth² − 1.
        (coth - 1.0 / q, 1.0 / (q * q) - coth.mul_add(coth, -1.0))
    }
}

/// Where [`langevin_single`] changes from its series to the closed form.
const SERIES_JOIN: f32 = 0.5;

/// The Langevin function in single precision. Near zero,
/// `coth q − 1/q` cancels badly in single precision, so below
/// [`SERIES_JOIN`] it is the series `q/3 − q³/45 + 2q⁵/945 − q⁷/4725`,
/// whose first missing term is under 3e-9 there.
fn langevin_single(q: f32) -> f32 {
    if q.abs() < SERIES_JOIN {
        let q2 = q * q;
        let inner = q2.mul_add(2.0 / 945.0 - q2 / 4_725.0, -1.0 / 45.0);
        q * q2.mul_add(inner, 1.0 / 3.0)
    } else {
        1.0 / q.tanh() - 1.0 / q
    }
}

/// Recording onto magnetic tape.
///
/// The magnetisation follows the Jiles–Atherton model (Jiles and Atherton,
/// "Theory of ferromagnetic hysteresis", 1986), in the form Jatin
/// Chowdhury used for real-time tape (Digital Audio Effects conference,
/// 2019): the anhysteretic
/// (Langevin) curve the domains relax toward, an irreversible part that
/// only moves when the field changes direction, and a reversible share.
/// It is solved with the explicit midpoint rule in field steps, split
/// finer when the field jumps, which keeps it stable at any drive.
///
/// Ideal AC bias linearises recording onto the anhysteretic curve, so the
/// output blends that curve (what a well-biased machine records) with the
/// raw hysteresis loop (what bias leaves behind when it is too low): the
/// loop brings the level-dependent lag, low-level grit and odd harmonics of
/// an under-biased machine. Both are scaled for unity gain on small
/// signals.
#[derive(Debug, Clone, Copy, Default)]
pub struct Magnetic {
    m: f64,
    h: f64,
}

impl Magnetic {
    /// One sample recorded with `drive` (the field per unit of signal, 0.05
    /// up to 64; 1.5 saturates full scale gently) and `loop_share` of the
    /// hysteresis loop (0 for ideal bias, up to 1 for none).
    pub fn process(&mut self, input: f32, drive: f32, loop_share: f32) -> f32 {
        let drive = drive.clamp(0.05, 64.0);
        let input = finite(input).clamp(-64.0, 64.0);
        let curve = Self::curve(input, drive);
        self.step(f64::from(drive * input) * JA_A);
        let looped = (3.0 * self.m) as f32 / drive;
        (looped - curve).mul_add(loop_share.clamp(0.0, 1.0), curve)
    }

    /// The anhysteretic curve alone, `3 L(k x) / k`: what ideally biased
    /// tape records, with no memory. Unity gain for small signals; `drive`
    /// as for [`Self::process`].
    #[must_use]
    pub fn curve(input: f32, drive: f32) -> f32 {
        let k = drive.clamp(0.05, 64.0);
        let q = k * finite(input).clamp(-64.0, 64.0);
        3.0 * langevin_single(q) / k
    }

    /// The slope of magnetisation against field at `m`, `h`, for a field
    /// moving in direction `delta`.
    fn slope(m: f64, h: f64, delta: f64) -> f64 {
        let (anhysteretic, rise) = langevin((JA_ALPHA.mul_add(m, h)) / JA_A);
        let diff = anhysteretic - m;
        let irreversible = if delta * diff > 0.0 {
            let den = ((1.0 - JA_C) * delta).mul_add(JA_K, -JA_ALPHA * diff);
            if den.abs() > 1e-9 {
                (1.0 - JA_C) * diff / den
            } else {
                0.0
            }
        } else {
            0.0
        };
        let reversible = JA_C * rise / JA_A;
        let den = 1.0 - JA_C * JA_ALPHA * rise / JA_A;
        (irreversible + reversible) / den
    }

    /// Move the field to `h`, integrating the magnetisation along the way.
    fn step(&mut self, h: f64) {
        let change = h - self.h;
        if change.abs() < 1e-12 {
            self.h = h;
            return;
        }
        let delta = change.signum();
        let pieces = (change.abs() / (JA_K * 0.5)).ceil().clamp(1.0, 8.0);
        let dh = change / pieces;
        let mut at = self.h;
        let mut m = self.m;
        for _ in 0..pieces as u32 {
            let first = Self::slope(m, at, delta) * dh;
            let second = Self::slope(0.5f64.mul_add(first, m), 0.5f64.mul_add(dh, at), delta) * dh;
            m = (m + second).clamp(-1.0, 1.0);
            at += dh;
        }
        self.m = if m.is_finite() { m } else { 0.0 };
        self.h = h;
    }

    /// Demagnetise.
    pub const fn reset(&mut self) {
        self.m = 0.0;
        self.h = 0.0;
    }
}

/// `sin x / x`, magnitude, for the aperture losses of a head.
fn aperture(x: f32) -> f32 {
    if x < 1e-4 { 1.0 } else { (x.sin() / x).abs() }
}

/// The geometry of a playback head against its tape, metres and metres per
/// second.
#[derive(Debug, Clone, Copy)]
pub struct Head {
    /// Tape speed past the head.
    pub speed: f32,
    /// Width of the playback gap.
    pub gap: f32,
    /// Track width times the tangent of the azimuth error: how far one edge
    /// of the track is read ahead of the other.
    pub skew: f32,
    /// How far the tape rides off the head.
    pub spacing: f32,
}

/// Playback loss at a tape head, as linear gain at `hz`: the gap loss
/// (`sin x / x`, `x = π g / λ`), the azimuth loss of a tilted gap (the same
/// aperture law over the skew), and the spacing loss of the tape riding off
/// the head (54.6 dB per wavelength of spacing, Wallace's law).
#[must_use]
pub fn head_loss(hz: f32, head: Head) -> f32 {
    let wavelength = head.speed.max(1e-4) / hz.max(1.0);
    let gap = aperture(PI * head.gap.max(0.0) / wavelength);
    let azimuth = aperture(PI * head.skew.max(0.0) / wavelength);
    gap * azimuth * db_to_gain(-54.6 * head.spacing.max(0.0) / wavelength)
}

/// Where [`head_loss`] reaches -3 dB, Hz, held below `limit`: the corner of
/// the lowpass that stands in for it.
#[must_use]
pub fn head_corner(head: Head, limit: f32) -> f32 {
    let limit = if limit.is_finite() {
        limit.max(20.0)
    } else {
        20_000.0
    };
    // Below the first aperture null (x = π) the loss only grows with
    // frequency.
    let widest = head.gap.max(head.skew).max(1e-9);
    let null = head.speed.max(1e-4) / widest * 0.99;
    let top = limit.min(null);
    if head_loss(top, head) >= std::f32::consts::FRAC_1_SQRT_2 {
        return top;
    }
    let (mut low, mut high) = (10.0f32, top);
    for _ in 0..32 {
        let mid = (low * high).sqrt();
        if head_loss(mid, head) >= std::f32::consts::FRAC_1_SQRT_2 {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}

#[cfg(test)]
pub mod testkit {
    //! Shared checks every lofi effect must pass.

    use crate::{Context, Curve, Effect, EffectKind};

    /// The rate tests run at.
    pub const RATE: f32 = 48_000.0;
    /// The context tests run with.
    pub const CONTEXT: Context = Context { bpm: 120.0 };

    /// Build `kind` with `params` set, prepared at [`RATE`] so every knob
    /// is settled.
    #[must_use]
    pub fn built(kind: &EffectKind, params: &[(usize, f32)]) -> Box<dyn Effect> {
        let mut effect = (kind.build)();
        for &(index, value) in params {
            effect.set_param(index, value);
        }
        effect.prepare(RATE);
        effect
    }

    /// `effect` run over `left` and `right` in blocks of `block`.
    pub fn render_blocks(
        effect: &mut dyn Effect,
        left: &[f32],
        right: &[f32],
        block: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let len = left.len().min(right.len());
        let mut out_l = vec![0.0; len];
        let mut out_r = vec![0.0; len];
        let mut start = 0;
        while start < len {
            let end = (start + block.max(1)).min(len);
            effect.process(
                &CONTEXT,
                [&left[start..end], &right[start..end]],
                [&mut out_l[start..end], &mut out_r[start..end]],
            );
            start = end;
        }
        (out_l, out_r)
    }

    /// `effect` run over `left` and `right` in blocks of 256.
    pub fn render(effect: &mut dyn Effect, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
        render_blocks(effect, left, right, 256)
    }

    /// A sine of `hz` at `amp` for `seconds`.
    #[must_use]
    pub fn sine(hz: f32, amp: f32, seconds: f32) -> Vec<f32> {
        let len = (seconds * RATE) as usize;
        let step = f64::from(hz) / f64::from(RATE);
        (0..len)
            .map(|n| {
                let turns = (step * n as f64).fract();
                amp * (std::f64::consts::TAU * turns).sin() as f32
            })
            .collect()
    }

    /// Seeded white noise at `amp` for `seconds`.
    #[must_use]
    pub fn noise(amp: f32, seconds: f32, seed: u32) -> Vec<f32> {
        let mut source = crate::dsp::Noise::new(seed);
        (0..(seconds * RATE) as usize)
            .map(|_| amp * source.sample())
            .collect()
    }

    /// Silence for `seconds`.
    #[must_use]
    pub fn silence(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * RATE) as usize]
    }

    /// The largest magnitude.
    #[must_use]
    pub fn peak(signal: &[f32]) -> f32 {
        signal.iter().fold(0.0, |most, x| most.max(x.abs()))
    }

    /// Root mean square.
    #[must_use]
    pub fn rms(signal: &[f32]) -> f32 {
        if signal.is_empty() {
            return 0.0;
        }
        (signal.iter().map(|x| x * x).sum::<f32>() / signal.len() as f32).sqrt()
    }

    /// The largest fractional deviation from `hz` of a steady tone, measured
    /// period by period from its rising zero crossings after `skip` seconds.
    #[must_use]
    pub fn frequency_deviation(signal: &[f32], hz: f32, skip: f32) -> f32 {
        let start = (skip * RATE) as usize;
        let mut last: Option<f64> = None;
        let mut worst = 0.0f64;
        for (n, pair) in signal.windows(2).enumerate().skip(start) {
            if pair[0] <= 0.0 && pair[1] > 0.0 {
                let (a, b) = (f64::from(pair[0]), f64::from(pair[1]));
                let at = n as f64 + a / (a - b);
                if let Some(before) = last {
                    let measured = f64::from(RATE) / (at - before);
                    worst = worst.max((measured / f64::from(hz) - 1.0).abs());
                }
                last = Some(at);
            }
        }
        worst as f32
    }

    fn bits(signal: &[f32]) -> Vec<u32> {
        signal.iter().map(|x| x.to_bits()).collect()
    }

    /// Every check the effect contract asks of `kind`, with a musical test
    /// signal.
    pub fn contract(kind: &EffectKind) {
        extremes_stay_bounded(kind);
        poison_passes_and_leaves_no_trace(kind);
        bad_params_are_ignored(kind);
        blocks_of_any_shape_work(kind);
        reset_restarts_everything(kind);
        stepped_labels_match(kind);
    }

    fn programme() -> (Vec<f32>, Vec<f32>) {
        let tone = sine(220.0, 0.9, 0.5);
        let hash = noise(0.3, 0.5, 7);
        let left: Vec<f32> = tone.iter().zip(&hash).map(|(a, b)| a + b).collect();
        let right: Vec<f32> = tone.iter().zip(&hash).map(|(a, b)| a - b).collect();
        (left, right)
    }

    fn extremes_stay_bounded(kind: &EffectKind) {
        let (left, right) = programme();
        for (index, spec) in kind.params.iter().enumerate() {
            for value in [spec.min, spec.max] {
                let mut effect = built(kind, &[(index, value)]);
                let (out_l, out_r) = render(effect.as_mut(), &left, &right);
                for out in [&out_l, &out_r] {
                    assert!(
                        out.iter().all(|x| x.is_finite()),
                        "{} {}",
                        kind.id,
                        spec.name
                    );
                    assert!(peak(out) < 2.0, "{} {} = {value}", kind.id, spec.name);
                }
            }
        }
        // Every knob at its maximum at once.
        let all: Vec<(usize, f32)> = kind
            .params
            .iter()
            .enumerate()
            .map(|(i, spec)| (i, spec.max))
            .collect();
        let mut effect = built(kind, &all);
        let (out_l, out_r) = render(effect.as_mut(), &left, &right);
        assert!(peak(&out_l) < 2.0 && peak(&out_r) < 2.0, "{}", kind.id);
    }

    fn poison_passes_and_leaves_no_trace(kind: &EffectKind) {
        let mut effect = built(kind, &[]);
        let mut left = sine(330.0, 0.5, 0.5);
        for (n, sample) in left.iter_mut().enumerate() {
            match n % 97 {
                0 => *sample = f32::NAN,
                1 => *sample = f32::INFINITY,
                2 => *sample = f32::NEG_INFINITY,
                3 => *sample = f32::MAX,
                _ => {}
            }
        }
        let right = left.clone();
        let (out_l, out_r) = render(effect.as_mut(), &left, &right);
        assert!(
            out_l.iter().chain(&out_r).all(|x| x.is_finite()),
            "{}",
            kind.id
        );
        let clean = sine(330.0, 0.5, 1.5);
        let (after_l, after_r) = render(effect.as_mut(), &clean, &clean);
        assert!(
            after_l.iter().chain(&after_r).all(|x| x.is_finite()),
            "{}",
            kind.id
        );
        assert!(
            rms(&after_l[48_000..]) > 0.01,
            "{} went silent after poison",
            kind.id
        );
    }

    fn bad_params_are_ignored(kind: &EffectKind) {
        let (left, right) = programme();
        let mut plain = built(kind, &[]);
        let mut poked = built(kind, &[]);
        for index in 0..kind.params.len() {
            poked.set_param(index, f32::NAN);
            poked.set_param(index, f32::INFINITY);
            poked.set_param(index, f32::NEG_INFINITY);
        }
        poked.set_param(kind.params.len(), 0.5);
        poked.set_param(usize::MAX, 0.5);
        let a = render(plain.as_mut(), &left, &right);
        let b = render(poked.as_mut(), &left, &right);
        assert_eq!(bits(&a.0), bits(&b.0), "{}", kind.id);
        assert_eq!(bits(&a.1), bits(&b.1), "{}", kind.id);
    }

    fn blocks_of_any_shape_work(kind: &EffectKind) {
        let (left, right) = programme();
        let mut whole = built(kind, &[]);
        let mut single = built(kind, &[]);
        let a = render_blocks(whole.as_mut(), &left[..4_800], &right[..4_800], 4_800);
        let b = render_blocks(single.as_mut(), &left[..4_800], &right[..4_800], 1);
        assert_eq!(
            bits(&a.0),
            bits(&b.0),
            "{}: block size changed the sound",
            kind.id
        );
        assert_eq!(bits(&a.1), bits(&b.1), "{}", kind.id);

        let mut effect = built(kind, &[]);
        let mut empty_l: [f32; 0] = [];
        let mut empty_r: [f32; 0] = [];
        effect.process(&CONTEXT, [&[], &[]], [&mut empty_l, &mut empty_r]);
        let mut out_l = [1.0f32; 48];
        let mut out_r = [1.0f32; 32];
        effect.process(
            &CONTEXT,
            [&left[..64], &right[..40]],
            [&mut out_l, &mut out_r],
        );
        assert!(
            out_l[32..].iter().all(|x| x.abs() < f32::EPSILON),
            "{}",
            kind.id
        );

        // Unprepared, it still must not panic.
        let mut raw = (kind.build)();
        let mut o_l = [0.0f32; 64];
        let mut o_r = [0.0f32; 64];
        raw.process(&CONTEXT, [&left[..64], &right[..64]], [&mut o_l, &mut o_r]);
        assert!(o_l.iter().chain(&o_r).all(|x| x.is_finite()), "{}", kind.id);

        // Odd sample rates are held to something workable.
        for rate in [
            f32::NAN,
            0.0,
            -44_100.0,
            1.0,
            f32::INFINITY,
            8_000.0,
            192_000.0,
        ] {
            let mut odd = (kind.build)();
            odd.prepare(rate);
            let mut o_l = [0.0f32; 512];
            let mut o_r = [0.0f32; 512];
            odd.process(
                &CONTEXT,
                [&left[..512], &right[..512]],
                [&mut o_l, &mut o_r],
            );
            assert!(
                o_l.iter().chain(&o_r).all(|x| x.is_finite()),
                "{} at {rate}",
                kind.id
            );
        }
    }

    fn reset_restarts_everything(kind: &EffectKind) {
        let (left, right) = programme();
        let mut used = built(kind, &[]);
        render(used.as_mut(), &left, &right);
        used.reset();
        let mut fresh = built(kind, &[]);
        let a = render(used.as_mut(), &left, &right);
        let b = render(fresh.as_mut(), &left, &right);
        assert_eq!(
            bits(&a.0),
            bits(&b.0),
            "{}: reset left something behind",
            kind.id
        );
        assert_eq!(bits(&a.1), bits(&b.1), "{}", kind.id);
    }

    fn stepped_labels_match(kind: &EffectKind) {
        for spec in kind.params {
            assert!(
                spec.min <= spec.default && spec.default <= spec.max,
                "{}",
                spec.name
            );
            if let Curve::Stepped { labels } = spec.curve {
                let steps = (spec.max - spec.min).round() as usize + 1;
                assert_eq!(labels.len(), steps, "{}", spec.name);
                assert!(
                    (spec.max - spec.min).fract().abs() < f32::EPSILON,
                    "{}",
                    spec.name
                );
            }
            if spec.curve == Curve::Log {
                assert!(spec.min > 0.0, "{}", spec.name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oversampler_is_transparent_in_band() {
        let mut os = Oversampler2::new();
        let rate = 48_000.0;
        let tone = |n: f32| (TAU * 5_000.0 * n / rate).sin();
        let mut worst = 0.0f32;
        for n in 0..4_800 {
            let out = os.process(tone(n as f32), |y| y);
            if n > 1_000 {
                worst = worst.max((out - tone(n as f32 - Oversampler2::LATENCY)).abs());
            }
        }
        assert!(worst < 2e-3, "{worst}");
    }

    #[test]
    fn the_oversampler_rejects_images() {
        // A tone near the top of the band, doubled: the image above the old
        // Nyquist must not come back down as audible junk.
        let mut os = Oversampler2::new();
        let rate = 48_000.0;
        let out: Vec<f32> = (0..9_600)
            .map(|n| os.process((TAU * 15_000.0 * n as f32 / rate).sin(), |y| y))
            .collect();
        // Correlate with the 33 kHz image folded back (15 kHz is the only
        // thing that should be there): measure energy off 15 kHz by
        // removing the best-fit sine.
        let (mut s, mut c) = (0.0f32, 0.0f32);
        for (n, x) in out.iter().enumerate().skip(1_000) {
            let phase = TAU * 15_000.0 * n as f32 / rate;
            s = x.mul_add(phase.sin(), s);
            c = x.mul_add(phase.cos(), c);
        }
        let count = (out.len() - 1_000) as f32;
        let (s, c) = (2.0 * s / count, 2.0 * c / count);
        let mut residual = 0.0f32;
        for (n, x) in out.iter().enumerate().skip(1_000) {
            let phase = TAU * 15_000.0 * n as f32 / rate;
            let fit = s.mul_add(phase.sin(), c * phase.cos());
            residual = (x - fit).mul_add(x - fit, residual);
        }
        let residual = (residual / count).sqrt();
        assert!(residual < 1e-3, "{residual}");
    }

    #[test]
    fn a_settled_filter_passes_a_constant_without_a_transient() {
        let mut filter = Butterworth::default();
        filter.lowpass(2, 4_500.0, 48_000.0);
        filter.settle(0.7);
        for _ in 0..100 {
            assert!((filter.process(0.7) - 0.7).abs() < 1e-5);
        }
        let mut high = Biquad::new();
        high.highpass(100.0, 0.7, 48_000.0);
        assert!(high.settle(1.0).abs() < 1e-6);
        assert!(high.process(1.0).abs() < 1e-6);
    }

    #[test]
    fn shelves_of_opposite_gain_cancel() {
        let mut up = Biquad::new();
        let mut down = Biquad::new();
        up.high_shelf(3_000.0, 10.0, 48_000.0);
        down.high_shelf(3_000.0, -10.0, 48_000.0);
        let mut worst = 0.0f32;
        for n in 0..4_800 {
            let x = (TAU * 7_000.0 * n as f32 / 48_000.0).sin();
            let y = down.process(up.process(x));
            if n > 100 {
                worst = worst.max((y - x).abs());
            }
        }
        assert!(worst < 1e-4, "{worst}");
    }

    #[test]
    fn wobble_depth_is_the_peak_speed_error() {
        static ONE: [Partial; 1] = [Partial {
            hz: 2.0,
            weight: 1.0,
            wander: 0.0,
            steady: 1.0,
        }];
        let mut wobble = Wobble::new(&ONE, 3);
        let rate = 48_000.0;
        let mut last = wobble.next(0.01, 1.0, 1.0, rate);
        let mut fastest = 0.0f32;
        for _ in 0..48_000 {
            let now = wobble.next(0.01, 1.0, 1.0, rate);
            fastest = fastest.max(((now - last) * rate).abs());
            last = now;
        }
        assert!((fastest - 0.01).abs() < 2e-4, "{fastest}");
        assert!(wobble.reach(0.01, 1.0) >= 0.01 / (TAU * 2.0) * 0.999);
    }

    #[test]
    fn head_loss_falls_with_frequency_and_rises_with_speed() {
        let cassette = Head {
            speed: 0.047_6,
            gap: 1.0e-6,
            skew: 0.0,
            spacing: 0.3e-6,
        };
        let slow = head_corner(cassette, 40_000.0);
        let fast = head_corner(
            Head {
                speed: 0.381,
                gap: 3.0e-6,
                skew: 0.0,
                spacing: 0.5e-6,
            },
            400_000.0,
        );
        assert!(slow > 4_000.0 && slow < 20_000.0, "{slow}");
        assert!(fast > slow * 3.0, "{slow} {fast}");
        assert!(head_loss(1_000.0, cassette) > 0.9);
        let tilted = head_corner(
            Head {
                skew: 3.0e-6,
                ..cassette
            },
            40_000.0,
        );
        assert!(tilted < slow * 0.8, "{tilted}");
    }

    #[test]
    fn hiss_is_about_unit_rms() {
        let mut hiss = Hiss::new(11);
        hiss.band(300.0, 8_000.0, 48_000.0);
        let mut sum = 0.0f32;
        for _ in 0..48_000 {
            let x = hiss.next();
            sum = x.mul_add(x, sum);
        }
        let rms = (sum / 48_000.0).sqrt();
        assert!(rms > 0.6 && rms < 1.6, "{rms}");
    }

    #[test]
    fn biased_tape_is_unity_for_small_signals_and_compresses_large_ones() {
        let mut tape = Magnetic::default();
        let small: f32 = (0..4_800)
            .map(|n| {
                let x = 0.01 * (TAU * 100.0 * n as f32 / 48_000.0).sin();
                (tape.process(x, 1.5, 0.0) - x).abs()
            })
            .fold(0.0, f32::max);
        assert!(small < 1e-4, "{small}");
        let loud = tape.process(1.0, 1.5, 0.0);
        assert!(loud < 0.95 && loud > 0.8, "{loud}");
    }

    #[test]
    fn the_hysteresis_loop_lags_and_stays_bounded() {
        let mut tape = Magnetic::default();
        let mut rising = 0.0f32;
        let mut falling = 0.0f32;
        for n in 0..9_600 {
            let phase = TAU * 50.0 * n as f32 / 48_000.0;
            let x = phase.sin();
            let y = tape.process(x, 4.0, 1.0);
            assert!(y.is_finite() && y.abs() < 1.0, "{y}");
            // Where the field crosses zero, the loop is open: the output
            // still carries the sign of where it came from.
            if n > 4_800 && x.abs() < 0.01 {
                if phase.cos() > 0.0 {
                    rising = y;
                } else {
                    falling = y;
                }
            }
        }
        assert!(rising < -0.01 && falling > 0.01, "{rising} {falling}");
        for x in [1e9, -1e9, f32::NAN, 0.3] {
            assert!(tape.process(x, 64.0, 1.0).is_finite());
        }
    }

    #[test]
    fn the_curve_is_smooth_across_its_series_join() {
        // Both branches agree with the double-precision Langevin function
        // on either side of the join, so there is no step there.
        for q in [0.3f32, 0.45, 0.499_99, 0.5, 0.500_01, 0.55, 0.8, 3.0] {
            let exact = langevin(f64::from(q)).0 as f32;
            let single = langevin_single(q);
            assert!((single - exact).abs() < 2e-7, "{q}: {single} vs {exact}");
            assert!((langevin_single(-q) + single).abs() < 1e-9, "odd at {q}");
        }
        let small = Magnetic::curve(1e-3, 1.5);
        assert!((small - 1e-3).abs() < 1e-8, "{small}");
        let big = Magnetic::curve(64.0, 64.0);
        assert!(big.is_finite() && big < 3.0 / 64.0 + 1e-6);
    }

    #[test]
    fn the_ceiling_holds() {
        assert!(ceiling(1e30) < 2.0);
        assert!(ceiling(-1e30) > -2.0);
        assert!(ceiling(f32::NAN).abs() < f32::EPSILON);
        assert!((ceiling(0.5) - 0.5).abs() < f32::EPSILON);
    }
}
