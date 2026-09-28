//! `spring`: a spring reverb tank, the "drip" of surf guitar and dub.
//!
//! A real tank is a transducer twisting one end of two or three long
//! helical springs and a pickup at the other end. A twist travels down a
//! spring *dispersively*: its speed depends on frequency, so a click comes
//! out the far end as a chirp, and every time it bounces back and forth it
//! is smeared further. That chirp, repeating and smearing every 50 ms or so,
//! is the drip.
//!
//! The model follows Välimäki, Parker and Abel, *Parametric spring
//! reverberation effect* (J. Audio Eng. Soc., 2010) and Parker, *Efficient
//! dispersion generation structures for spring reverb emulation* (EURASIP,
//! 2011). Each spring is:
//!
//! - **The low chirp**: a cascade of eighty *stretched* first-order
//!   allpasses, `(a + z^-K) / (1 + a z^-K)`, inside a feedback loop. Each
//!   stage delays high frequencies more than low ones up to the corner at
//!   `fs / 2K` (about 4.4 kHz, the transition frequency of a real spring),
//!   so the cascade turns a click into a rising chirp. The chain sits inside
//!   the loop, so every echo passes through it again and smears further, as
//!   a real echo does. A lowpass at the corner in the loop and at the output
//!   keeps the chain's mirror images above the corner out.
//! - **The echo timing**: the pickup hears the first arrival after one
//!   trip down the spring (`D`) and then every round trip (`2D`), so the
//!   loop delay is `2D` and the output is read at `D`. The corner lowpass
//!   runs a sample and a half ahead of the analogue filter it copies, so
//!   both reads are made that much later, and the loop's resonances fall
//!   on the same frequencies at every rate.
//! - **The high chirp**: a shorter cascade of lightly stretched allpasses
//!   with a negative coefficient, in its own loop, kept between the corner
//!   and about 14 kHz: the faint, fast "splash" above the drip.
//!
//! Both cascades' stretches are fractional (see `Chains`), so the corners,
//! the chirps and the echo timing are the same at 44.1 kHz and 192 kHz.
//! - **Life**: each spring's delay wanders by a fraction of a millisecond
//!   (a slow sine plus smoothed noise), so repeats never line up exactly.
//!
//! Two or three springs of slightly different length and stiffness run in
//! parallel, the first to the left, the second to the right, the third to
//! both, as in the classic Accutronics tanks.
//!
//! **Tension** sets how long and how stiff the springs are: slack springs are
//! slower (longer `D`) and more dispersive (a larger allpass coefficient),
//! for a longer, droopier drip. **Decay** is a true RT60: the loop gain is
//! set from the round-trip time, chain included. **Drive** is the valve
//! driver in front of the tank: gain into a soft clipper. **Tone** is the
//! lowpass after the pickup.
//!
//! **The boing**: kick a real tank, or hit it with a hard transient, and the
//! springs lurch: their tension, and so their pitch, wobbles for a moment.
//! A fast envelope jumping well above a slow one, at a level that drives
//! the valve stage hard into clipping, spots such a hit, and it sets a
//! decaying five-and-a-half hertz wobble on every spring's delay, deeper the
//! harder the hit. Played gently, the tank never boings.

use crate::dsp::{DcBlocker, Noise, Phasor, flush};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

use super::halfband::{Down2, Halfband, Up2};
use super::kit::{
    CONTROL, Knobs, MAX_SLEW, Ramp, Slew, ceiling, clean, decay_gain, equal_power, frames, silence,
    sine, usable_rate,
};
use super::line::{Kernel, Line, MIN_SINC_READ};
use super::matched::{self, Biquad};

/// The chirp corner: a real spring's transition frequency.
const CORNER_HZ: f32 = 4_400.0;
/// Stretched allpasses in each low chirp chain.
const LOW_STAGES: usize = 80;
/// Lightly stretched allpasses in each high chirp chain, and their
/// coefficient.
const HIGH_STAGES: usize = 24;
const HIGH_COEFF: f32 = -0.6;
/// The high chirp's level against the drip.
const HIGH_LEVEL: f32 = 0.2;
/// One trip down the spring at full and at no tension, seconds.
const TRIP_SLACK: f32 = 0.048;
const TRIP_TIGHT: f32 = 0.024;
/// How each spring differs: length ratio, coefficient offset, sweep rate.
const SPRINGS: [(f32, f32, f32); 3] = [(1.0, 0.0, 0.37), (1.137, 0.02, 0.53), (0.883, -0.02, 0.29)];
/// How far each spring's delay wanders, seconds.
const WANDER: f32 = 0.000_25;
/// The boing: wobble rate, deepest wobble (seconds), and how fast it dies.
const BOING_HZ: f32 = 5.5;
const BOING_DEPTH: f32 = 0.0015;
const BOING_SECONDS: f32 = 0.35;
/// The pickup's level: the drip sits just under the dry sound.
const PICKUP_LEVEL: f32 = 0.66;
/// The frequency the decay knob is exact at: the heart of the drip.
const DECAY_HZ: f32 = 2_000.0;
/// Q of the two sections of a fourth-order Butterworth lowpass.
const BUTTERWORTH_4: [f64; 2] = [0.541_196_1, 1.306_563];
/// How often the cascades' state is swept for denormals, seconds (64
/// samples at 48 kHz): well inside the time a decaying state takes to fall
/// from the flush threshold into the denormal range.
const SWEEP_SECONDS: f32 = 0.001_33;
/// How fast the boing's wobble comes in, seconds.
const BOING_ONSET: f32 = 0.005;
/// The most the governor ever takes off: 12 dB, well past the deepest
/// resonance, so a misjudged prediction can never silence the tank.
const GOVERNOR_DEEPEST: f32 = 0.25;
/// The governor looks at energy over at least this long, seconds: long
/// enough that a hit's chirp and first echoes, however they spread, count
/// the same as the prediction does, while a note held on a resonance (which
/// takes many round trips to build) is still caught.
const GOVERNOR_FLOOR: f32 = 0.3;
/// The wander's noise cutoff, hertz.
const WANDER_HZ: f32 = 0.7;
/// How far (in power) the tank may ring above what the governor predicts
/// before it is held down: 3 dB.
const GOVERNOR_MARGIN: f32 = 1.995_262;
/// The tank's power gain on noise, over the `1 / (1 - g²)` that a loop
/// with round-trip gain `g` gives: what the chains, filters and the mix of
/// springs make of it, measured across decays and tensions (0.66 to 0.79).
const TANK_RATIO: f32 = 0.74;
/// How much of the other side's spring each side hears.
const CROSS: f32 = 0.5;
/// The high chirp's stretch at 44.1 kHz, scaled with the rate so its
/// corner sits near 14.7 kHz at every rate.
const HIGH_STRETCH_44K: f32 = 1.5;

const SPRING_COUNT: usize = 0;
const TENSION: usize = 1;
const DECAY: usize = 2;
const TONE: usize = 3;
const DRIVE: usize = 4;
const MIX: usize = 5;

static PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "springs",
        min: 2.0,
        max: 3.0,
        default: 3.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["2 springs", "3 springs"],
        },
    },
    ParamSpec {
        name: "tension",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "decay",
        min: 0.5,
        max: 8.0,
        default: 2.5,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "tone",
        min: 1_000.0,
        max: 8_000.0,
        default: 4_000.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "drive",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.35,
        unit: "",
        curve: Curve::Linear,
    },
];

/// Glide times; tension moves under a speed limit of its own.
const GLIDES: [f32; 6] = [0.0, 0.0, 0.05, 0.03, 0.03, 0.03];

/// The spring tank's entry in the catalogue.
pub static KIND: EffectKind = EffectKind {
    id: "spring",
    name: "Spring reverb",
    description: "A two- or three-spring tank: dispersive chirps, the surf drip, a valve driver and the boing when you hit it hard.",
    params: &PARAMS,
    build: || Box::new(Spring::new()),
};

/// Three cascades of identical stretched first-order allpasses, one per
/// spring. The three are run stage by stage together: each cascade is one
/// long serial chain of multiply-adds, and interleaving three independent
/// chains lets the processor overlap them.
///
/// The stretch `K` is fractional, so a cascade's timing in seconds is the
/// same at every sample rate: each stage's `z^-K` is `N` whole samples of
/// delay followed by a first-order Thiran allpass for the fraction
/// (`d` between 0.5 and 1.5 samples, coefficient `(1 - d) / (1 + d)`),
/// which is maximally flat in delay at low frequencies. This is Parker's
/// interpolated stretched allpass (2011).
#[derive(Debug, Clone, Default)]
struct Chains {
    /// Per cascade, `N + 1` samples of each stage's inner signal.
    history: [Vec<f32>; 3],
    /// Per cascade, each stage's last Thiran output.
    thiran_out: [Vec<f32>; 3],
    ring: usize,
    thiran: f32,
    position: usize,
}

impl Chains {
    /// Cascades of `stages` stages stretched by `stretch` samples (held at
    /// 1.5 or more, so each stage's delay is at least one whole sample).
    fn new(stages: usize, stretch: f32) -> Self {
        let stretch = if stretch.is_finite() {
            stretch.max(1.5)
        } else {
            1.5
        };
        let whole = (stretch - 0.5).floor();
        let fraction = stretch - whole;
        let ring = whole as usize + 1;
        Self {
            history: std::array::from_fn(|_| vec![0.0; stages * ring]),
            thiran_out: std::array::from_fn(|_| vec![0.0; stages]),
            ring,
            thiran: (1.0 - fraction) / (1.0 + fraction),
            position: 0,
        }
    }

    /// One sample through every stage of each cascade, cascade `i` taking
    /// `inputs[i]` with coefficient `coeffs[i]`.
    fn process(&mut self, inputs: [f32; 3], coeffs: [f32; 3]) -> [f32; 3] {
        let mut x = inputs;
        // The slot about to be written holds the inner signal N + 1 samples
        // back; the next slot holds it N samples back.
        let oldest = self.position;
        let newer = if oldest + 1 == self.ring {
            0
        } else {
            oldest + 1
        };
        let eta = self.thiran;
        let ring = self.ring;
        let [h0, h1, h2] = &mut self.history;
        let [t0, t1, t2] = &mut self.thiran_out;
        let stages = h0
            .chunks_exact_mut(ring)
            .zip(h1.chunks_exact_mut(ring))
            .zip(h2.chunks_exact_mut(ring))
            .zip(t0.iter_mut().zip(t1.iter_mut()).zip(t2.iter_mut()));
        for (((one, two), three), ((last0, last1), last2)) in stages {
            let cells = [(one, last0), (two, last1), (three, last2)];
            for (((cell, last), x), a) in cells.into_iter().zip(&mut x).zip(coeffs) {
                // The fractional delay: Thiran's first-order allpass.
                let delayed = eta.mul_add(cell[newer] - *last, cell[oldest]);
                *last = delayed;
                // The stretched allpass around it.
                let w = (-a).mul_add(delayed, *x);
                cell[oldest] = w;
                *x = a.mul_add(w, delayed);
            }
        }
        self.position = newer;
        x
    }

    /// Zero any state that has decayed into the denormal range. Done once
    /// a block rather than at every stage: a flush inside the cascade would
    /// sit on its serial path and cost several times the cascade itself,
    /// and between sweeps the state can spend at most a block near the
    /// denormal range.
    fn sweep(&mut self) {
        for state in self.history.iter_mut().chain(&mut self.thiran_out) {
            for value in state.iter_mut() {
                flush(value);
            }
        }
    }

    fn clear(&mut self) {
        for state in self.history.iter_mut().chain(&mut self.thiran_out) {
            state.fill(0.0);
        }
        self.position = 0;
    }
}

/// One spring: its low and high chirp loops and its wander.
#[derive(Debug, Clone, Default)]
struct Coil {
    line: Line,
    high_line: Line,
    loop_filter: [Biquad; 2],
    out_filter: [Biquad; 2],
    high_filter: Biquad,
    high_top: Biquad,
    sweep: Phasor,
    wander: Wander,
    /// How far the corner lowpass runs ahead of the analogue one it copies,
    /// in samples (see [`filter_lag`]): the loop and the pickup are read
    /// that much later.
    lag: f32,
}

/// Smoothed noise for a spring's wander, scaled to a standard deviation
/// of one whatever the cutoff. It is drawn on a fixed clock, a thousand
/// times a second, and glided between draws, so from the same seed the
/// wander is the same in time at every sample rate.
#[derive(Debug, Clone, Copy)]
struct Wander {
    source: Noise,
    state: f32,
    coeff: f32,
    scale: f32,
    /// The last two draws, and how far the clock is from one to the next.
    previous: f32,
    current: f32,
    phase: f32,
    /// Clock ticks a sample.
    step: f32,
}

impl Default for Wander {
    fn default() -> Self {
        Self {
            source: Noise::new(1),
            state: 0.0,
            coeff: 1.0,
            scale: 1.0,
            previous: 0.0,
            current: 0.0,
            phase: 0.0,
            step: 1.0,
        }
    }
}

/// The wander's clock, ticks a second.
const WANDER_CLOCK: f32 = 1_000.0;

/// What brings evenly spread noise (variance 1/3) through a one-pole
/// lowpass with coefficient `coeff` (variance `c / (2 - c)` of what goes
/// in) back to a standard deviation of one.
fn wander_scale(coeff: f32) -> f32 {
    (3.0 * (2.0 - coeff) / coeff.max(1e-9)).sqrt()
}

impl Wander {
    fn tune(&mut self, hz: f32, rate: f32) {
        self.coeff = 1.0 - (-std::f32::consts::TAU * hz / WANDER_CLOCK).exp();
        self.scale = wander_scale(self.coeff);
        self.step = WANDER_CLOCK / rate;
    }

    const fn reset(&mut self, seed: u32) {
        self.source = Noise::new(seed);
        self.state = 0.0;
        self.previous = 0.0;
        self.current = 0.0;
        self.phase = 0.0;
    }

    fn next(&mut self) -> f32 {
        self.phase += self.step;
        // Whole periods passed (one at most at any sane rate); the phase
        // is never negative.
        let passed = self.phase as u32;
        self.phase = self.phase.fract();
        for _ in 0..passed {
            self.previous = self.current;
            self.state = (self.source.sample() - self.state).mul_add(self.coeff, self.state);
            flush(&mut self.state);
            self.current = self.state * self.scale;
        }
        self.phase
            .mul_add(self.current - self.previous, self.previous)
    }
}

/// The group delay, samples, of a cascade of [`LOW_STAGES`] stretched
/// allpasses `(a + z^-K) / (1 + a z^-K)` at `hz`:
/// `K (1 - a²) / (1 + 2a cos Kω + a²)` a stage.
fn chain_delay(coeff: f32, stretch: f32, hz: f32, rate: f32) -> f32 {
    let angle = stretch * std::f32::consts::TAU * hz / rate;
    let a = coeff;
    let per_stage =
        stretch * a.mul_add(-a, 1.0) / (2.0 * a).mul_add(angle.cos(), a.mul_add(a, 1.0));
    LOW_STAGES as f32 * per_stage
}

/// How many samples the matched corner lowpass (a fourth-order
/// Butterworth at `corner`) runs ahead of the analogue filter it copies,
/// at `rate`. Matched designs copy the analogue magnitude, not its phase:
/// through the band the digital filter delays about 1.4 samples less, so
/// 28 µs less at 48 kHz but 7.5 µs less at 192 kHz, nearly the same at
/// every frequency. In a loop whose resonances are a fraction of a hertz
/// wide that is enough to move them (0.4 Hz near 1 kHz), and a note that
/// sat on one at one rate would sit off it at another. Measured at the
/// drip's heart, [`DECAY_HZ`].
fn filter_lag(corner: f64, rate: f64) -> f32 {
    let w = matched::omega(f64::from(DECAY_HZ), rate);
    let angle = matched::Angle::new(w);
    let x = f64::from(DECAY_HZ) / corner;
    let (digital, analogue) = BUTTERWORTH_4.iter().fold((0.0, 0.0), |(d, a), &q| {
        let design = matched::lowpass(corner, q, rate);
        (
            d + design.phase_at(angle),
            a - (x / q).atan2(x.mul_add(-x, 1.0)),
        )
    });
    // Phase is minus the angle times the delay.
    ((digital - analogue) / w) as f32
}

/// Filter through a cascade of biquads.
fn cascade(sections: &mut [Biquad], input: f64) -> f64 {
    sections
        .iter_mut()
        .fold(input, |signal, section| section.process(signal))
}

/// The valve driver's soft clipper, `tanh`, run at about 192 kHz whatever
/// the sample rate (four times at 44.1 or 48 kHz, twice at 88.2 or 96 kHz,
/// through half-band stages up and down, see `halfband`) and with
/// first-order antiderivative anti-aliasing on top (Parker, Zavalishin and
/// Le Bivic, DAFX 2016): each output is the average of `tanh` over the
/// straight line from the last input to this one,
/// `(F(x) - F(x₁)) / (x - x₁)` with `F = ln cosh`. Driven hard, a clipper
/// makes harmonics far past Nyquist; between them, the two keep what folds
/// back more than 60 dB down.
#[derive(Debug, Clone, Default)]
struct Driver {
    up: [Up2; 2],
    down: [Down2; 2],
    /// Half-band stages in use: two, one or none.
    stages: usize,
    last: f64,
}

/// `ln cosh x`, safe for any size of `x`.
fn log_cosh(x: f64) -> f64 {
    let size = x.abs();
    size + (-2.0 * size).exp().ln_1p() - std::f64::consts::LN_2
}

impl Driver {
    /// A driver with its oversampling filters for `rate`. Allocates.
    fn new(rate: f32) -> Self {
        let outer = || Halfband::new(16, 8.0);
        let inner = || Halfband::new(6, 8.0);
        let stages = if rate <= 50_000.0 {
            2
        } else {
            usize::from(rate <= 100_000.0)
        };
        Self {
            up: [Up2::new(outer()), Up2::new(inner())],
            down: [Down2::new(inner()), Down2::new(outer())],
            stages,
            last: 0.0,
        }
    }

    /// The clipper at the high rate, anti-aliased.
    fn shape(&mut self, input: f32) -> f32 {
        let x = f64::from(input);
        let before = self.last;
        self.last = x;
        let step = x - before;
        let out = if step.abs() > 1e-6 {
            (log_cosh(x) - log_cosh(before)) / step
        } else {
            (0.5 * (x + before)).tanh()
        };
        out as f32
    }

    fn clip(&mut self, input: f32) -> f32 {
        match self.stages {
            0 => self.shape(input),
            1 => {
                let [first, second] = self.up[0].process(input);
                let shaped = [self.shape(first), self.shape(second)];
                self.down[1].process(shaped)
            }
            _ => {
                let [first, second] = self.up[0].process(input);
                let [a, b] = self.up[1].process(first);
                let [c, d] = self.up[1].process(second);
                let shaped = [self.shape(a), self.shape(b), self.shape(c), self.shape(d)];
                let low = self.down[0].process([shaped[0], shaped[1]]);
                let high = self.down[0].process([shaped[2], shaped[3]]);
                self.down[1].process([low, high])
            }
        }
    }

    fn reset(&mut self) {
        for stage in &mut self.up {
            stage.reset();
        }
        for stage in &mut self.down {
            stage.reset();
        }
        self.last = 0.0;
    }
}

/// The per-spring numbers derived from the knobs.
#[derive(Debug, Clone, Copy, Default)]
struct Setting {
    coeff: f32,
    gain: f32,
    high_gain: f32,
}

impl Coil {
    fn clear(&mut self) {
        self.line.clear();
        self.high_line.clear();
        for section in self.loop_filter.iter_mut().chain(&mut self.out_filter) {
            section.reset();
        }
        self.high_filter.reset();
        self.high_top.reset();
    }

    /// The first half of a sample: what goes into the low and the high
    /// cascade, from `input` and what has come back round the loops. `trip`
    /// is one trip down the spring in samples, `wobble` the extra delay now.
    fn feed(&mut self, input: f32, trip: f32, wobble: f32, setting: Setting) -> (f32, f32) {
        let returned = self.line.read(2.0f32.mul_add(trip, wobble) + self.lag);
        let shaped = cascade(&mut self.loop_filter, f64::from(returned));
        let feedback = setting.gain * shaped as f32;
        let mut low = input + feedback;
        flush(&mut low);
        let mut high = setting
            .high_gain
            .mul_add(self.high_line.read(2.0 * trip), input);
        flush(&mut high);
        (low, high)
    }

    /// The second half: the cascades' outputs go into the loops, and the
    /// pickup hears them one trip later.
    fn pick_up(
        &mut self,
        chirped: f32,
        splashed: f32,
        trip: f32,
        wobble: f32,
        splash_on: f32,
    ) -> f32 {
        self.line.push(chirped);
        let late = self.line.read(0.5f32.mul_add(wobble, trip) + self.lag);
        let drip = cascade(&mut self.out_filter, f64::from(late)) as f32;
        self.high_line.push(splashed);
        let above = self
            .high_filter
            .process(f64::from(self.high_line.read(trip)));
        let splash = self.high_top.process(above) as f32;
        (HIGH_LEVEL * splash_on).mul_add(splash, drip)
    }
}

/// The resonance governor.
///
/// A spring tank is a set of resonant loops, and a steady note that lands
/// right on one of their resonances comes back far louder than the same
/// note a few hertz away, or than noise: about 10 dB at the default decay.
/// On a real tank that is part of the boing; on a wall's master bus it is a
/// clip waiting to happen. So the governor predicts how much energy the tank
/// *should* hold: the energy fed in, smoothed with the tank's own decay time
/// and scaled by the tank's average gain (what it gives back on noise). When
/// the tank's actual output runs more than [`GOVERNOR_MARGIN`] above that
/// prediction, the wet signal is turned down to it, quickly, and let back
/// up slowly. Drips, tails and hits all match the prediction and pass
/// untouched; only a note parked on a resonance is held down.
#[derive(Debug, Clone, Copy)]
struct Governor {
    expected: f32,
    /// Each side's smoothed power.
    actual: [f32; 2],
    gain: f32,
    expect_rate: f32,
    expect_ratio: f32,
    actual_rate: f32,
    attack: f32,
    release: f32,
}

impl Governor {
    fn new(rate: f32) -> Self {
        let per = |seconds: f32| 1.0 - (-1.0 / (seconds * rate)).exp();
        Self {
            expected: 0.0,
            actual: [0.0; 2],
            gain: 1.0,
            expect_rate: per(0.2),
            expect_ratio: 1.0,
            actual_rate: per(GOVERNOR_FLOOR),
            attack: per(0.02),
            release: per(0.3),
        }
    }

    /// Set the prediction for a tank that decays in `rt60` seconds and
    /// gives back `ratio` of the power fed into it on noise.
    fn tune(&mut self, rt60: f32, ratio: f32, rate: f32) {
        // Energy falls 60 dB in rt60: a time constant of rt60 / ln(10^6),
        // but never quicker than the governor's own window.
        let constant = (rt60 / 13.815_51).max(GOVERNOR_FLOOR);
        self.expect_rate = 1.0 - (-1.0 / (constant * rate)).exp();
        self.expect_ratio = ratio;
    }

    /// Given the power going into the tank and the wet pair coming out,
    /// the gain for the wet pair.
    fn process(&mut self, fed: f32, wet: [f32; 2]) -> f32 {
        let wanted = fed * self.expect_ratio;
        self.expected = (wanted - self.expected).mul_add(self.expect_rate, self.expected);
        for (side, &sample) in self.actual.iter_mut().zip(&wet) {
            *side = sample
                .mul_add(sample, -*side)
                .mul_add(self.actual_rate, *side);
            flush(side);
        }
        flush(&mut self.expected);
        // The louder side is the one that would clip.
        let actual = self.actual[0].max(self.actual[1]);
        let limit = self.expected * GOVERNOR_MARGIN;
        let target = if actual > limit && actual > 1e-12 {
            (limit / actual).sqrt().max(GOVERNOR_DEEPEST)
        } else {
            1.0
        };
        let rate = if target < self.gain {
            self.attack
        } else {
            self.release
        };
        self.gain = (target - self.gain).mul_add(rate, self.gain);
        if !self.gain.is_finite() {
            self.gain = 1.0;
        }
        self.gain
    }

    const fn reset(&mut self) {
        self.expected = 0.0;
        self.actual = [0.0; 2];
        self.gain = 1.0;
    }
}

/// The spring tank.
#[derive(Debug, Clone)]
pub struct Spring {
    rate: f32,
    prepared: bool,
    knobs: Knobs<6>,
    stretch: f32,
    coils: [Coil; 3],
    lows: Chains,
    highs: Chains,
    settings: [Setting; 3],
    third: Ramp,
    input_filter: Biquad,
    tone: [Biquad; 2],
    dc: [DcBlocker; 2],
    fast: f32,
    slow: f32,
    fast_attack: f32,
    fast_release: f32,
    slow_rate: f32,
    shake: f32,
    shake_target: f32,
    shake_onset: f32,
    shake_fall: f32,
    driver: Driver,
    /// The power going into the tank, delayed to when it reaches the
    /// pickup, for the governor.
    fed: Line,
    arrival: f32,
    tension: Slew,
    high_stretch: f32,
    splash_on: f32,
    sweep_countdown: usize,
    sweep_every: usize,
    boing: Phasor,
    governor: Governor,
    countdown: usize,
}

impl Default for Spring {
    fn default() -> Self {
        Self::new()
    }
}

impl Spring {
    /// A tank at the default settings, unprepared.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 0.0,
            prepared: false,
            knobs: Knobs::new(&PARAMS),
            stretch: 1.5,
            coils: Default::default(),
            lows: Chains::default(),
            highs: Chains::default(),
            settings: [Setting::default(); 3],
            third: Ramp::new(1.0),
            input_filter: Biquad::default(),
            tone: [Biquad::default(); 2],
            dc: [DcBlocker::new(48_000.0); 2],
            fast: 0.0,
            slow: 0.0,
            fast_attack: 1.0,
            fast_release: 1.0,
            slow_rate: 1.0,
            shake: 0.0,
            shake_target: 0.0,
            shake_onset: 1.0,
            shake_fall: 1.0,
            driver: Driver::default(),
            fed: Line::default(),
            arrival: MIN_SINC_READ,
            tension: Slew::new(0.5, 0.0),
            high_stretch: 1.5,
            splash_on: 1.0,
            sweep_countdown: 64,
            sweep_every: 64,
            boing: Phasor::default(),
            governor: Governor::new(48_000.0),
            countdown: 0,
        }
    }

    /// One trip down spring `index`, in samples, at the moving tension.
    fn trip(&self, index: usize) -> f32 {
        let tension = self.tension.value();
        let seconds = tension.mul_add(TRIP_TIGHT - TRIP_SLACK, TRIP_SLACK);
        seconds * SPRINGS[index].0 * self.rate
    }

    fn update(&mut self) {
        let tension = self.tension.value();
        let decay = self.knobs.get(DECAY);
        let trips: [f32; 3] = std::array::from_fn(|index| self.trip(index));
        let high_chain = HIGH_STAGES as f32 * self.high_stretch;
        for ((setting, trip), spring) in self.settings.iter_mut().zip(trips).zip(&SPRINGS) {
            let coeff = (0.2f32.mul_add(-tension, 0.75) + spring.1).clamp(0.3, 0.85);
            // The round trip as the drip's heart hears it: the loop delay
            // and the cascade's group delay there.
            let chain = chain_delay(coeff, self.stretch, DECAY_HZ, self.rate);
            let round_trip = 2.0f32.mul_add(trip, chain) / self.rate;
            let high_trip = 2.0f32.mul_add(trip, high_chain) / self.rate;
            *setting = Setting {
                coeff,
                gain: decay_gain(round_trip, decay),
                // The splash dies twice as fast as the drip.
                high_gain: decay_gain(high_trip, 0.5 * decay),
            };
        }
        // The first sound reaches the pickup a little over one trip after
        // it goes in (the splash's short cascade barely delays it), less
        // whatever the wander and the boing can take off.
        let shortest = trips.iter().fold(f32::MAX, |least, &trip| least.min(trip));
        let slack = WANDER.mul_add(2.0, BOING_DEPTH) * self.rate;
        self.arrival = (shortest - slack).max(MIN_SINC_READ);
        let loop_gain = self.settings[0].gain;
        let ratio = TANK_RATIO / loop_gain.mul_add(-loop_gain, 1.0).max(1e-3);
        self.governor.tune(decay, ratio, self.rate);
        let count = self.knobs.target(SPRING_COUNT);
        let target = if count >= 2.5 { 1.0 } else { 0.0 };
        if (self.third.target() - target).abs() > f32::EPSILON {
            self.third.set(target, 0.05 * self.rate);
        }
        let tone = matched::lowpass(
            f64::from(self.knobs.get(TONE)),
            std::f64::consts::FRAC_1_SQRT_2,
            f64::from(self.rate),
        );
        for side in &mut self.tone {
            side.set(tone);
        }
    }

    /// The driver: bass trimmed, gain into a soft clipper, and the gain
    /// taken off again after it, so the knob changes how hard the tank is
    /// driven, not how loud it is. Returns the level going into the clipper
    /// and what comes out.
    fn drive(&mut self, input: f32) -> (f32, f32) {
        let drive = self.knobs.get(DRIVE);
        let gain = (11.0 * drive).mul_add(drive, 1.0);
        let hot = gain * self.input_filter.process(f64::from(input)) as f32;
        (hot, self.driver.clip(hot) / gain)
    }

    /// Watch for hits and advance the boing; returns the extra delay now.
    /// A hit is a sudden jump well past where the driver starts to clip.
    fn boing(&mut self, hot: f32) -> f32 {
        let level = hot.abs();
        let rate = if level > self.fast {
            self.fast_attack
        } else {
            self.fast_release
        };
        self.fast = (level - self.fast).mul_add(rate, self.fast);
        self.slow = (level - self.slow).mul_add(self.slow_rate, self.slow);
        flush(&mut self.fast);
        flush(&mut self.slow);
        let jolt = 2.0f32.mul_add(-self.slow, self.fast) - 1.0;
        if jolt > 0.0 {
            let strength = (0.25 * jolt).min(1.0);
            if strength > self.shake_target {
                // A wobble starting from rest starts from the middle of its
                // swing, so the springs' delay does not jump.
                if self.shake < 1e-4 {
                    self.boing.set(0.0);
                }
                self.shake_target = strength;
            }
        }
        self.shake_target *= self.shake_fall;
        flush(&mut self.shake_target);
        // The wobble's depth follows over a few milliseconds.
        self.shake = (self.shake_target - self.shake).mul_add(self.shake_onset, self.shake);
        flush(&mut self.shake);
        let phase = self.boing.next(BOING_HZ, self.rate);
        self.shake * BOING_DEPTH * self.rate * sine(phase)
    }

    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.knobs.step();
        self.tension.set(self.knobs.get(TENSION));
        self.tension.next();
        if self.countdown == 0 {
            self.update();
            self.countdown = CONTROL;
        }
        self.countdown -= 1;
        let third = self.third.next();
        let (left, right) = (clean(left), clean(right));
        let (hot, driven) = self.drive(0.5 * (left + right));
        let wobble = self.boing(hot);

        let trips: [f32; 3] = std::array::from_fn(|index| self.trip(index));
        let mut delays = [0.0f32; 3];
        let mut lows = [0.0f32; 3];
        let mut highs = [0.0f32; 3];
        for (index, coil) in self.coils.iter_mut().enumerate() {
            let wander = sine(coil.sweep.next(SPRINGS[index].2, self.rate));
            let noise = coil.wander.next();
            let drift = WANDER * self.rate * 0.5 * (wander + noise);
            delays[index] = wobble + drift;
            let feed = if index == 2 { driven * third } else { driven };
            (lows[index], highs[index]) =
                coil.feed(feed, trips[index], delays[index], self.settings[index]);
        }
        let chirped = self
            .lows
            .process(lows, self.settings.map(|setting| setting.coeff));
        let splashed = self.highs.process(highs, [HIGH_COEFF; 3]);
        let mut pickups = [0.0f32; 3];
        for (index, (pickup, coil)) in pickups.iter_mut().zip(&mut self.coils).enumerate() {
            *pickup = coil.pick_up(
                chirped[index],
                splashed[index],
                trips[index],
                delays[index],
                self.splash_on,
            );
        }
        // Each side hears its own spring, some of the other, and the third
        // spring in the middle, so a note that lands on one spring's
        // resonance is always blended with springs that are off theirs.
        let shared = std::f32::consts::FRAC_1_SQRT_2 * third * pickups[2];
        let norm = 1.0 / (0.5f32.mul_add(third, CROSS.mul_add(CROSS, 1.0))).sqrt();
        let mut wet = [
            CROSS.mul_add(pickups[1], pickups[0] + shared) * norm,
            CROSS.mul_add(pickups[0], pickups[1] + shared) * norm,
        ];
        self.fed.push(driven * driven);
        let held = self.governor.process(self.fed.read(self.arrival), wet);
        for side in &mut wet {
            *side *= held;
        }
        let (dry_gain, wet_gain) = equal_power(self.knobs.get(MIX));
        let mut out = [0.0f32; 2];
        for (side, (dry, wet)) in [left, right].into_iter().zip(wet).enumerate() {
            let toned = self.tone[side].process(f64::from(wet)) as f32;
            let settled = self.dc[side].process(toned);
            out[side] = dry_gain.mul_add(dry, wet_gain * ceiling(PICKUP_LEVEL * settled));
        }
        self.sweep_countdown = self.sweep_countdown.saturating_sub(1);
        if self.sweep_countdown == 0 {
            self.lows.sweep();
            self.highs.sweep();
            self.sweep_countdown = self.sweep_every;
        }
        out.into()
    }
}

impl Effect for Spring {
    fn prepare(&mut self, sample_rate: f32) {
        let Some(rate) = usable_rate(sample_rate) else {
            self.prepared = false;
            return;
        };
        self.rate = rate;
        let kernel = Kernel::new(rate);
        // Fractional stretches keep both corners, and so the whole sound,
        // the same at every rate from 44.1 kHz up.
        self.stretch = (rate / (2.0 * CORNER_HZ)).max(1.5);
        let corner = f64::from(rate) / (2.0 * f64::from(self.stretch));
        let high_stretch = (HIGH_STRETCH_44K * rate / 44_100.0).max(1.5);
        self.high_stretch = high_stretch;
        let high_corner = f64::from(rate) / (2.0 * f64::from(high_stretch));
        // Below 44.1 kHz the splash's band narrows; once it is less than
        // half an octave wide there is no splash left to hear.
        self.splash_on = if high_corner >= 1.5 * corner {
            1.0
        } else {
            0.0
        };
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let longest =
            (TRIP_SLACK * SPRINGS[1].0).mul_add(2.0, WANDER.mul_add(3.0, BOING_DEPTH)) * rate;
        self.lows = Chains::new(LOW_STAGES, self.stretch);
        self.highs = Chains::new(HIGH_STAGES, high_stretch);
        for coil in &mut self.coils {
            coil.line = Line::new(longest as usize + 8, &kernel);
            coil.high_line = Line::new(longest as usize + 8, &kernel);
            for (index, &section_q) in BUTTERWORTH_4.iter().enumerate() {
                let design = matched::lowpass(corner, section_q, f64::from(rate));
                coil.loop_filter[index].set(design);
                coil.out_filter[index].set(design);
            }
            coil.lag = filter_lag(corner, f64::from(rate));
            coil.high_filter
                .set(matched::highpass(corner, q, f64::from(rate)));
            coil.high_top
                .set(matched::lowpass(0.95 * high_corner, q, f64::from(rate)));
            coil.wander.tune(WANDER_HZ, rate);
        }
        self.input_filter
            .set(matched::highpass(120.0, q, f64::from(rate)));
        self.dc = [DcBlocker::new(rate); 2];
        let per = |seconds: f32| 1.0 - (-1.0 / (seconds * rate)).exp();
        self.fast_attack = per(0.001);
        self.fast_release = per(0.04);
        self.slow_rate = per(0.15);
        self.shake_fall = (-1.0 / (BOING_SECONDS * rate)).exp();
        self.shake_onset = per(BOING_ONSET);
        self.driver = Driver::new(rate);
        self.sweep_every = ((SWEEP_SECONDS * rate) as usize).max(1);
        self.governor = Governor::new(rate);
        self.fed = Line::new(((TRIP_SLACK + 0.02) * rate) as usize + 8, &kernel);
        // Tension moves the longest loop read, two trips of the longest
        // spring, by at most MAX_SLEW samples a sample.
        let reach = 2.0 * (TRIP_SLACK - TRIP_TIGHT) * SPRINGS[1].0 * rate;
        self.tension.set_speed(MAX_SLEW / reach);
        self.tension.snap(self.knobs.get(TENSION));
        self.knobs.prepare(rate, &GLIDES);
        let third = if self.knobs.target(SPRING_COUNT) >= 2.5 {
            1.0
        } else {
            0.0
        };
        self.third.snap(third);
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        self.lows.clear();
        self.highs.clear();
        for (index, coil) in self.coils.iter_mut().enumerate() {
            coil.clear();
            coil.wander.reset(0x5EED_0000 + index as u32);
            coil.sweep.set(index as f32 / 3.0);
        }
        self.input_filter.reset();
        for side in &mut self.tone {
            side.reset();
        }
        for side in &mut self.dc {
            side.reset();
        }
        self.fast = 0.0;
        self.slow = 0.0;
        self.shake = 0.0;
        self.shake_target = 0.0;
        self.boing.set(0.0);
        self.governor.reset();
        self.driver.reset();
        self.fed.clear();
        self.sweep_countdown = self.sweep_every;
        self.countdown = 0;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.knobs.set(index, value);
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], mut output: [&mut [f32]; 2]) {
        if !self.prepared {
            silence(&mut output);
            return;
        }
        let count = frames(input, &mut output);
        let [out_left, out_right] = output;
        for n in 0..count {
            let (l, r) = self.frame(input[0][n], input[1][n]);
            out_left[n] = l;
            out_right[n] = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{self, RATE, build, impulse, peak, rms, run_mono, silence};

    /// The corner lowpass runs about a sample and a half ahead of the
    /// analogue filter at every rate, and with that taken off its phase is
    /// the analogue filter's through the drip's band.
    #[test]
    fn the_filter_lag_lines_the_loop_up_with_the_analogue_one() {
        for rate in [44_100.0f64, 48_000.0, 96_000.0, 192_000.0] {
            let corner = rate / (2.0 * (rate / (2.0 * f64::from(super::CORNER_HZ))).max(1.5));
            let lag = f64::from(super::filter_lag(corner, rate));
            assert!((1.2..1.6).contains(&lag), "{rate}: {lag}");
            for hz in [250.0, 1_000.0, 3_000.0] {
                let w = super::matched::omega(hz, rate);
                let angle = super::matched::Angle::new(w);
                let x = hz / corner;
                let (digital, analogue) =
                    super::BUTTERWORTH_4.iter().fold((0.0, 0.0), |(d, a), &q| {
                        let design = super::matched::lowpass(corner, q, rate);
                        (
                            d + design.phase_at(angle),
                            a - (x / q).atan2(x.mul_add(-x, 1.0)),
                        )
                    });
                let off = (w.mul_add(-lag, digital) - analogue) / w;
                assert!(off.abs() < 0.02, "{rate} Hz, {hz} Hz: {off:.4} samples");
            }
        }
    }

    #[test]
    fn the_spring_keeps_the_contract() {
        testing::contract("spring");
    }

    #[test]
    fn the_wet_tank_sits_just_under_the_dry_on_noise() {
        let level = testing::noise_level("spring", &[], 0.05);
        assert!((-3.0..=0.0).contains(&level), "{level:.2} dB");
    }

    #[test]
    fn no_steady_tone_rings_more_than_six_decibels_over_the_dry() {
        // Every 1/12 octave from 100 Hz to 8 kHz (and every 1/96 octave in
        // the calibration that set the levels), a steady tone, held long
        // enough for the tank and its governor to settle.
        let steps = (80.0f32.log2() * 12.0) as i32;
        for step in 0..=steps {
            let hz = 100.0 * (step as f32 / 12.0).exp2();
            let mut spring = build("spring", &[("mix", 1.0)]);
            let tone = testing::sine(1.0, hz, 0.05);
            let (left, right) = run_mono(spring.as_mut(), &tone);
            let settled = (0.6 * RATE) as usize;
            let dry = rms(&tone[settled..]);
            let loudest = rms(&left[settled..]).max(rms(&right[settled..]));
            let level = 20.0 * (loudest / dry).log10();
            assert!(level <= 6.0, "{hz:.1} Hz rings at {level:.2} dB");
        }
    }

    #[test]
    fn a_tight_spring_is_the_same_at_every_rate() {
        testing::rates_agree_with("spring", &[("tension", 1.0), ("tone", 8_000.0)]);
    }

    #[test]
    fn tension_never_bends_the_drip_past_the_speed_limit() {
        use crate::Effect;
        let mut spring = super::Spring::new();
        spring.set_param(super::TENSION, 0.0);
        spring.prepare(RATE);
        spring.set_param(super::TENSION, 1.0);
        let input = [0.1f32];
        let (mut left, mut right) = ([0.0f32], [0.0f32]);
        let longest = |spring: &super::Spring| 2.0 * spring.trip(1);
        let mut last = longest(&spring);
        for _ in 0..(3.0 * RATE) as usize {
            spring.process(&testing::CONTEXT, [&input, &input], [&mut left, &mut right]);
            let now = longest(&spring);
            assert!(
                (now - last).abs() <= super::MAX_SLEW * 1.05,
                "moved {}",
                now - last
            );
            last = now;
        }
        assert!(spring.tension.settled(), "the tension never arrived");
    }

    #[test]
    fn nothing_reaches_the_pickup_before_one_trip_down_the_spring() {
        let mut spring = build(
            "spring",
            &[("mix", 1.0), ("springs", 2.0), ("tension", 0.5)],
        );
        let (left, _) = run_mono(spring.as_mut(), &impulse(1.0));
        let trip = 0.036;
        let span = |from: f32, to: f32| rms(&left[(from * RATE) as usize..(to * RATE) as usize]);
        let before = span(0.0, 0.9 * trip);
        let arrival = span(trip, 2.0 * trip);
        let echoes = span(3.0 * trip, 6.0 * trip);
        assert!(
            before < 0.01 * arrival,
            "sound before the first trip: {before} vs {arrival}"
        );
        assert!(echoes > 0.1 * arrival, "no echoes: {echoes} vs {arrival}");
    }

    #[test]
    fn a_hard_hit_makes_it_boing() {
        // The same click, soft and hard: the hard one wobbles the pitch, so
        // the tails differ by more than their level.
        let run = |level: f32| {
            let mut spring = build("spring", &[("mix", 1.0), ("drive", 1.0)]);
            let mut hit = impulse(1.5);
            for sample in hit.iter_mut().take(200) {
                *sample = level;
            }
            let (left, _) = run_mono(spring.as_mut(), &hit);
            let top = peak(&left);
            left.iter().map(|x| x / top).collect::<Vec<f32>>()
        };
        let soft = run(0.02);
        let hard = run(1.0);
        let late = (0.3 * RATE) as usize..(1.0 * RATE) as usize;
        let difference: Vec<f32> = soft[late.clone()]
            .iter()
            .zip(&hard[late.clone()])
            .map(|(a, b)| a - b)
            .collect();
        assert!(rms(&difference) > 0.2 * rms(&soft[late]), "no boing");
    }

    /// A band of `signal` around `hz`: two one-pole highpasses a third of
    /// an octave below and two lowpasses a third above.
    fn band(signal: &[f32], hz: f32) -> Vec<f32> {
        let edge = 1.26f32;
        let low = testing::lowpassed(&testing::lowpassed(signal, hz * edge), hz * edge);
        testing::highpassed(&testing::highpassed(&low, hz / edge), hz / edge)
    }

    #[test]
    fn the_decay_knob_is_a_true_rt60_through_the_drip() {
        for decay in [1.0f32, 2.5, 8.0] {
            for tension in [0.0f32, 1.0] {
                let mut spring = build(
                    "spring",
                    &[
                        ("mix", 1.0),
                        ("decay", decay),
                        ("tension", tension),
                        ("drive", 0.0),
                    ],
                );
                let mut click = impulse(decay.mul_add(1.2, 0.5));
                click[0] = 0.1;
                let (left, right) = run_mono(spring.as_mut(), &click);
                let mono: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l + r).collect();
                for hz in [500.0f32, 2_000.0] {
                    let measured = testing::rt60(&band(&mono, hz));
                    assert!(
                        (measured / decay - 1.0).abs() < 0.15,
                        "decay {decay} s, tension {tension}, {hz} Hz: {measured} s"
                    );
                }
            }
        }
    }

    /// Run a spring built directly, in 32-frame blocks, and report the
    /// quietest its governor got.
    fn lowest_governor_gain(knobs: &[(usize, f32)], input: &[f32]) -> f32 {
        use crate::Effect;
        let mut spring = super::Spring::new();
        spring.set_param(super::MIX, 1.0);
        for &(index, value) in knobs {
            spring.set_param(index, value);
        }
        spring.prepare(RATE);
        let mut lowest = 1.0f32;
        let (mut left, mut right) = ([0.0f32; 32], [0.0f32; 32]);
        for block in input.chunks(32) {
            let size = block.len();
            spring.process(
                &testing::CONTEXT,
                [block, block],
                [&mut left[..size], &mut right[..size]],
            );
            lowest = lowest.min(spring.governor.gain);
        }
        lowest
    }

    #[test]
    fn hits_pass_the_governor_untouched() {
        let mut hit = testing::noise(0.05, 0.8, 5);
        for (n, sample) in hit.iter_mut().enumerate() {
            *sample *= (-(n as f32) / (0.01 * RATE)).exp();
        }
        hit.extend(silence(1.5));
        let mut click = impulse(1.5);
        click[0] = 0.8;
        for decay in [0.5f32, 8.0] {
            for tension in [0.0f32, 1.0] {
                for input in [&hit, &click] {
                    let knobs = [(super::DECAY, decay), (super::TENSION, tension)];
                    let lowest = lowest_governor_gain(&knobs, input);
                    assert!(
                        20.0 * lowest.log10() > -0.5,
                        "decay {decay}, tension {tension}: held down to {lowest}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_boing_starts_without_a_jump() {
        use crate::Effect;
        let mut spring = super::Spring::new();
        spring.set_param(super::DRIVE, 1.0);
        spring.prepare(RATE);
        let wobble = |spring: &super::Spring| {
            spring.shake * super::BOING_DEPTH * spring.rate * super::sine(spring.boing.phase())
        };
        // Let the boing's phase run to somewhere mid-swing first.
        let quiet = [0.0f32; 1];
        let (mut left, mut right) = ([0.0f32], [0.0f32]);
        for _ in 0..(0.37 * RATE) as usize {
            spring.process(&testing::CONTEXT, [&quiet, &quiet], [&mut left, &mut right]);
        }
        let mut last = wobble(&spring);
        let mut worst = 0.0f32;
        for n in 0..(0.5 * RATE) as usize {
            let hit = [if n < 200 { 1.0 } else { 0.0 }];
            spring.process(&testing::CONTEXT, [&hit, &hit], [&mut left, &mut right]);
            let now = wobble(&spring);
            worst = worst.max((now - last).abs());
            last = now;
        }
        assert!(spring.shake > 0.1, "the hit never boinged");
        // A full wobble's steepest slope is about 0.05 samples a sample.
        assert!(worst < 0.06, "the delay jumped {worst} samples");
    }

    #[test]
    fn the_driver_does_not_fold_its_harmonics_back() {
        // 3.1 kHz driven twelve times into the clipper: its odd harmonics
        // run far past Nyquist, and each one that folds back lands between
        // the harmonics, where it can be measured.
        let mut driver = super::Driver::new(RATE);
        let hz = 3_100.0f32;
        let total = (1.0 * RATE) as usize;
        let out: Vec<f32> = (0..total)
            .map(|n| driver.clip(6.0 * (std::f32::consts::TAU * hz * n as f32 / RATE).sin()))
            .collect();
        let window = &out[total / 2..];
        let at = |f: f32| {
            let size = window.len() as f64;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, &x) in window.iter().enumerate() {
                let hann = 0.5f64.mul_add(-(std::f64::consts::TAU * n as f64 / size).cos(), 0.5);
                let angle = std::f64::consts::TAU * f64::from(f) * n as f64 / f64::from(RATE);
                re = (f64::from(x) * hann).mul_add(angle.cos(), re);
                im = (f64::from(x) * hann).mul_add(angle.sin(), im);
            }
            re.hypot(im)
        };
        let fundamental = at(hz);
        let mut worst = f64::MIN;
        for harmonic in (9..80).step_by(2) {
            let above = harmonic as f32 * hz;
            let folded = (above % RATE).min(RATE - above % RATE);
            if folded > 100.0 && folded < 20_000.0 {
                worst = worst.max(20.0 * (at(folded) / fundamental).log10());
            }
        }
        assert!(worst < -60.0, "folded harmonics at {worst} dBc");
    }

    #[test]
    fn the_wander_noise_has_the_same_spread_at_any_rate() {
        for rate in [44_100.0f32, 192_000.0] {
            let mut wander = super::Wander::default();
            wander.tune(20.0, rate);
            let values: Vec<f32> = (0..(20.0 * rate) as usize).map(|_| wander.next()).collect();
            let spread = rms(&values[rate as usize..]);
            assert!((spread - 1.0).abs() < 0.05, "{rate} Hz: {spread}");
        }
    }

    #[test]
    fn the_cascades_never_hold_denormals() {
        use crate::Effect;
        let mut spring = super::Spring::new();
        spring.set_param(super::DECAY, 0.5);
        spring.prepare(RATE);
        let mut burst = testing::noise(0.3, 0.8, 9);
        burst.extend(silence(20.0));
        let (mut left, mut right) = (vec![0.0f32; 4_096], vec![0.0f32; 4_096]);
        for block in burst.chunks(4_096) {
            let size = block.len();
            spring.process(
                &testing::CONTEXT,
                [block, block],
                [&mut left[..size], &mut right[..size]],
            );
            let state = spring
                .lows
                .history
                .iter()
                .chain(&spring.lows.thiran_out)
                .chain(&spring.highs.history)
                .chain(&spring.highs.thiran_out);
            for values in state {
                assert!(
                    values.iter().all(|x| *x == 0.0 || x.is_normal()),
                    "a denormal in a cascade"
                );
            }
        }
    }

    #[test]
    fn below_44_khz_the_splash_bows_out() {
        use crate::Effect;
        let mut spring = super::Spring::new();
        spring.prepare(16_000.0);
        assert!(spring.splash_on.abs() < f32::EPSILON);
        spring.prepare(44_100.0);
        assert!((spring.splash_on - 1.0).abs() < f32::EPSILON);
    }
}
