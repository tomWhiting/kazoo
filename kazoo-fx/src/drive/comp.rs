//! Comp: a stereo-linked compressor with three characters.
//!
//! Every compressor here shares one gain computer: below the threshold
//! nothing happens, above it every `ratio` decibels in becomes one out,
//! and across the `knee` the ratio eases in along a quadratic (after
//! Giannoulis, Massberg and Reiss). What differs is how each one listens
//! and how its gain element moves:
//!
//! - **vca**: a clean modern bus compressor, after the dbx and SSL designs.
//!   It listens to the RMS level (a 5 ms window), feed-forward, and moves
//!   its gain smoothly in decibels with the attack and release as set.
//! - **fet**: a fast, aggressive peak compressor, after the 1176. It
//!   listens to the peaks of its own output (feedback): the gain it needs
//!   is solved each sample from its own effect, so it grabs transients
//!   hard and pumps musically. The FET used as the gain element adds a
//!   little second harmonic that grows with the gain reduction: the
//!   output's square less its own running mean, over its running level,
//!   so it is a harmonic of the note and never a thump of the envelope.
//! - **opto**: a slow, forgiving levelling amp, after the LA-2A's
//!   electro-optical cell. It listens to peaks, feed-forward, with a
//!   softer knee (never under 10 dB). Its light cell remembers: the longer
//!   and harder it has been working, the slower it lets go (up to five
//!   times the release), and that memory fades over several seconds, as
//!   the T4 cell's does.
//!
//! The `model` switch crossfades between them: all three listen all the
//! time. Both channels share one gain, from the louder of the two, so the
//! stereo image never shifts. The detector hears the input through a
//! `sidechain` highpass, so a bass line need not pump the whole mix. The
//! `mix` knob blends the compressed signal with the dry for parallel
//! ("New York") compression, and `makeup` is capped at 18 dB, where a
//! guitar peaking at -12 dBFS still stays clear of the output ceiling.
//!
//! A gain that moves within a cycle, as the FET's does at its fastest, is
//! itself a kind of distortion, and its products fold like any other. So
//! the detectors and the gain run at an internal rate of at least
//! 176.4 kHz (4x at 44.1 and 48 kHz, as is at 176.4 and 192 kHz; lower
//! only below 11.025 kHz, where the 16x cap bites), which
//! also makes every model behave the same at every host rate.
//!
//! Sources: D. Giannoulis, M. Massberg and J. D. Reiss, "Digital dynamic
//! range compressor design: a tutorial and analysis" (JAES, 2012);
//! Universal Audio's 1176 and LA-2A manuals for the topologies and time
//! behaviour.

use super::filter::{Analogue, Iir3, warp};
use super::kit::{
    DEFAULT_RATE, Knobs, Retune, ceiling, flush64, for_each_frame, fraction, gain as db_gain,
    one_pole, weight,
};
use super::oversample::{MAX_FACTOR, Oversampler, TARGET_RATE, factor_for};
use super::solve::rising_root;
use crate::dsp::sane_rate;
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const THRESHOLD: usize = 0;
const RATIO: usize = 1;
const ATTACK: usize = 2;
const RELEASE: usize = 3;
const KNEE: usize = 4;
const MAKEUP: usize = 5;
const MODEL: usize = 6;
const SIDECHAIN: usize = 7;
const MIX: usize = 8;

static PARAMS: [ParamSpec; 9] = [
    ParamSpec {
        name: "threshold",
        min: -60.0,
        max: 0.0,
        default: -18.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "ratio",
        min: 1.0,
        max: 20.0,
        default: 4.0,
        unit: "",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "attack",
        min: 0.000_1,
        max: 0.1,
        default: 0.01,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "release",
        min: 0.01,
        max: 2.0,
        default: 0.15,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "knee",
        min: 0.0,
        max: 24.0,
        default: 6.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "makeup",
        min: 0.0,
        max: 18.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "model",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["vca", "fet", "opto"],
        },
    },
    ParamSpec {
        name: "sidechain",
        min: 20.0,
        max: 500.0,
        default: 20.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 100.0,
        default: 100.0,
        unit: "%",
        curve: Curve::Linear,
    },
];

/// The Comp's entry in the catalogue.
pub const KIND: EffectKind = EffectKind {
    id: "comp",
    name: "Compressor",
    description: "A stereo-linked compressor with clean VCA, fast FET and slow opto characters, a sidechain highpass and parallel mix.",
    params: &PARAMS,
    build: || Box::new(Comp::new()),
};

/// The VCA's RMS window.
const RMS_SECONDS: f64 = 0.005;
/// The opto's softest-allowed knee.
const OPTO_KNEE: f64 = 10.0;
/// How much slower the opto lets go when its memory is full.
const OPTO_MEMORY_SLOWDOWN: f64 = 4.0;
/// How fast the opto's memory fills while it works, and fades after.
const OPTO_CHARGE_SECONDS: f64 = 2.0;
const OPTO_FADE_SECONDS: f64 = 5.0;
/// The gain reduction above which the opto counts as working.
const OPTO_WORKING_DB: f64 = 1.0;
/// The FET's second harmonic, relative to the note, at 20 dB of gain
/// reduction: about -23 dB.
const FET_COLOUR: f64 = 0.1;
/// The quietest level the colour is measured against.
const COLOUR_FLOOR: f64 = 1e-4;
/// Silence, in decibels.
const FLOOR_DB: f64 = -120.0;

/// The gain computer: how many decibels to take off a signal `level` dB
/// loud, with the knee eased quadratically.
fn reduction(level: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    overshoot(level - threshold, knee) * (1.0 - 1.0 / ratio)
}

/// How far `over` decibels past the threshold counts, through a knee
/// `knee` dB wide: nothing below it, all of it above, a quadratic between.
fn overshoot(over: f64, knee: f64) -> f64 {
    if 2.0 * over <= -knee {
        0.0
    } else if 2.0 * over < knee {
        let into = over + knee / 2.0;
        into * into / (2.0 * knee)
    } else {
        over
    }
}

/// The slope of [`overshoot`].
fn overshoot_slope(over: f64, knee: f64) -> f64 {
    if 2.0 * over <= -knee {
        0.0
    } else if 2.0 * over < knee {
        (over + knee / 2.0) / knee
    } else {
        1.0
    }
}

/// Decibels of a linear level, with silence at [`FLOOR_DB`].
fn decibels(level: f64) -> f64 {
    if level > 1e-6 {
        20.0 * level.log10()
    } else {
        FLOOR_DB
    }
}

/// What the knobs mean this sample.
#[derive(Debug, Clone, Copy)]
struct Settings {
    threshold: f64,
    ratio: f64,
    knee: f64,
    attack: f64,
    release: f64,
    /// The release time itself, which the opto stretches.
    release_seconds: f64,
}

/// The coefficients that follow the internal rate alone.
#[derive(Debug, Clone, Copy, Default)]
struct Rates {
    rate: f64,
    rms: f64,
    charge: f64,
    fade: f64,
    /// The quadrature pair's coefficients at the internal rate.
    quadrature: [f64; QUADRATURE_COEFFICIENTS],
}

/// The three characters' detectors and gain elements, each holding its
/// gain reduction in decibels.
#[derive(Debug, Clone, Copy, Default)]
struct Detectors {
    mean_square: f64,
    vca: f64,
    fet: f64,
    opto: f64,
    memory: f64,
}

impl Detectors {
    fn vca(&mut self, power: f64, rms: f64, settings: &Settings) -> f64 {
        self.mean_square = (power - self.mean_square).mul_add(rms, self.mean_square);
        flush64(&mut self.mean_square);
        // The RMS of a sine is 3 dB under its peak: put that back so the
        // threshold means the same to every model.
        let level = decibels((2.0 * self.mean_square).sqrt());
        let target = reduction(level, settings.threshold, settings.ratio, settings.knee);
        let rate = if target > self.vca {
            settings.attack
        } else {
            settings.release
        };
        self.vca = (target - self.vca).mul_add(rate, self.vca);
        flush64(&mut self.vca);
        self.vca
    }

    /// The feedback FET: the reduction `g` is solved from
    /// `g = g' + a ((R - 1) k(in - T - g) - g')`, where `in - g` is the
    /// level it hears at its own output and `k` the knee.
    fn fet(&mut self, peak: f64, settings: &Settings) -> f64 {
        let level = decibels(peak);
        let slope = settings.ratio - 1.0;
        let before = self.fet;
        let solve = |rate: f64| {
            let low = before * (1.0 - rate);
            let reach = slope * overshoot(level - settings.threshold - low, settings.knee);
            let high = rate.mul_add(reach, low);
            rising_root(low, high.max(low), before, |g| {
                let over = level - settings.threshold - g;
                let pull = slope * overshoot(over, settings.knee);
                let value = g - rate.mul_add(pull - before, before);
                let dslope = rate * slope * overshoot_slope(over, settings.knee);
                (value, 1.0 + dslope)
            })
        };
        let attacking = solve(settings.attack);
        self.fet = if attacking >= before {
            attacking
        } else {
            solve(settings.release)
        };
        flush64(&mut self.fet);
        self.fet
    }

    fn opto(&mut self, peak: f64, settings: &Settings, rates: &Rates) -> f64 {
        let knee = settings.knee.max(OPTO_KNEE);
        let target = reduction(decibels(peak), settings.threshold, settings.ratio, knee);
        let working = self.opto > OPTO_WORKING_DB;
        self.memory = if working {
            (1.0 - self.memory).mul_add(rates.charge, self.memory)
        } else {
            self.memory * (1.0 - rates.fade)
        };
        flush64(&mut self.memory);
        let step = if target > self.opto {
            settings.attack
        } else {
            let slower = OPTO_MEMORY_SLOWDOWN.mul_add(self.memory, 1.0);
            one_pole(settings.release_seconds * slower, rates.rate)
        };
        self.opto = (target - self.opto).mul_add(step, self.opto);
        flush64(&mut self.opto);
        self.opto
    }
}

/// How many allpass coefficients the quadrature pair uses, split between
/// its two paths.
const QUADRATURE_COEFFICIENTS: usize = 16;
/// The lowest pitch the quadrature pair must hold at 90° apart.
const QUADRATURE_LOW_HZ: f64 = 20.0;

/// The coefficients of a pair of allpass chains whose outputs stay 90°
/// apart from `transition` up to `0.5 - transition` of the sample rate:
/// Laurent de Soras's design (HIIR), from the elliptic halfband filter.
fn quadrature_coefficients(transition: f64) -> [f64; QUADRATURE_COEFFICIENTS] {
    use std::f64::consts::PI;
    let modulus = (transition.mul_add(-2.0, 1.0) * PI / 4.0).tan().powi(2);
    let root = modulus.mul_add(-modulus, 1.0).max(0.0).powf(0.25);
    let eta = 0.5 * (1.0 - root) / (1.0 + root);
    let eta4 = eta.powi(4);
    let nome = eta * eta4.mul_add(eta4.mul_add(eta4.mul_add(150.0, 15.0), 2.0), 1.0);
    let order = (QUADRATURE_COEFFICIENTS * 2 + 1) as f64;
    let mut coefficients = [0.0; QUADRATURE_COEFFICIENTS];
    for (index, coefficient) in coefficients.iter_mut().enumerate() {
        let place = (index + 1) as f64;
        let mut numerator = 0.0;
        let mut sign = 1.0;
        for i in 0..64 {
            let i = f64::from(i);
            let term =
                nome.powf(i * (i + 1.0)) * (i.mul_add(2.0, 1.0) * place * PI / order).sin() * sign;
            numerator += term;
            sign = -sign;
            if term.abs() <= 1e-100 {
                break;
            }
        }
        let mut denominator = 0.0;
        let mut sign = -1.0;
        for i in 1..64 {
            let i = f64::from(i);
            let term = nome.powf(i * i) * (i * 2.0 * place * PI / order).cos() * sign;
            denominator += term;
            sign = -sign;
            if term.abs() <= 1e-100 {
                break;
            }
        }
        let ratio = numerator * nome.powf(0.25) / (denominator + 0.5);
        let squared = ratio * ratio;
        let product = squared.mul_add(-modulus, 1.0) * (1.0 - squared / modulus);
        let spread = product.max(0.0).sqrt() / (1.0 + squared);
        *coefficient = (1.0 - spread) / (1.0 + spread);
    }
    coefficients
}

/// One second-order allpass section in `z⁻²`, shifted a quarter of the
/// sample rate up from the halfband it was designed as:
/// `y = c (x + y'') - x''`.
#[derive(Debug, Clone, Copy, Default)]
struct Allpass2 {
    inputs: [f64; 2],
    outputs: [f64; 2],
}

impl Allpass2 {
    fn process(&mut self, input: f64, coefficient: f64) -> f64 {
        let out = coefficient.mul_add(input + self.outputs[1], -self.inputs[1]);
        self.inputs = [input, self.inputs[0]];
        self.outputs = [out, self.outputs[0]];
        out
    }
}

/// Two allpass chains whose outputs are 90° apart across the audio band:
/// the in-phase and quadrature parts of a signal.
#[derive(Debug, Clone, Copy, Default)]
struct Quadrature {
    even: [Allpass2; QUADRATURE_COEFFICIENTS / 2],
    odd: [Allpass2; QUADRATURE_COEFFICIENTS / 2],
    /// The last input: the second chain hears the signal a sample late.
    last: f64,
}

impl Quadrature {
    fn process(&mut self, input: f64, coefficients: &[f64; QUADRATURE_COEFFICIENTS]) -> [f64; 2] {
        let mut first = input;
        for (section, pair) in self.even.iter_mut().zip(coefficients.chunks_exact(2)) {
            first = section.process(first, pair[0]);
        }
        let mut second = self.last;
        self.last = input;
        for (section, pair) in self.odd.iter_mut().zip(coefficients.chunks_exact(2)) {
            second = section.process(second, pair[1]);
        }
        for state in [&mut first, &mut second] {
            flush64(state);
        }
        [first, second]
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    oversampler: Oversampler,
    sidechain: Iir3,
    quadrature: Quadrature,
}

impl Channel {
    /// The FET's second harmonic for output `wet`, at strength `colour`.
    /// With `i` and `q` the output's in-phase and quadrature parts, a note
    /// `A cos θ` gives `(i² - q²) / 2 = (A² / 2) cos 2θ`: its second harmonic
    /// and nothing of its envelope. Divided by the note's size
    /// `√(i² + q²)`, it grows with the note as a harmonic should.
    fn colour(
        &mut self,
        wet: f64,
        colour: f64,
        coefficients: &[f64; QUADRATURE_COEFFICIENTS],
    ) -> f64 {
        let [i, q] = self.quadrature.process(wet, coefficients);
        let size = i.hypot(q).max(COLOUR_FLOOR);
        colour * 0.5 * i.mul_add(i, -q * q) / size
    }
}

/// A compressor. See the module documentation for the three characters.
#[derive(Debug)]
pub struct Comp {
    knobs: Knobs<9>,
    rates: Rates,
    /// Attack, release and sidechain as last turned into coefficients.
    retune: Retune<3>,
    attack: f64,
    release: f64,
    detectors: Detectors,
    /// Left and right, each on the heap: with their oversamplers the pair
    /// is larger than a stack frame should carry.
    channels: [Box<Channel>; 2],
}

impl Default for Comp {
    fn default() -> Self {
        Self::new()
    }
}

impl Comp {
    /// A compressor at its default settings, ready for 48 kHz until
    /// prepared.
    #[must_use]
    pub fn new() -> Self {
        let mut comp = Self {
            knobs: Knobs::new(&PARAMS),
            rates: Rates::default(),
            retune: Retune::new([
                PARAMS[ATTACK].default,
                PARAMS[RELEASE].default,
                PARAMS[SIDECHAIN].default,
            ]),
            attack: 0.0,
            release: 0.0,
            detectors: Detectors::default(),
            channels: [Box::default(), Box::default()],
        };
        comp.prepare(DEFAULT_RATE);
        comp
    }

    fn tune(&mut self, [attack, release, sidechain]: [f32; 3]) {
        let rate = self.rates.rate;
        self.attack = one_pole(f64::from(attack), rate);
        self.release = one_pole(f64::from(release), rate);
        let omega = warp(f64::from(sidechain), rate);
        for channel in &mut self.channels {
            channel.sidechain.design(
                &Analogue::highpass2(omega, std::f64::consts::FRAC_1_SQRT_2),
                2.0 * rate,
            );
        }
    }

    /// One oversampled frame through the detectors and the gain.
    fn tick(&mut self, dry: [f64; 2], settings: &Settings, knob: &[f32; 9]) -> [f64; 2] {
        let [heard_left, heard_right] = [0, 1].map(|i| self.channels[i].sidechain.process(dry[i]));
        let peak = heard_left.abs().max(heard_right.abs());
        let power = 0.5 * heard_left.mul_add(heard_left, heard_right * heard_right);
        let weights = [0.0, 1.0, 2.0].map(|step| f64::from(weight(knob[MODEL], step)));
        let vca = self.detectors.vca(power, self.rates.rms, settings);
        let fet = self.detectors.fet(peak, settings);
        let opto = self.detectors.opto(peak, settings, &self.rates);
        let reduced = weights[2].mul_add(opto, weights[1].mul_add(fet, weights[0] * vca));
        let gain = db_gain(knob[MAKEUP]) * 10f64.powf(-reduced / 20.0);
        let colour = weights[1] * FET_COLOUR * (fet / 20.0).min(1.0);
        let mix = fraction(knob[MIX]);
        let coefficients = self.rates.quadrature;
        [0, 1].map(|i| {
            let wet = dry[i] * gain;
            let second = self.channels[i].colour(wet, colour, &coefficients);
            mix.mul_add(wet + second - dry[i], dry[i])
        })
    }

    /// Design for where the knobs are now, with nothing left waiting:
    /// after a prepare or a reset.
    fn retune_now(&mut self) {
        let knob = self.knobs.values();
        let now = [knob[ATTACK], knob[RELEASE], knob[SIDECHAIN]];
        self.retune.settle(now);
        self.tune(now);
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let knob = self.knobs.step();
        let timing = [knob[ATTACK], knob[RELEASE], knob[SIDECHAIN]];
        if self.retune.due(timing) {
            self.tune(timing);
        }
        let settings = Settings {
            threshold: f64::from(knob[THRESHOLD]),
            ratio: f64::from(knob[RATIO]).max(1.0),
            knee: f64::from(knob[KNEE]),
            attack: self.attack,
            release: self.release,
            release_seconds: f64::from(self.retune.tuned()[1]),
        };
        let factor = self.channels[0].oversampler.factor();
        let up = [
            self.channels[0].oversampler.expand(left),
            self.channels[1].oversampler.expand(right),
        ];
        let mut out = [[0.0f32; MAX_FACTOR]; 2];
        for k in 0..factor {
            let dry = [f64::from(up[0][k]), f64::from(up[1][k])];
            let [wet_left, wet_right] = self.tick(dry, &settings, &knob);
            out[0][k] = wet_left as f32;
            out[1][k] = wet_right as f32;
        }
        [0, 1].map(|i| ceiling(self.channels[i].oversampler.reduce(out[i])))
    }
}

impl Effect for Comp {
    fn prepare(&mut self, sample_rate: f32) {
        let base = sane_rate(sample_rate);
        let factor = factor_for(base, TARGET_RATE);
        let rate = f64::from(base) * factor as f64;
        self.rates = Rates {
            rate,
            rms: one_pole(RMS_SECONDS, rate),
            charge: one_pole(OPTO_CHARGE_SECONDS, rate),
            fade: one_pole(OPTO_FADE_SECONDS, rate),
            quadrature: quadrature_coefficients(QUADRATURE_LOW_HZ / rate),
        };
        for channel in &mut self.channels {
            channel.oversampler.configure(factor, base, 0.0);
        }
        self.knobs.prepare(base);
        self.retune_now();
        self.reset();
    }

    fn reset(&mut self) {
        self.knobs.settle();
        self.retune_now();
        self.detectors = Detectors::default();
        for channel in &mut self.channels {
            channel.oversampler.reset();
            channel.sidechain.reset();
            channel.quadrature = Quadrature::default();
        }
    }

    fn latency(&self) -> usize {
        self.channels[0].oversampler.latency()
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.knobs.set(index, value);
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        for_each_frame(input, output, |left, right| self.frame(left, right));
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{
        Limits, built, conformance, db, guitar, render, render_mono, rms, sine,
    };
    use super::*;

    conformance!(
        KIND,
        Limits {
            worst_spur_db: Some(-80.0),
            // Every character slammed: vca, fet and opto.
            hot: &[
                &[
                    (RATIO, 20.0),
                    (ATTACK, 0.000_1),
                    (THRESHOLD, -40.0),
                    (MODEL, 0.0),
                ],
                &[
                    (RATIO, 20.0),
                    (ATTACK, 0.000_1),
                    (THRESHOLD, -40.0),
                    (MODEL, 1.0),
                ],
                &[
                    (RATIO, 20.0),
                    (ATTACK, 0.000_1),
                    (THRESHOLD, -40.0),
                    (MODEL, 2.0),
                ],
            ],
            hot_spur_db: Some(-48.0),
            unity: false,
            rough_glides: &[],
            latency_knobs: &[],
        }
    );

    /// The steady-state change in level of a 1 kHz tone at `input_db`.
    fn change(model: f32, input_db: f64) -> f64 {
        let mut comp = built(&KIND);
        comp.set_param(MODEL, model);
        comp.set_param(KNEE, 0.0);
        comp.set_param(THRESHOLD, -20.0);
        comp.set_param(RATIO, 4.0);
        let amplitude = 10f64.powf(input_db / 20.0) as f32;
        let input = sine(1_000.0, amplitude, 3.0);
        let out = render_mono(&mut *comp, &input);
        let tail = 96_000..;
        db(rms(&out[tail.clone()]) / rms(&input[tail]))
    }

    #[test]
    fn every_model_holds_the_ratio() {
        for model in [0.0, 1.0, 2.0] {
            // 10 dB over at 4:1 should come out 2.5 dB over: 7.5 dB down.
            let reduced = change(model, -10.0);
            assert!(
                (reduced + 7.5).abs() < 1.0,
                "model {model}: {reduced:.2} dB"
            );
            let untouched = change(model, -30.0);
            assert!(untouched.abs() < 0.1, "model {model}: {untouched:.2} dB");
        }
    }

    /// The quadrature pair holds its two outputs 90° apart, within a
    /// degree, from 20 Hz to 20 kHz at the lowest internal rate.
    #[test]
    fn the_quadrature_pair_is_ninety_degrees_apart() {
        let rate = 176_400.0;
        let coefficients = quadrature_coefficients(QUADRATURE_LOW_HZ / rate);
        for hz in [20.0, 50.0, 200.0, 1_000.0, 5_000.0, 20_000.0] {
            let mut pair = Quadrature::default();
            let n = (rate * 2.0) as usize;
            let (mut cross, mut first, mut second) = (0.0, 0.0, 0.0);
            for i in 0..n {
                let x = (std::f64::consts::TAU * hz * i as f64 / rate).sin();
                let [a, b] = pair.process(x, &coefficients);
                if i > n / 2 {
                    cross = a.mul_add(b, cross);
                    first = a.mul_add(a, first);
                    second = b.mul_add(b, second);
                }
            }
            // The normalised correlation of two sines is the cosine of the
            // angle between them: zero at 90°.
            let cosine = cross / (first * second).sqrt();
            let degrees = cosine.acos().to_degrees();
            assert!((degrees - 90.0).abs() < 1.0, "{hz} Hz: {degrees:.2}°");
            assert!(
                ((first / second).sqrt() - 1.0).abs() < 0.01,
                "{hz} Hz: unequal"
            );
        }
    }

    /// The FET's colour is a second harmonic of the note and nothing else:
    /// a note swelling in and out ten times a second gives colour at twice
    /// its pitch, and next to none down where the swelling would thump (a
    /// colour made from the plain square of the note puts about as much
    /// there as at the harmonic).
    #[test]
    fn the_fet_colour_is_a_harmonic_not_a_thump() {
        let mut comp = Comp::new();
        comp.prepare(44_100.0);
        let rate = comp.rates.rate;
        let coefficients = comp.rates.quadrature;
        let channel = &mut comp.channels[0];
        let n = rate as usize;
        let out: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / rate;
                let swell = 0.5f64.mul_add(-(std::f64::consts::TAU * 10.0 * t).cos(), 0.5);
                let y = 0.5 * swell * (std::f64::consts::TAU * 220.0 * t).sin();
                channel.colour(y, 1.0, &coefficients)
            })
            .collect();
        let power_at = |hz: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, s) in out.iter().enumerate() {
                let angle = std::f64::consts::TAU * hz * i as f64 / rate;
                re = s.mul_add(angle.cos(), re);
                im = s.mul_add(angle.sin(), im);
            }
            re.mul_add(re, im * im)
        };
        let thump: f64 = (1..20).map(|k| power_at(f64::from(k) * 5.0)).sum();
        let harmonic: f64 = (-6..=6)
            .map(|k| power_at(f64::from(k).mul_add(5.0, 440.0)))
            .sum();
        let ratio = 10.0 * (thump / harmonic).log10();
        assert!(ratio < -40.0, "thump {ratio:.1} dB against the harmonic");
    }

    #[test]
    fn the_knee_eases_in() {
        assert!(reduction(-21.0, -20.0, 4.0, 0.0).abs() < f64::EPSILON);
        let soft = reduction(-21.0, -20.0, 4.0, 6.0);
        assert!(soft > 0.0 && soft < 0.75);
        assert!((reduction(-10.0, -20.0, 4.0, 6.0) - 7.5).abs() < 1e-9);
    }

    #[test]
    fn the_link_holds_the_image() {
        let loud = sine(500.0, 0.8, 1.0);
        let quiet = sine(500.0, 0.05, 1.0);
        let mut comp = built(&KIND);
        let (left, right) = render(&mut *comp, &loud, &quiet);
        let left_gain = rms(&left[24_000..]) / rms(&loud[24_000..]);
        let right_gain = rms(&right[24_000..]) / rms(&quiet[24_000..]);
        assert!((db(left_gain) - db(right_gain)).abs() < 0.05);
        assert!(db(right_gain) < -6.0);
    }

    #[test]
    fn the_sidechain_highpass_ignores_the_bass() {
        let bass = sine(40.0, 0.5, 2.0);
        let at = |sidechain: f32| {
            let mut comp = built(&KIND);
            comp.set_param(SIDECHAIN, sidechain);
            let out = render_mono(&mut *comp, &bass);
            db(rms(&out[48_000..]) / rms(&bass[48_000..]))
        };
        assert!(at(20.0) < -6.0);
        assert!(at(500.0) > -1.0);
    }

    #[test]
    fn the_opto_lets_go_slower_after_working_hard() {
        let recovery = |hold_seconds: f64| {
            let mut comp = built(&KIND);
            comp.set_param(MODEL, 2.0);
            let mut input = sine(1_000.0, 0.8, hold_seconds);
            let quiet_start = input.len();
            input.extend(sine(1_000.0, 0.05, 0.3));
            let out = render_mono(&mut *comp, &input);
            let window = quiet_start + 4_800..quiet_start + 9_600;
            rms(&out[window.clone()]) / rms(&input[window])
        };
        assert!(recovery(4.0) < recovery(0.2) * 0.9);
    }

    #[test]
    fn parallel_mix_and_makeup_are_bounded() {
        let input = guitar(1.0, 0.9);
        let mut comp = built(&KIND);
        comp.set_param(MAKEUP, 18.0);
        comp.set_param(RATIO, 1.0);
        comp.set_param(MIX, 50.0);
        let out = render_mono(&mut *comp, &input);
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 2.0));
    }
}
