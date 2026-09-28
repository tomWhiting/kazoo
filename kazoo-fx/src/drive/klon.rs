//! Klon: a transparent overdrive after the Klon Centaur.
//!
//! The Centaur splits the guitar in three and adds the paths back together
//! as currents into the virtual ground of an inverting summing amplifier.
//! One path goes through an op-amp with a lot of gain into a pair of
//! germanium diodes that clip it hard to ground; a second, clean path is
//! turned down by the gain pot's second gang as the first gang turns the
//! drive up; a third, always there, feeds the clean low end forward. An
//! active treble control after the sum puts the top back. The clean paths
//! keep the low end and the pick attack, hence "transparent".
//!
//! The model is the circuit, part for part, from the gain stage's netlist
//! (every value below is the pedal's; the designators are the schematic's),
//! run at an internal rate of at least 176.4 kHz (4x at 44.1 and 48 kHz,
//! 2x at 88.2 and 96 kHz, as is at 176.4 and 192 kHz; lower only below
//! 11.025 kHz, where the 16x cap bites). The pedal runs around a 4.5 V
//! bias, which is ground here.
//!
//! 1. The input buffer's coupling, a highpass at 16 Hz.
//! 2. The front network, solved by nodal analysis (see `network`): the
//!    buffer drives C3 (100 nF) into a node that feeds the clipping
//!    amplifier's input through R6 (10 kΩ) and C5 (68 nF) in parallel, with
//!    the gain pot's first gang, from zero to 100 kΩ as the gain turns up,
//!    from that input to ground; and feeds the clean low end forward
//!    through R7 (1.5 kΩ) to C16 (1 µF) to ground and R19 (15 kΩ) into the
//!    summing node (Electrosmash's first feed-forward network, a lowpass
//!    at 106 Hz). At no gain the pot shorts the amplifier's input, so the
//!    drive is silent and the pedal is its clean paths alone.
//! 3. The clipping amplifier, non-inverting: R12 (422 kΩ) with C8
//!    (390 pF) across it in the feedback, and to ground R11 (15 kΩ) with
//!    C7 (82 nF) across it, then R3 (2 kΩ) and the rest of the first gang
//!    (100 kΩ at no gain, nothing at full):
//!    `H(s) = 1 + (R12 / (1 + s R12 C8)) / (Rleg + R11 / (1 + s R11 C7))`,
//!    from 4.6x at no gain to 26x at full in the bass and up to 212x
//!    (46.5 dB) in the mids, rolled off by C8 above 967 Hz. It runs from
//!    9 V around the bias, so it swings ±4.5 V, with a soft knee.
//! 4. The back network, solved by nodal analysis with the diodes inside
//!    it: the amplifier drives C9 (1 µF) and R13 (1 kΩ) into two germanium
//!    diodes (1N34A-like, Is = 2 µA, n = 1.5, about 0.3 V forward), back to
//!    back to ground; that node feeds C10 (1 µF) into R16 (47 kΩ) to the
//!    summing node. The second clean path (Electrosmash's second
//!    feed-forward network): the buffer drives R5 (5.1 kΩ) and C4 (68 nF)
//!    in parallel into a node loaded by R8 (1.5 kΩ) and by C6 (390 nF) with
//!    R9 (1 kΩ) to ground; the gain pot's second gang runs from there
//!    through its upper part (zero to 100 kΩ as the gain turns up) to its
//!    wiper, and through its lower part (100 kΩ to zero) to ground; the
//!    wiper feeds the summing node through R17 (27 kΩ) and through R18
//!    (12 kΩ) with C12 (27 nF), and the clipped path through R15 (22 kΩ)
//!    with C11 (2.2 nF). So the clean path fades as the drive comes up.
//!    The diodes are solved against the network's exact Thévenin
//!    equivalent at their node every step, `(Vopen - V) / Z = 2 Is
//!    sinh(V / n Vt)`, by bracketed Newton, then the network takes the
//!    current they drew. A hard clip makes harmonics far past even the
//!    internal rate, so the clipper is also anti-aliased by its
//!    antiderivative (first-order ADAA): what the pedal puts out is the
//!    mean over the step of the diode current, `(F(x) - F(x')) / (x - x')`
//!    for the diode voltage, where `F` has a closed form in it: since
//!    `x = V + Z I(V)`, `F = ∫V dx = V²/2 + Z (V I(V) - ∫I dV)`. Because a
//!    difference of `F` is divided by the input step, the node is solved to
//!    1e-13 V. The rest of the sum, linear, is averaged over the same step
//!    so every path stays aligned; the network itself moves on with the
//!    diode's exact voltage at each step.
//! 5. The summing amplifier: R20 (392 kΩ) with C13 (820 pF) across it
//!    turns the sum of the currents into a voltage, and makes a lowpass at
//!    495 Hz across the whole sum, which the treble control then answers.
//!    It runs from +16.2 V and -8.6 V around the bias. It inverts; the
//!    model turns its sign back, so the pedal is in phase with its input.
//! 6. The treble control: an active shelf from 408 Hz (R22, C14) with
//!    unity gain below it. The 10 kΩ pot sets the gain above it from
//!    `R23 / (RV2 + R21)` = 0.4 (-8 dB) fully down to
//!    `(RV2 + R23) / R21` = 8.16 (+18.2 dB) fully up, with R21 = 1.8 kΩ and
//!    R23 = 4.7 kΩ.
//! 7. The output level.
//!
//! The gain pot is a dual 100 kΩ linear (B) pot. Turning it up does two
//! things at once, as in the pedal: the drive gets louder and the clean
//! path quieter, and the drive wins, so more gain is louder.
//!
//! Sources: Electrosmash, "Klon Centaur Analysis"; J. Chowdhury, "A
//! Comparison of Virtual Analog Modelling Techniques for Desktop and
//! Embedded Implementations" (DAFX 2020) and its gain-stage netlist (the
//! `ChowCentaur` repository, `GainStageTraining/SPICE/GainStage2.asc`), for
//! the parts and how they connect; W. Shockley's diode equation; Parker,
//! Zavalishin and Le Bivic, "Reducing the aliasing of nonlinear
//! waveshaping using continuous-time convolution" (DAFX 2016); the
//! bilinear transform for every linear stage.

use std::f64::consts::TAU;

use super::filter::{Analogue, Iir3};
use super::kit::{
    DEFAULT_RATE, Knobs, Retune, ceiling, for_each_frame, fraction, gain as db_gain, rail,
};
use super::network::{End, Network, Part};
use super::oversample::TARGET_RATE;
use super::pair::{self, Pair};
use super::solve::{Diode, rising_root_within};
use crate::dsp::sane_rate;
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

/// How far the clipper's antiderivative delays: half a sample at its rate.
const ADAA_DELAY: f64 = 0.5;

const GAIN: usize = 0;
const TREBLE: usize = 1;
const LEVEL: usize = 2;

static PARAMS: [ParamSpec; 3] = [
    ParamSpec {
        name: "gain",
        min: 0.0,
        max: 100.0,
        default: 40.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "treble",
        min: 0.0,
        max: 100.0,
        default: 50.0,
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

/// The Klon's entry in the catalogue.
pub const KIND: EffectKind = EffectKind {
    id: "klon",
    name: "Klon overdrive",
    description: "A Centaur-style transparent overdrive: germanium diodes clip a driven path that is blended back with the clean guitar.",
    params: &PARAMS,
    build: || Box::new(Klon::new()),
};

/// Volts at the pedal's input for a full-scale sample.
const INPUT_VOLTS: f64 = 1.0;
/// Full scale per volt at the pedal's output, set so the default settings
/// come out as loud as they went in.
const OUTPUT_SCALE: f64 = 0.103;

const COUPLING_HZ: f64 = 16.0;
/// Each gang of the gain pot, end to end.
const GAIN_POT: f64 = 100e3;
/// The least a pot section is taken to be: its wiper's contact.
const WIPER: f64 = 1.0;
const R3: f64 = 2e3;
const R11: f64 = 15e3;
const C7: f64 = 82e-9;
const R12: f64 = 422e3;
const C8: f64 = 390e-12;
/// The clipping amplifier's swing either side of the bias.
const AMP_SWING: f64 = 4.5;
const RAIL_KNEE: f64 = 0.5;
const R20: f64 = 392e3;
const C13: f64 = 820e-12;
/// The summing amplifier's swing above and below the bias.
const SUM_ABOVE: f64 = 11.7;
const SUM_BELOW: f64 = 13.1;
const GERMANIUM: Diode = Diode::new(2.0e-6, 1.5);
/// The diode node is solved to this, in volts.
const CLIP_TOLERANCE: f64 = 1e-13;
/// Steps closer than this are evaluated directly, not by antiderivative.
const TINY_STEP: f64 = 1e-6;
const TREBLE_HZ: f64 = 408.0;
const R21: f64 = 1.8e3;
const R23: f64 = 4.7e3;
const TREBLE_POT: f64 = 10e3;

/// The front network's nodes: after C3, the clipping amplifier's input,
/// and the first feed-forward's capacitor.
const FRONT_SPLIT: usize = 0;
const AMP_IN: usize = 1;
const LOW_END: usize = 2;
/// The front network's gain pot section, by part index.
const FRONT_POT: usize = 3;
/// The front network, driven by the buffer (source 0).
const FRONT: [Part; 7] = [
    Part::capacitor(End::Source(0), End::Node(FRONT_SPLIT), 100e-9),
    Part::resistor(End::Node(FRONT_SPLIT), End::Node(AMP_IN), 10e3),
    Part::capacitor(End::Node(FRONT_SPLIT), End::Node(AMP_IN), 68e-9),
    Part::resistor(End::Node(AMP_IN), End::Ground, GAIN_POT),
    Part::resistor(End::Node(FRONT_SPLIT), End::Node(LOW_END), 1.5e3),
    Part::capacitor(End::Node(LOW_END), End::Ground, 1e-6),
    Part::resistor(End::Node(LOW_END), End::Sum, 15e3),
];

/// The back network's nodes: between C9 and R13, the diodes, after C10,
/// between C11 and R15, the second gang's wiper, between R18 and C12,
/// after R5 and C4, and between C6 and R9.
const AFTER_C9: usize = 0;
const DIODES: usize = 1;
const AFTER_C10: usize = 2;
const AFTER_C11: usize = 3;
const WIPER_NODE: usize = 4;
const AFTER_R18: usize = 5;
const CLEAN: usize = 6;
const AFTER_C6: usize = 7;
/// The back network's gain pot sections, by part index.
const BACK_POT_UPPER: usize = 9;
const BACK_POT_LOWER: usize = 10;
/// The back network, driven by the buffer (source 0) and the clipping
/// amplifier (source 1).
const BACK: [Part; 16] = [
    Part::capacitor(End::Source(1), End::Node(AFTER_C9), 1e-6),
    Part::resistor(End::Node(AFTER_C9), End::Node(DIODES), 1e3),
    Part::capacitor(End::Node(DIODES), End::Node(AFTER_C10), 1e-6),
    Part::resistor(End::Node(AFTER_C10), End::Sum, 47e3),
    Part::capacitor(End::Node(AFTER_C10), End::Node(AFTER_C11), 2.2e-9),
    Part::resistor(End::Node(AFTER_C11), End::Node(WIPER_NODE), 22e3),
    Part::resistor(End::Node(WIPER_NODE), End::Sum, 27e3),
    Part::resistor(End::Node(WIPER_NODE), End::Node(AFTER_R18), 12e3),
    Part::capacitor(End::Node(AFTER_R18), End::Sum, 27e-9),
    Part::resistor(End::Node(CLEAN), End::Node(WIPER_NODE), GAIN_POT),
    Part::resistor(End::Node(WIPER_NODE), End::Ground, GAIN_POT),
    Part::resistor(End::Source(0), End::Node(CLEAN), 5.1e3),
    Part::capacitor(End::Source(0), End::Node(CLEAN), 68e-9),
    Part::resistor(End::Node(CLEAN), End::Ground, 1.5e3),
    Part::capacitor(End::Node(CLEAN), End::Node(AFTER_C6), 390e-9),
    Part::resistor(End::Node(AFTER_C6), End::Ground, 1e3),
];

/// The treble stage's gain above its turnover at pot position `treble`.
fn treble_gain(treble: f64) -> f64 {
    TREBLE_POT.mul_add(treble, R23) / TREBLE_POT.mul_add(1.0 - treble, R21)
}

/// The clipper's antiderivative, `∫V dx` for the diode voltage `V` over
/// the open voltage `x = V + R I(V)`, at diode voltage `volts` behind
/// `reach` ohms: `V²/2 + R (V I(V) - ∫I dV)`.
fn clipper_integral(volts: f64, reach: f64) -> f64 {
    let (current, _) = GERMANIUM.pair(volts);
    (0.5 * volts).mul_add(
        volts,
        reach * volts.mul_add(current, -GERMANIUM.pair_integral(volts)),
    )
}

/// A soft limit at `above` over zero and `below` under it.
fn swing(x: f64, above: f64, below: f64, knee: f64) -> f64 {
    if x >= 0.0 {
        rail(x, above, knee)
    } else {
        rail(x, below, knee)
    }
}

/// One channel of the pedal at the oversampled rate.
#[derive(Debug, Clone, Copy)]
struct Circuit {
    coupling: Iir3,
    front: Network<3, 7, 1>,
    amp: Iir3,
    back: Network<8, 16, 2>,
    summing: Iir3,
    treble: Iir3,
    /// The back network's impedance at the diodes: the Thévenin resistance
    /// they see.
    reach: f64,
    /// How much current into the summing node each amp the diodes draw
    /// takes away.
    pull: f64,
    /// The diode node's last voltage, and the one before: the next solve
    /// starts from their straight-line extrapolation.
    diode: f64,
    before: f64,
    /// The diode node's last open voltage.
    last_open: f64,
    /// The clipper's antiderivative at the last open voltage.
    last_integral: f64,
    /// The last current into the summing node with the diodes drawing
    /// nothing.
    last_linear: f64,
}

impl Default for Circuit {
    fn default() -> Self {
        let c = 2.0 * f64::from(TARGET_RATE);
        let mut circuit = Self {
            coupling: Iir3::new(),
            front: Network::new(FRONT, c),
            amp: Iir3::new(),
            back: Network::new(BACK, c),
            summing: Iir3::new(),
            treble: Iir3::new(),
            reach: 0.0,
            pull: 0.0,
            diode: 0.0,
            before: 0.0,
            last_open: 0.0,
            last_integral: 0.0,
            last_linear: 0.0,
        };
        let gain = fraction(PARAMS[GAIN].default);
        let treble = fraction(PARAMS[TREBLE].default);
        circuit.design(gain, treble, c);
        circuit
    }
}

impl pair::Circuit for Circuit {
    fn reset(&mut self) {
        self.coupling.reset();
        self.front.reset();
        self.amp.reset();
        self.back.reset();
        self.summing.reset();
        self.treble.reset();
        self.diode = 0.0;
        self.before = 0.0;
        self.last_open = 0.0;
        self.last_integral = 0.0;
        self.last_linear = 0.0;
    }
}

impl Circuit {
    fn tick(&mut self, sample: f32, anti_alias: bool) -> f32 {
        let buffer = self.coupling.process(f64::from(sample) * INPUT_VOLTS);
        let front = self.front.open([buffer]);
        let low_end = self.front.sum_current(&front, [buffer]);
        self.front.settle(&front, [buffer]);
        let driven = rail(self.amp.process(front[AMP_IN]), AMP_SWING, RAIL_KNEE);
        let sources = [buffer, driven];
        let mut back = self.back.open(sources);
        let linear = low_end + self.back.sum_current(&back, sources);
        let open = back[DIODES];
        let drawn = self.clip(open, anti_alias);
        let exact = (open - self.diode) / self.reach;
        for (node, volt) in back.iter_mut().enumerate() {
            *volt = self.back.impedance(node, DIODES).mul_add(-exact, *volt);
        }
        self.back.settle(&back, sources);
        let current = if anti_alias {
            0.5f64.mul_add(linear + self.last_linear, -self.pull * drawn)
        } else {
            self.pull.mul_add(-drawn, linear)
        };
        self.last_linear = linear;
        let summed = swing(
            -R20 * self.summing.process(current),
            SUM_ABOVE,
            SUM_BELOW,
            RAIL_KNEE,
        );
        self.treble.process(-summed) as f32
    }

    /// Solve the diodes for the open voltage `open` at their node and
    /// return the current they draw: the exact one, or its mean over the
    /// step from the last open voltage to this one when anti-aliased. The
    /// exact voltage is kept in `diode` either way.
    fn clip(&mut self, open: f64, anti_alias: bool) -> f64 {
        let reach = self.reach;
        let previous = self.diode;
        let bound = open.abs();
        let guess = 2.0f64.mul_add(previous, -self.before);
        let volts = rising_root_within(-bound, bound, guess, CLIP_TOLERANCE, |v| {
            let (current, slope) = GERMANIUM.pair(v);
            (current + (v - open) / reach, slope + 1.0 / reach)
        });
        self.before = previous;
        self.diode = volts;
        if !anti_alias {
            return (open - volts) / reach;
        }
        let integral = clipper_integral(volts, reach);
        let step = open - self.last_open;
        let mean = if step.abs() > TINY_STEP {
            (integral - self.last_integral) / step
        } else {
            0.5 * (volts + previous)
        };
        let mean_open = 0.5 * (open + self.last_open);
        self.last_open = open;
        self.last_integral = integral;
        (mean_open - mean) / reach
    }

    fn design(&mut self, gain: f64, treble: f64, c: f64) {
        self.coupling
            .design(&Analogue::highpass1(TAU * COUPLING_HZ), c);
        self.front
            .set_resistance(FRONT_POT, (GAIN_POT * gain).max(WIPER));
        self.front.design(c);
        // H(s) = 1 + Zf / Zg over the common denominator
        // (1 + s R12 C8) (leg + R11 + s leg R11 C7).
        let leg = GAIN_POT.mul_add(1.0 - gain, R3);
        let flat = leg + R11;
        let first = (C8 * R12).mul_add(flat, C7 * leg * R11);
        let second = C7 * C8 * leg * R11 * R12;
        self.amp.design(
            &Analogue {
                b: [R12 + flat, (C7 * R11).mul_add(R12, first), second, 0.0],
                a: [flat, first, second, 0.0],
            },
            c,
        );
        self.back
            .set_resistance(BACK_POT_UPPER, (GAIN_POT * gain).max(WIPER));
        self.back
            .set_resistance(BACK_POT_LOWER, (GAIN_POT * (1.0 - gain)).max(WIPER));
        self.back.design(c);
        self.reach = self.back.impedance(DIODES, DIODES);
        self.pull = self.back.sum_pull(DIODES);
        // The gain pot moves the resistance the diodes see, and with it the
        // clipper's map from open voltage to diode voltage: restate the last
        // step's end on the new map, or the antiderivative's difference
        // across the step mixes two maps and spikes.
        let (current, _) = GERMANIUM.pair(self.diode);
        self.last_open = self.reach.mul_add(current, self.diode);
        self.last_integral = clipper_integral(self.diode, self.reach);
        self.summing
            .design(&Analogue::lowpass1(1.0 / (R20 * C13)), c);
        let lift = treble_gain(treble);
        let turnover = TAU * TREBLE_HZ;
        let (zero, pole) = if lift >= 1.0 {
            (turnover, turnover * lift)
        } else {
            (turnover / lift, turnover)
        };
        self.treble.design(
            &Analogue {
                b: [1.0, 1.0 / zero, 0.0, 0.0],
                a: [1.0, 1.0 / pole, 0.0, 0.0],
            },
            c,
        );
    }
}

/// A Klon-style overdrive. See the module documentation for the circuit.
#[derive(Debug)]
pub struct Klon {
    knobs: Knobs<3>,
    /// The gain and treble the filters were last designed for.
    retune: Retune<2>,
    pair: Pair<Circuit>,
}

impl Default for Klon {
    fn default() -> Self {
        Self::new()
    }
}

impl Klon {
    /// A Klon at its default settings, ready for 48 kHz until prepared.
    #[must_use]
    pub fn new() -> Self {
        Self::build(true)
    }

    /// The same circuit run at the base rate with a plain clipper, for the
    /// aliasing tests.
    #[cfg(test)]
    #[must_use]
    pub fn naive() -> Self {
        Self::build(false)
    }

    fn build(anti_alias: bool) -> Self {
        let mut klon = Self {
            knobs: Knobs::new(&PARAMS),
            retune: Retune::new([PARAMS[GAIN].default, PARAMS[TREBLE].default]),
            pair: Pair::new(anti_alias),
        };
        klon.prepare(DEFAULT_RATE);
        klon
    }

    fn design(&mut self, [gain, treble]: [f32; 2]) {
        let c = 2.0 * self.pair.rate();
        for circuit in self.pair.circuits() {
            circuit.design(fraction(gain), fraction(treble), c);
        }
    }

    /// Design for where the knobs are now, with nothing left waiting:
    /// after a prepare or a reset.
    fn retune_now(&mut self) {
        let knob = self.knobs.values();
        let now = [knob[GAIN], knob[TREBLE]];
        self.retune.settle(now);
        self.design(now);
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let knob = self.knobs.step();
        let tone = [knob[GAIN], knob[TREBLE]];
        if self.retune.due(tone) {
            self.design(tone);
        }
        let out_gain = db_gain(knob[LEVEL]) * OUTPUT_SCALE;
        let anti_alias = self.pair.anti_alias();
        self.pair
            .process([left, right], |_, circuit, x| circuit.tick(x, anti_alias))
            .map(|out| ceiling((f64::from(out) * out_gain) as f32))
    }
}

impl Effect for Klon {
    fn prepare(&mut self, sample_rate: f32) {
        let base = sane_rate(sample_rate);
        self.pair.prepare(base, TARGET_RATE, ADAA_DELAY);
        self.knobs.prepare(base);
        self.retune_now();
    }

    fn reset(&mut self) {
        self.knobs.settle();
        self.retune_now();
        self.pair.reset();
    }

    fn latency(&self) -> usize {
        self.pair.latency()
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
        Limits, aliasing_db, built, conformance, db, guitar, power_near, render_mono, rms, sine,
        spectrum,
    };
    use super::*;

    conformance!(
        KIND,
        Limits {
            worst_spur_db: Some(-80.0),
            hot: &[&[(GAIN, 100.0), (TREBLE, 100.0)]],
            hot_spur_db: Some(-48.0),
            unity: true,
            rough_glides: &[],
            latency_knobs: &[],
        }
    );

    #[test]
    fn oversampling_cuts_the_aliasing() {
        let naive = aliasing_db(&mut Klon::naive(), 4_999.0, 0.5);
        let proper = aliasing_db(&mut Klon::new(), 4_999.0, 0.5);
        assert!(
            proper < naive - 20.0,
            "naive {naive:.1} dB, oversampled {proper:.1} dB"
        );
        assert!(proper < -30.0, "oversampled {proper:.1} dB");
    }

    /// Quiet enough that the diodes are all but linear, the model's
    /// response is the netlist's: at no, half and full gain, with the treble
    /// at noon, the level at 100 Hz, 1 kHz and 5 kHz is within a quarter of
    /// a decibel of an AC solve of the same parts (both networks by complex
    /// nodal analysis, the diodes as their conductance at rest,
    /// `2 Is / n Vt`, the amplifiers, coupling and treble shelf by their
    /// transfer functions, the output scale applied), done apart from the
    /// model in double precision.
    #[test]
    fn the_small_signal_response_is_the_netlists() {
        let expected = [
            (0.0, [-5.40, -4.58, -6.34]),
            (50.0, [-1.57, 7.26, -6.92]),
            (100.0, [9.07, 29.29, 12.06]),
        ];
        for (gain, levels) in expected {
            // Full gain puts 60 times the input on the diodes.
            let amplitude = if gain > 75.0 { 1e-5 } else { 1e-4 };
            for (hz, want) in [100.0, 1_000.0, 5_000.0].into_iter().zip(levels) {
                let mut klon = built(&KIND);
                klon.set_param(GAIN, gain);
                klon.reset();
                let input = sine(hz, amplitude, 0.4);
                let out = render_mono(&mut *klon, &input);
                let got = db(rms(&out[9_600..]) / rms(&input[9_600..]));
                assert!(
                    (got - want).abs() < 0.25,
                    "gain {gain} at {hz} Hz: {got:.2} dB, the netlist {want:.2} dB"
                );
            }
        }
    }

    /// As in the pedal, the drive coming up outweighs the clean path going
    /// down: a guitar at -12 dBFS gets louder at every step of the gain
    /// knob, by more than 8 dB from none to full.
    #[test]
    fn more_gain_is_louder() {
        let played = guitar(1.0, 0.25);
        let levels: Vec<f64> = [0.0, 25.0, 50.0, 75.0, 100.0]
            .into_iter()
            .map(|gain| {
                let mut klon = built(&KIND);
                klon.set_param(GAIN, gain);
                klon.reset();
                db(rms(&render_mono(&mut *klon, &played)) / rms(&played))
            })
            .collect();
        assert!(
            levels.windows(2).all(|pair| pair[1] > pair[0]),
            "{levels:?}"
        );
        assert!(levels[4] - levels[0] > 8.0, "{levels:?}");
    }

    #[test]
    fn more_gain_is_more_distortion() {
        let input = sine(220.0, 0.03, 0.5);
        let harmonics = |gain: f32| {
            let mut klon = built(&KIND);
            klon.set_param(GAIN, gain);
            let power = spectrum(&render_mono(&mut *klon, &input));
            power_near(&power, 660.0) / power_near(&power, 220.0)
        };
        let low = harmonics(0.0);
        let high = harmonics(100.0);
        assert!(high > low * 100.0, "third harmonic {low} to {high}");
    }

    #[test]
    fn treble_tilts_the_top() {
        let input = sine(6_000.0, 0.01, 0.3);
        let at = |treble: f32| {
            let mut klon = built(&KIND);
            klon.set_param(TREBLE, treble);
            klon.set_param(GAIN, 0.0);
            rms(&render_mono(&mut *klon, &input)[4_800..])
        };
        let change = db(at(100.0) / at(0.0));
        assert!(change > 15.0, "{change:.1} dB");
    }

    #[test]
    fn level_is_decibels() {
        let input = sine(440.0, 0.05, 0.3);
        let at = |level: f32| {
            let mut klon = built(&KIND);
            klon.set_param(LEVEL, level);
            rms(&render_mono(&mut *klon, &input)[4_800..])
        };
        let change = db(at(-12.0) / at(0.0));
        assert!((change + 12.0).abs() < 0.1, "{change:.2} dB");
    }
}
