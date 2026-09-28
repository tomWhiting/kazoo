//! Crush: a bit crusher and sample-rate reducer, modelled as a cheap
//! converter.
//!
//! The effect is an analogue-to-digital converter with too few bits and
//! too slow a clock, straight into a digital-to-analogue converter that
//! holds each sample until the next:
//!
//! - **rate** is the converter's clock. Every tick it takes a sample and
//!   holds it (sample and hold). Nothing stops a tone above half the clock
//!   folding back down as a new, unrelated pitch, and the held steps ring
//!   with images of everything above: that is the sound. The clock keeps
//!   its own time between the host's samples: each tick samples the input
//!   at the exact instant it falls, read between the two host samples
//!   around it, and the held staircase is the converter's analogue output,
//!   its steps placed exactly and taken out through the line stage every
//!   audio output has (a twentieth-order Butterworth lowpass at 20 kHz, or
//!   at 0.4 of the host's rate where that is lower, 17.6 kHz at 44.1 kHz;
//!   run as the analogue filter it is, see below). So the sound does not
//!   change with the host's rate, aliasing and all, through 15 kHz; at
//!   44.1 kHz only the top two kilohertz are rounded off, where the host
//!   cannot hold them anyway. It cannot run faster than the host, so at
//!   44.1 kHz the top of the knob (48 kHz) samples every host sample.
//! - **jitter** wobbles the clock: every period is stretched or shrunk at
//!   random by up to 90 %, smearing the aliases into grit. The wobble is
//!   even either way, so the clock keeps its average rate.
//! - **bits** is the converter's resolution, from 16 down to 1, and may sit
//!   between whole numbers: the step is `2^(1 - bits)` of full scale. The
//!   converter is mid-tread, so silence stays silent; that means "1 bit"
//!   gives three levels, -1, 0 and +1. It clips at full scale, as a real
//!   one does.
//! - **dither** adds triangular (TPDF) noise of up to one step before
//!   rounding, trading the quantiser's gritty, signal-following error for
//!   a steady hiss, as mastering engineers do on purpose.
//! - **aa** puts in the filters a proper converter has: an eighth-order
//!   Butterworth lowpass just below half the clock before the sampler, so
//!   nothing can fold, and the same after the hold, so no images escape.
//!   Both filters are run as the analogue filters they are, not as digital
//!   copies: split into four resonant poles, each is integrated exactly
//!   over every host sample, taking the input as a straight line between
//!   host samples before the converter and as the held steps after it,
//!   wherever between host samples each step falls. The converter is thus
//!   sampling and holding in continuous time, and the result is clean and
//!   the same at any host rate. Switched off, which is the default, the
//!   aliasing is the effect, and it is there because it was asked for.
//!   The switch crossfades.
//!
//! - **mix** blends the dry signal back in. The dry side goes through the
//!   same line stage as the converter, so the two share its phase at every
//!   frequency and a blend does not comb; all that tells them apart is the
//!   hold, which is the converter's sound. That is a choice: a pedal's dry
//!   path does not pass a reconstruction filter. It costs the dry side
//!   nothing to 15 kHz at any rate, and at 44.1 kHz rolls off its top two
//!   kilohertz (0.9 dB down at 17 kHz) with the converter's.
//! - **level** is the output level.
//!
//! The clock and its jitter are shared by both channels, so a stereo image
//! stays put. The line stage's delay is padded out to a whole number of
//! samples, which is the latency reported.
//!
//! Sources: the sampling theorem; J. Vanderkooy and S. P. Lipshitz,
//! "Dither in digital audio" (JAES, 1987), for TPDF dither; the exact
//! (impulse-invariant, input-interpolating) discretisation of a linear
//! system's modal form.

use std::f64::consts::{PI, TAU};

use super::kit::{
    DEFAULT_RATE, Knobs, Retune, ceiling, flush64, for_each_frame, fraction, gain as db_gain,
    weight,
};
use super::oversample::{Pad, whole_latency};
use crate::dsp::Noise;
use crate::dsp::sane_rate;
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const BITS: usize = 0;
const RATE: usize = 1;
const JITTER: usize = 2;
const DITHER: usize = 3;
const AA: usize = 4;
const MIX: usize = 5;
const LEVEL: usize = 6;

static PARAMS: [ParamSpec; 7] = [
    ParamSpec {
        name: "bits",
        min: 1.0,
        max: 16.0,
        default: 8.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "rate",
        min: 100.0,
        max: 48_000.0,
        default: 8_000.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "jitter",
        min: 0.0,
        max: 100.0,
        default: 0.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "dither",
        min: 0.0,
        max: 100.0,
        default: 0.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "aa",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["off", "on"],
        },
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 100.0,
        default: 100.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "level",
        min: -24.0,
        max: 12.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
];

/// The Crush's entry in the catalogue.
pub const KIND: EffectKind = EffectKind {
    id: "crush",
    name: "Bit crusher",
    description: "A cheap converter on purpose: fewer bits, a slow and jittery clock, optional dither, and anti-aliasing filters you can switch in.",
    params: &PARAMS,
    build: || Box::new(Crush::new()),
};

/// The anti-aliasing filters' corner, as a fraction of the clock.
const CORNER: f64 = 0.45;
/// The line stage's corner, and the most of the host rate it may reach.
const LINE_HZ: f64 = 20_000.0;
const LINE_SHARE: f64 = 0.4;
/// While the clock moves, the converter's filters are redesigned once in
/// this many samples. A design is four complex exponentials and a product
/// over every pole for each mode; every sample, a cabled clock made the
/// crusher cost about seven times as much as at rest.
const REDESIGN_EVERY: u32 = 4;
/// How far jitter can stretch or shrink one clock period.
const MAX_JITTER: f64 = 0.9;

/// A complex number: just what the modal filters need.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const ZERO: Self = Self { re: 0.0, im: 0.0 };

    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }

    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re.mul_add(other.re, -self.im * other.im),
            self.re.mul_add(other.im, self.im * other.re),
        )
    }

    fn scale(self, by: f64) -> Self {
        Self::new(self.re * by, self.im * by)
    }

    fn div(self, other: Self) -> Self {
        let size = other.re.mul_add(other.re, other.im * other.im);
        Self::new(
            self.re.mul_add(other.re, self.im * other.im) / size,
            self.im.mul_add(other.re, -self.re * other.im) / size,
        )
    }

    /// `e^z - 1`, accurate for small `z`.
    fn exp_m1(self) -> Self {
        let half = 0.5 * self.im;
        let sine_half = half.sin();
        Self::new(
            self.re
                .exp_m1()
                .mul_add(self.im.cos(), -2.0 * sine_half * sine_half),
            self.re.exp() * self.im.sin(),
        )
    }
}

/// `(e^z - 1) / z`, accurate for small `z`.
fn first_integral(z: Complex) -> Complex {
    if z.re.hypot(z.im) < 1e-4 {
        // 1 + z/2 + z²/6
        Complex::new(1.0, 0.0)
            .add(z.scale(0.5))
            .add(z.mul(z).scale(1.0 / 6.0))
    } else {
        z.exp_m1().div(z)
    }
}

/// `(e^z - 1 - z) / z²`, accurate for small `z`.
fn second_integral(z: Complex) -> Complex {
    if z.re.hypot(z.im) < 1e-3 {
        // 1/2 + z/6 + z²/24
        Complex::new(0.5, 0.0)
            .add(z.scale(1.0 / 6.0))
            .add(z.mul(z).scale(1.0 / 24.0))
    } else {
        z.exp_m1().sub(z).div(z.mul(z))
    }
}

/// How many resonant pole pairs the converter's own filters have: eighth
/// order.
const POLES: usize = 4;
/// How many the line stage has: twentieth order, steep enough that the
/// staircase's images just above half a 44.1 kHz host rate fold back into
/// the audio band more than 80 dB down (61 dB off an image at 25 kHz, a
/// 3 kHz clock's eighth, which the hold leaves 28 dB down).
const LINE_POLES: usize = 10;

/// An analogue Butterworth lowpass of order `2P` in modal form: `P`
/// complex poles (each standing for itself and its mirror image), each a
/// first-order system `s' = p s + u` whose response is `2 Re(r s)`.
/// Stepped exactly: over one host sample of length `T`, a pole's state
/// becomes `e^(pT) s` plus the exact integral of the input, which is taken
/// as a straight line between samples or as held steps.
#[derive(Debug, Clone, Copy)]
struct Modal<const P: usize> {
    /// The host sample period.
    period: f64,
    /// Each pole times the host sample period, `pT`.
    poles: [Complex; P],
    /// Each pole's residue.
    residues: [Complex; P],
    /// `e^(pT)`, and the integrals of a constant and a ramp over a sample.
    decay: [Complex; P],
    constant: [Complex; P],
    ramp: [Complex; P],
    state: [Complex; P],
    /// The filter's group delay at low frequencies, in host samples.
    delay: f64,
    /// The gain at DC of the impulse-invariant form ([`Self::impulse`]),
    /// which it is divided by so that it passes DC at unity as the
    /// analogue filter does.
    impulse_gain: f64,
}

impl<const P: usize> Default for Modal<P> {
    fn default() -> Self {
        Self {
            period: 0.0,
            poles: [Complex::ZERO; P],
            residues: [Complex::ZERO; P],
            decay: [Complex::ZERO; P],
            constant: [Complex::ZERO; P],
            ramp: [Complex::ZERO; P],
            state: [Complex::ZERO; P],
            delay: 0.0,
            impulse_gain: 1.0,
        }
    }
}

impl<const P: usize> Modal<P> {
    /// Set the corner to `hz` for host rate `rate`.
    fn design(&mut self, hz: f64, rate: f64) {
        let omega = TAU * hz.clamp(1.0, rate * 0.49);
        let period = 1.0 / rate;
        self.period = period;
        let orders = 2 * P;
        let pole = |k: usize| {
            let angle = PI / 2.0 + (2 * k + 1) as f64 * PI / (2 * orders) as f64;
            Complex::new(omega * angle.cos(), omega * angle.sin())
        };
        // At low frequencies each pole pair delays by -2 Re(p) / |p|².
        self.delay = (0..P)
            .map(|k| -2.0 * pole(k).re / (omega * omega))
            .sum::<f64>()
            * rate;
        for k in 0..P {
            let own = pole(k);
            // The residue of ω^(2P) / Π(s - p) at p: the product runs over
            // all 2P poles, the P here and their mirror images.
            let mut product = Complex::new(1.0, 0.0);
            for j in 0..P {
                let other = pole(j);
                if j != k {
                    product = product.mul(own.sub(other));
                }
                product = product.mul(own.sub(Complex::new(other.re, -other.im)));
            }
            let residue = Complex::new(omega.powi(i32::try_from(orders).unwrap_or(i32::MAX)), 0.0)
                .div(product);
            // Keep each mode's share of the output where it was, so moving
            // the corner never steps the output.
            let old = self.residues[k];
            if old.re.hypot(old.im) > 0.0 {
                self.state[k] = self.state[k].mul(old).div(residue);
            }
            self.residues[k] = residue;
            let z = own.scale(period);
            self.poles[k] = z;
            self.decay[k] = z.exp_m1().add(Complex::new(1.0, 0.0));
            self.constant[k] = first_integral(z).scale(period);
            self.ramp[k] = second_integral(z).scale(period);
        }
        // The impulse-invariant form sums the impulse response at the host
        // samples, T h(nT): per mode, T r / (1 - e^(pT)).
        let gain: f64 = (0..P)
            .map(|k| {
                let tail = Complex::new(1.0, 0.0).sub(self.decay[k]);
                2.0 * self.residues[k].scale(period).div(tail).re
            })
            .sum();
        self.impulse_gain = if gain.is_finite() && gain > 0.0 {
            gain
        } else {
            1.0
        };
    }

    /// Take `other`'s design, as [`Self::design`] would have made it,
    /// keeping this filter's own state and each mode's share of its output
    /// where it was, just as a design does. Designing is the costly part
    /// (a complex exponential and a product over every pole for each mode),
    /// so filters that share a corner design once and copy.
    fn take_design(&mut self, other: &Self) {
        for k in 0..P {
            let old = self.residues[k];
            let new = other.residues[k];
            if old.re.hypot(old.im) > 0.0 && new.re.hypot(new.im) > 0.0 {
                self.state[k] = self.state[k].mul(old).div(new);
            }
        }
        *self = Self {
            state: self.state,
            ..*other
        };
    }

    /// Zero any state that has decayed into the denormal range or gone
    /// non-finite.
    fn flush(&mut self) {
        for state in &mut self.state {
            flush64(&mut state.re);
            flush64(&mut state.im);
        }
    }

    fn output(&self, state: &[Complex; P]) -> f64 {
        state
            .iter()
            .zip(&self.residues)
            .map(|(s, r)| 2.0 * r.mul(*s).re)
            .sum()
    }

    /// One host sample with the input running in a straight line from
    /// `from` to `to`.
    fn ramp(&mut self, from: f64, to: f64) -> f64 {
        for k in 0..P {
            let held = self.constant[k].scale(from);
            let sloped = self.ramp[k].scale(to - from);
            self.state[k] = self.decay[k].mul(self.state[k]).add(held).add(sloped);
        }
        self.flush();
        self.output(&self.state)
    }

    /// What the output would be `after` of the way (0 to 1) into the
    /// coming sample, with the input running from `from` to `to` over the
    /// whole sample: the filter read between host samples.
    fn peek(&self, from: f64, to: f64, after: f64) -> f64 {
        let span = self.period * after;
        let mut state = [Complex::ZERO; P];
        for (k, into) in state.iter_mut().enumerate() {
            let z = self.poles[k].scale(after);
            let decay = z.exp_m1().add(Complex::new(1.0, 0.0));
            let held = first_integral(z).scale(span * from);
            // The input has come `after` of the way from `from` to `to`.
            let sloped = second_integral(z).scale(span * (to - from) * after);
            *into = decay.mul(self.state[k]).add(held).add(sloped);
        }
        self.output(&state)
    }

    /// One host sample of a sampled signal through the filter's
    /// impulse-invariant form: the analogue filter's own impulse response
    /// taken at the host samples, so its phase in the audio band is the
    /// analogue filter's exactly, with none of the droop that feeding it
    /// held steps or straight lines would add.
    fn impulse(&mut self, x: f64) -> f64 {
        for k in 0..P {
            self.state[k] = self.decay[k]
                .mul(self.state[k])
                .add(Complex::new(x * self.period, 0.0));
        }
        self.flush();
        self.output(&self.state) / self.impulse_gain
    }

    /// One host sample with the input held at `old` until `ago` samples
    /// before the sample's end, then at `new`.
    fn step(&mut self, old: f64, new: f64, ago: f64) -> f64 {
        let late = self.period * ago;
        for k in 0..P {
            let tail = first_integral(self.poles[k].scale(ago)).scale(late);
            let before = self.constant[k].sub(tail).scale(old);
            let after = tail.scale(new);
            self.state[k] = self.decay[k].mul(self.state[k]).add(before).add(after);
        }
        self.flush();
        self.output(&self.state)
    }

    const fn reset(&mut self) {
        self.state = [Complex::ZERO; P];
    }
}

/// Four-point cubic (Hermite) interpolation between `x1` (at 0) and `x2`
/// (at 1), with `x0` before and `x3` after.
fn hermite64(x0: f64, x1: f64, x2: f64, x3: f64, t: f64) -> f64 {
    let c1 = 0.5 * (x2 - x0);
    // x0 - 2.5 x1 + 2 x2 - 0.5 x3
    let c2 = 0.5f64.mul_add(-x3, 2.0f64.mul_add(x2, 2.5f64.mul_add(-x1, x0)));
    let c3 = 0.5f64.mul_add(x3 - x0, 1.5 * (x1 - x2));
    c3.mul_add(t, c2).mul_add(t, c1).mul_add(t, x1)
}

/// `sample` through a converter with steps of `step`, clipped at full
/// scale, with `dither` (in steps) added before rounding.
fn quantise(sample: f64, step: f64, dither: f64) -> f64 {
    let steps = (sample.clamp(-1.0, 1.0) / step + dither).round();
    (steps * step).clamp(-1.0, 1.0)
}

/// The converter's clock, which keeps its own time between host samples.
#[derive(Debug, Clone, Copy)]
struct Clock {
    /// How far through the current period, in periods.
    phase: f64,
    /// How long this period is, in periods of the set clock: 1 without
    /// jitter.
    period: f64,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            phase: 0.0,
            period: 1.0,
        }
    }
}

impl Clock {
    /// Move on one host sample, `increment` periods of the set clock (at
    /// most 1). If the clock ticked, returns how long before this host
    /// sample it did, in host samples (0 up to 1). `wobble` (-1 to 1)
    /// times `jitter` (0 to 0.9) sets the next period's length.
    fn advance(
        &mut self,
        increment: f64,
        jitter: f64,
        wobble: impl FnOnce() -> f64,
    ) -> Option<f64> {
        let increment = increment.clamp(1e-9, 1.0);
        self.phase += increment;
        if self.phase < self.period {
            return None;
        }
        let over = self.phase - self.period;
        self.period = jitter.mul_add(wobble(), 1.0);
        // One tick per host sample at most: a period shorter than a host
        // sample waits for the next.
        self.phase = over.min(self.period * 0.999_999);
        Some((over / increment).min(1.0))
    }
}

#[derive(Debug, Clone, Copy)]
struct Channel {
    /// What the raw converter holds.
    raw: f64,
    /// What the filtered converter holds.
    clean: f64,
    /// The last three host samples of input, oldest first.
    history: [f64; 3],
    /// The filtered converter's output a sample ago.
    last_restored: f64,
    before: Modal<POLES>,
    after: Modal<POLES>,
    /// The raw converter's line output: its staircase through the line
    /// stage, so what reaches the host is the same at any rate.
    line: Modal<LINE_POLES>,
    /// The filtered converter's output through the same line stage, so
    /// both converters come out equally late and the `aa` switch can
    /// crossfade between them.
    clean_line: Modal<LINE_POLES>,
    /// The dry side of `mix` through the same line stage, a sample behind
    /// as the converter runs, so the two sides share the line's phase at
    /// every frequency and a blend does not comb (all that differs is the
    /// hold, which is the converter's sound).
    dry_line: Modal<LINE_POLES>,
    /// What is left of the line stage's delay to the next whole sample, so
    /// the latency declared is the latency heard.
    pad: Pad,
}

impl Default for Channel {
    fn default() -> Self {
        Self {
            raw: 0.0,
            clean: 0.0,
            history: [0.0; 3],
            last_restored: 0.0,
            before: Modal::default(),
            after: Modal::default(),
            line: Modal::default(),
            clean_line: Modal::default(),
            dry_line: Modal::default(),
            pad: Pad::default(),
        }
    }
}

/// A bit crusher. See the module documentation for the converter.
#[derive(Debug)]
pub struct Crush {
    knobs: Knobs<7>,
    rate: f64,
    clock: Clock,
    clock_noise: Noise,
    dither_noise: Noise,
    /// The clock the filters were last designed for.
    retune: Retune<1>,
    /// Samples until the converter's filters may be redesigned again while
    /// the clock moves (see [`REDESIGN_EVERY`]).
    redesign_in: u32,
    channels: [Channel; 2],
    /// The line stage's delay in whole host samples.
    latency: usize,
}

impl Default for Crush {
    fn default() -> Self {
        Self::new()
    }
}

impl Crush {
    /// A crusher at its default settings, ready for 48 kHz until prepared.
    #[must_use]
    pub fn new() -> Self {
        let mut crush = Self {
            knobs: Knobs::new(&PARAMS),
            rate: f64::from(DEFAULT_RATE),
            clock: Clock::default(),
            clock_noise: Noise::new(0x51A7_0C1C),
            dither_noise: Noise::new(0xD17E_4A11),
            retune: Retune::new([PARAMS[RATE].default]),
            redesign_in: 0,
            channels: [Channel::default(); 2],
            latency: 0,
        };
        crush.prepare(DEFAULT_RATE);
        crush
    }

    fn design(&mut self, [clock]: [f32; 1]) {
        let corner = CORNER * f64::from(clock);
        // All four filters share the corner: design one, copy to the rest.
        let [first, second] = &mut self.channels;
        first.before.design(corner, self.rate);
        let design = first.before;
        first.after.take_design(&design);
        second.before.take_design(&design);
        second.after.take_design(&design);
    }

    /// The line stage's corner at the host's rate: 20 kHz, or 0.4 of the
    /// rate if that is lower, and the delay, the line's and the sample the
    /// converter runs behind, padded out to the whole samples declared.
    fn design_line(&mut self) {
        let corner = LINE_HZ.min(LINE_SHARE * self.rate);
        for channel in &mut self.channels {
            channel.line.design(corner, self.rate);
            channel.clean_line.design(corner, self.rate);
            channel.dry_line.design(corner, self.rate);
        }
        let (latency, pad) = whole_latency(self.channels[0].line.delay + 1.0, 1);
        self.latency = latency;
        for channel in &mut self.channels {
            channel.pad = pad;
        }
    }

    /// Design for where the knobs are now, with nothing left waiting:
    /// after a prepare or a reset.
    fn retune_now(&mut self) {
        let knob = self.knobs.values();
        let now = [knob[RATE]];
        self.retune.settle(now);
        self.design(now);
        self.redesign_in = 0;
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let knob = self.knobs.step();
        let (bits, clock, jitter) = (knob[BITS], knob[RATE], knob[JITTER]);
        let (dither, aa, mix, level) = (knob[DITHER], knob[AA], knob[MIX], knob[LEVEL]);
        // A moving clock redesigns every few samples, not every one: each
        // design keeps every mode's share of the output where it was, so
        // the steps are seamless, and a clock that stops is designed for
        // exactly within a few samples.
        self.redesign_in = self.redesign_in.saturating_sub(1);
        if self.redesign_in == 0 && self.retune.due([clock]) {
            self.design([clock]);
            self.redesign_in = REDESIGN_EVERY;
        }
        let noise = &mut self.clock_noise;
        let tick = self.clock.advance(
            f64::from(clock) / self.rate,
            MAX_JITTER * fraction(jitter),
            || f64::from(noise.sample()),
        );
        let step = (1.0 - f64::from(bits)).exp2();
        let dither = fraction(dither) * 0.5;
        let filtered = f64::from(weight(aa, 1.0));
        let mix = fraction(mix);
        let out_gain = db_gain(level);
        let noise = &mut self.dither_noise;
        let [first, second] = &mut self.channels;
        [(first, left), (second, right)].map(|(channel, sample)| {
            let dry = f64::from(sample);
            // The converter runs one host sample behind, so the instant it
            // samples always has two host samples either side of it to read
            // between with a cubic: a straight line would leave an error
            // that moves with where the ticks fall on the host's grid.
            let [x0, x1, x2] = channel.history;
            channel.history = [x1, x2, dry];
            let (last, now) = (x1, x2);
            let (restored_in, raw) = if let Some(ago) = tick {
                let mut tpdf = || dither * f64::from(noise.sample() + noise.sample());
                let at = hermite64(x0, x1, x2, dry, 1.0 - ago);
                let raw = quantise(at, step, tpdf());
                let old_raw = channel.raw;
                channel.raw = raw;
                let captured = channel.before.peek(last, now, 1.0 - ago);
                channel.before.ramp(last, now);
                let held = quantise(captured, step, tpdf());
                let old = channel.clean;
                channel.clean = held;
                (
                    channel.after.step(old, held, ago),
                    channel.line.step(old_raw, raw, ago),
                )
            } else {
                channel.before.ramp(last, now);
                (
                    channel.after.ramp(channel.clean, channel.clean),
                    channel.line.ramp(channel.raw, channel.raw),
                )
            };
            // The filtered converter's output is smooth, so it goes through
            // its line stage as a straight line between host samples.
            let restored = channel.clean_line.ramp(channel.last_restored, restored_in);
            channel.last_restored = restored_in;
            let wet = filtered.mul_add(restored - raw, raw);
            let dry = channel.dry_line.impulse(x2);
            let out = (mix.mul_add(wet - dry, dry) * out_gain) as f32;
            ceiling(channel.pad.push(out))
        })
    }
}

impl Effect for Crush {
    fn prepare(&mut self, sample_rate: f32) {
        let base = sane_rate(sample_rate);
        self.rate = f64::from(base);
        self.knobs.prepare(base);
        self.retune_now();
        self.design_line();
        self.reset();
    }

    fn reset(&mut self) {
        self.knobs.settle();
        self.retune_now();
        self.clock = Clock::default();
        self.clock_noise = Noise::new(0x51A7_0C1C);
        self.dither_noise = Noise::new(0xD17E_4A11);
        for channel in &mut self.channels {
            let (mut before, mut after) = (channel.before, channel.after);
            let (mut line, mut clean_line) = (channel.line, channel.clean_line);
            let (mut dry_line, mut pad) = (channel.dry_line, channel.pad);
            before.reset();
            after.reset();
            line.reset();
            clean_line.reset();
            dry_line.reset();
            pad.clear();
            *channel = Channel {
                before,
                after,
                line,
                clean_line,
                dry_line,
                pad,
                ..Channel::default()
            };
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.knobs.set(index, value);
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        for_each_frame(input, output, |left, right| self.frame(left, right));
    }

    /// The line stage's delay at low frequencies and the sample the
    /// converter runs behind, which the dry side of `mix` goes through
    /// too, padded out to a whole number of samples. The converter's own
    /// sample-and-hold and anti-aliasing filters are its sound, not
    /// latency.
    fn latency(&self) -> usize {
        self.latency
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{
        Limits, aliasing_db, built, conformance, db, fft, guitar, power_near, render_mono, rms,
        sine, spectrum,
    };
    use super::*;

    conformance!(
        KIND,
        Limits {
            worst_spur_db: None,
            hot: &[&[(AA, 1.0), (BITS, 16.0), (RATE, 32_000.0)]],
            hot_spur_db: Some(-48.0),
            unity: true,
            rough_glides: &[BITS],
            // Eight bits turn the check's quiet tone into a handful of
            // steps whose phase no fit can read; sixteen leave the path.
            latency_knobs: &[(BITS, 16.0)],
        }
    );

    #[test]
    fn the_modal_filter_is_a_butterworth() {
        let rate = 48_000.0;
        let mut filter = Modal::<POLES>::default();
        filter.design(1_000.0, rate);
        let mut out = 0.0;
        for _ in 0..48_000 {
            out = filter.ramp(1.0, 1.0);
        }
        assert!((out - 1.0).abs() < 1e-9, "DC gain {out}");
        let gain_at = |hz: f64| {
            let mut filter = filter;
            filter.reset();
            let mut last = 0.0;
            let mut peak = 0.0f64;
            for n in 0..96_000 {
                let now = (TAU * hz * f64::from(n) / rate).sin();
                let y = filter.ramp(last, now);
                last = now;
                if n > 48_000 {
                    peak = peak.max(y.abs());
                }
            }
            peak
        };
        let corner = gain_at(1_000.0);
        assert!(
            (corner - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01,
            "{corner}"
        );
        assert!(gain_at(2_000.0) < 0.005);
        // A held step lands where it is told: halfway through a sample is
        // halfway between stepping at the sample before and the one after.
        let mut early = filter;
        early.reset();
        let mut late = early;
        let mut half = early;
        early.step(0.0, 1.0, 1.0);
        late.step(0.0, 1.0, 0.0);
        half.step(0.0, 1.0, 0.5);
        let [e, l, h] = [early, late, half].map(|mut f| f.ramp(1.0, 1.0));
        assert!(h > l && h < e, "{l} < {h} < {e}");
    }

    /// The share of the output below 20 kHz that is not a true product
    /// `|k clock ± f|` of the clock and a 997 Hz tone, in dB, at host rate
    /// `rate`, with `aa` as given.
    fn stray_db(rate: f32, aa: f32, clock: f32) -> f64 {
        let mut crush = Crush::new();
        crush.prepare(rate);
        crush.set_param(BITS, 16.0);
        crush.set_param(RATE, clock);
        crush.set_param(AA, aa);
        crush.reset();
        let n = rate as usize;
        let input: Vec<f32> = (0..n)
            .map(|i| 0.5 * (TAU * 997.0 * i as f64 / f64::from(rate)).sin() as f32)
            .collect();
        let out = render_mono(&mut crush, &input);
        // The spectrum of the last half second below 20 kHz, zero-padded
        // to a fine grid: one FFT, the same measure at every host rate. The
        // window is the four-term Blackman-Harris, whose sidelobes sit
        // 92 dB down and whose main lobe (8 Hz either side over half a
        // second) fits inside the 12 Hz each product is given, so what
        // counts as stray is what the converter made, not the window's
        // leakage beside the products. (A Hann window leaks -55 dB at 13 Hz
        // off.)
        let tail = &out[n / 2..];
        let len = tail.len() as f64;
        let size = (4 * tail.len()).next_power_of_two();
        let mut re = vec![0.0f64; size];
        let mut im = vec![0.0f64; size];
        for (i, (slot, s)) in re.iter_mut().zip(tail).enumerate() {
            let phase = TAU * i as f64 / len;
            let window = 0.011_68f64.mul_add(
                -(3.0 * phase).cos(),
                0.141_28f64.mul_add(
                    (2.0 * phase).cos(),
                    0.488_29f64.mul_add(-phase.cos(), 0.358_75),
                ),
            );
            *slot = f64::from(*s) * window;
        }
        fft(&mut re, &mut im);
        let products: Vec<f64> = (0..=(40_000.0 / clock) as usize + 1)
            .flat_map(|k| {
                let centre = k as f64 * f64::from(clock);
                [centre + 997.0, (centre - 997.0).abs()]
            })
            .collect();
        let bin_hz = f64::from(rate) / size as f64;
        let (mut total, mut stray) = (0.0, 0.0);
        for (bin, (r, i)) in re.iter().zip(&im).enumerate().take(size / 2) {
            let hz = bin as f64 * bin_hz;
            if !(20.0..20_000.0).contains(&hz) {
                continue;
            }
            let power = r.mul_add(*r, i * i);
            total += power;
            if products.iter().all(|p| (p - hz).abs() > 12.0) {
                stray += power;
            }
        }
        10.0 * (stray.max(1e-30) / total).log10()
    }

    /// The converter sounds the same at every host rate, aa on or off (the
    /// default): everything it makes, the aliases and the staircase's
    /// images, is a true product of the clock and the tone, and what the
    /// host's own rate adds on top stays at least 80 dB down at 44.1, 48,
    /// 96 and 192 kHz. (Before the steps were placed between host samples
    /// and taken out through the line stage, that residue was -20 dB at
    /// 44.1 kHz and -33 dB at 192 kHz.)
    #[test]
    fn the_sound_does_not_depend_on_the_host_rate() {
        for aa in [0.0, 1.0] {
            for clock in [3_000.0, 7_000.0, 26_000.0] {
                for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
                    let stray = stray_db(rate, aa, clock);
                    assert!(
                        stray < -80.0,
                        "aa {aa}, clock {clock} at {rate} Hz: {stray:.1} dB"
                    );
                }
            }
        }
    }

    /// Half dry and half crushed at 16 bits, the two sides line up: all
    /// that tells them apart is the converter's hold, whose half-period
    /// delay and sinc droop are its sound. So the blend's response is the
    /// dry plus a zero-order hold at the clock, averaged, to within half a
    /// decibel to 15 kHz at every host rate: no comb from the line stage or
    /// the latency.
    #[test]
    fn a_blend_does_not_comb() {
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            // The clock cannot run faster than the host.
            let clock = 48_000.0f64.min(f64::from(rate));
            for hz in [1_000.0, 5_000.0, 10_000.0, 15_000.0] {
                let x = PI * hz / clock;
                let hold = x.sin() / x;
                let expected = 20.0
                    * (0.5f64.mul_add(hold * x.cos(), 0.5))
                        .hypot(0.5 * hold * x.sin())
                        .log10();
                let mut crush = Crush::new();
                crush.prepare(rate);
                crush.set_param(BITS, 16.0);
                crush.set_param(RATE, 48_000.0);
                crush.set_param(MIX, 50.0);
                crush.reset();
                let n = rate as usize / 2;
                let input: Vec<f32> = (0..n)
                    .map(|i| 0.25 * (TAU * hz * i as f64 / f64::from(rate)).sin() as f32)
                    .collect();
                let out = render_mono(&mut crush, &input);
                let level = db(rms(&out[n / 2..]) / rms(&input[n / 2..]));
                assert!(
                    (level - expected).abs() < 0.5,
                    "{hz} Hz at {rate} Hz: {level:.2} dB, the hold alone gives {expected:.2}"
                );
            }
        }
    }

    #[test]
    fn jitter_keeps_the_average_rate() {
        for jitter in [0.0, 0.45, 0.9] {
            let mut clock = Clock::default();
            let mut noise = Noise::new(7);
            let ticks = (0..480_000)
                .filter(|_| {
                    clock
                        .advance(1_000.0 / 48_000.0, jitter, || f64::from(noise.sample()))
                        .is_some()
                })
                .count();
            // Ten seconds of a 1 kHz clock.
            assert!(
                (9_700..=10_300).contains(&ticks),
                "jitter {jitter}: {ticks} ticks"
            );
        }
    }

    #[test]
    fn a_tick_knows_where_it_fell() {
        let mut clock = Clock::default();
        let increment = 0.3;
        let mut ticks = Vec::new();
        for _ in 0..10 {
            if let Some(ago) = clock.advance(increment, 0.0, || 0.0) {
                ticks.push(ago);
            }
        }
        // 0.3 per sample crosses 1 at 3.33 samples: 0.2 of a period over,
        // two thirds of a sample ago; the next at 6.67, one third ago.
        assert!((ticks[0] - 2.0 / 3.0).abs() < 1e-9, "{ticks:?}");
        assert!((ticks[1] - 1.0 / 3.0).abs() < 1e-9, "{ticks:?}");
    }

    #[test]
    fn the_aa_switch_takes_the_aliasing_out() {
        let at = |aa: f32| {
            let mut crush = Crush::new();
            crush.set_param(BITS, 16.0);
            crush.set_param(AA, aa);
            crush.reset();
            aliasing_db(&mut crush, 1_000.0, 0.5)
        };
        let raw = at(0.0);
        let clean = at(1.0);
        assert!(raw > -25.0, "the aliasing should be there: {raw:.1} dB");
        assert!(
            clean < raw - 30.0,
            "raw {raw:.1} dB, filtered {clean:.1} dB"
        );
    }

    #[test]
    fn a_tone_above_the_clock_folds_down_unless_filtered() {
        // 7 kHz sampled at 8 kHz comes back at 1 kHz.
        let input = sine(7_000.0, 0.5, 0.5);
        let at = |aa: f32| {
            let mut crush = built(&KIND);
            crush.set_param(BITS, 16.0);
            crush.set_param(AA, aa);
            power_near(&spectrum(&render_mono(&mut *crush, &input)), 1_000.0)
        };
        let folded = at(0.0);
        let filtered = at(1.0);
        assert!(10.0 * (folded / filtered).log10() > 40.0);
    }

    #[test]
    fn bits_set_the_steps() {
        // A steady 0.3 is rounded to the nearest step: 0.5 at 2 bits,
        // 0.25 at 3 and 4 bits, 0.3125 at 5.
        let input = vec![0.3f32; 48_000];
        for (bits, level) in [(2.0, 0.5), (3.0, 0.25), (4.0, 0.25), (5.0, 0.312_5)] {
            let mut crush = built(&KIND);
            crush.set_param(BITS, bits);
            crush.reset();
            let out = render_mono(&mut *crush, &input);
            let settled = out[47_999];
            assert!((settled - level).abs() < 1e-4, "{bits} bits: {settled}");
        }
    }

    #[test]
    fn jitter_and_dither_stay_in_bounds() {
        let input = guitar(1.0, 0.8);
        let mut crush = built(&KIND);
        crush.set_param(JITTER, 100.0);
        crush.set_param(DITHER, 100.0);
        crush.set_param(BITS, 4.0);
        let out = render_mono(&mut *crush, &input);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0 + 1e-6));
    }

    /// All dry, the input comes out at its own level from the bass to
    /// 15 kHz at every host rate: the line stage it shares with the
    /// converter takes nothing below its corner.
    #[test]
    fn a_dry_mix_is_the_input() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
            for hz in [300.0, 5_000.0, 15_000.0] {
                let n = rate as usize / 4;
                let input: Vec<f32> = (0..n)
                    .map(|i| 0.4 * (TAU * hz * i as f64 / f64::from(rate)).sin() as f32)
                    .collect();
                let mut crush = Crush::new();
                crush.prepare(rate);
                crush.set_param(MIX, 0.0);
                crush.reset();
                let out = render_mono(&mut crush, &input);
                let change = db(rms(&out[n / 2..]) / rms(&input[n / 2..]));
                assert!(change.abs() < 0.02, "{hz} Hz at {rate} Hz: {change:.3} dB");
            }
        }
    }
}
