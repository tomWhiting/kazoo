//! Ringmod: a ring modulator with its own carrier, as a mathematically
//! ideal multiplier or as the four-diode ring of the analogue originals.
//!
//! A ring modulator multiplies the input by a carrier. Every frequency in
//! the input comes out as a pair, the carrier plus it and the carrier minus
//! it, and the input itself disappears: bells, robots, Dalek voices.
//!
//! - **ideal** is the plain product, `out = carrier × input`.
//! - **diode** is the classic circuit: two transformers and a ring of four
//!   diodes. The carrier switches pairs of diodes on and off, so the input
//!   is chopped rather than smoothly multiplied, which adds sidebands
//!   around the carrier's odd harmonics; and where the input is louder than
//!   the carrier, or the carrier passes through zero, the diodes'
//!   soft knees let it bleed and distort. Following Parker, each diode is a
//!   smooth piecewise curve (off below 0.2 V, a quadratic knee to 0.4 V,
//!   then straight), and the ring's output is
//!   `D(c + x/2) - D(c - x/2) - D(-c + x/2) + D(-c - x/2)`.
//!
//! The carrier reaches the ring through a transformer, whose limited
//! bandwidth (a two-pole lowpass at three times the carrier, and never
//! below 9 kHz) rounds a square carrier's edges, so the diodes switch in a
//! few microseconds rather than instantly. The ideal multiplier hears the
//! same carrier, so switching flavour never combs.
//!
//! The carrier is a sine, a triangle or a square (band-limited with
//! polyBLEP) from 1 Hz, a tremolo-like wobble, up to 8 kHz. `spread` puts
//! the right channel's carrier up to half a cycle behind the left's for a
//! stereo swirl. The carrier's phase is kept in f64, so even a 1 Hz
//! carrier at 192 kHz stays in tune. Everything runs at an internal rate
//! of at least 176.4 kHz (4x at 48 kHz, as is at 192 kHz; lower only
//! below 11.025 kHz, where the 16x cap bites): the
//! sidebands of a high carrier and the diode ring's extra harmonics would
//! otherwise fold straight back down.
//!
//! Sources: J. Parker, "A simple digital model of the diode-based
//! ring-modulator" (DAFX 2011); V. Välimäki and A. Huovilainen,
//! "Antialiasing oscillators in subtractive synthesis" (IEEE SPM, 2007),
//! for polyBLEP.

use std::f32::consts::TAU;

use super::kit::{
    DEFAULT_RATE, Knobs, Retune, ceiling, for_each_frame, fraction, gain as db_gain, weight,
};
use super::oversample::{MAX_FACTOR, TARGET_RATE};
use super::pair::{self, Pair};
use crate::dsp::OnePole;
use crate::dsp::sane_rate;
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const FREQUENCY: usize = 0;
const SHAPE: usize = 1;
const FLAVOUR: usize = 2;
const SPREAD: usize = 3;
const MIX: usize = 4;
const LEVEL: usize = 5;

static PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "frequency",
        min: 1.0,
        max: 8_000.0,
        default: 440.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "shape",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["sine", "triangle", "square"],
        },
    },
    ParamSpec {
        name: "flavour",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["ideal", "diode"],
        },
    },
    ParamSpec {
        name: "spread",
        min: 0.0,
        max: 100.0,
        default: 0.0,
        unit: "%",
        curve: Curve::Linear,
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

/// The Ringmod's entry in the catalogue.
pub const KIND: EffectKind = EffectKind {
    id: "ringmod",
    name: "Ring modulator",
    description: "Multiplies the input by its own sine, triangle or square carrier, ideally or through a modelled four-diode ring.",
    params: &PARAMS,
    build: || Box::new(Ringmod::new()),
};

/// Where a diode starts to conduct, and where its knee ends, in volts.
const DIODE_ON: f32 = 0.2;
const DIODE_LINEAR: f32 = 0.4;

/// The carrier transformer's lowest bandwidth: it rounds the square's
/// edges before they reach the diodes, as the real windings do.
const TRANSFORMER_HZ: f32 = 9_000.0;
/// How far above the carrier the transformer's corner sits.
const TRANSFORMER_SPAN: f32 = 3.0;

/// One diode of the ring, after Parker.
fn diode(volts: f32) -> f32 {
    let knee = DIODE_LINEAR - DIODE_ON;
    if volts <= DIODE_ON {
        0.0
    } else if volts <= DIODE_LINEAR {
        let over = volts - DIODE_ON;
        over * over / (2.0 * knee)
    } else {
        volts - DIODE_LINEAR + knee / 2.0
    }
}

/// The diode ring's output for carrier `carrier` and input `input`.
fn ring(carrier: f32, input: f32) -> f32 {
    let half = input / 2.0;
    diode(carrier + half) - diode(carrier - half) - diode(-carrier + half) + diode(-carrier - half)
}

/// The polyBLEP correction for a step at phase 0, at phase `t` with phase
/// increment `dt`.
fn blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let x = t / dt;
        x.mul_add(-x, 2.0 * x) - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x.mul_add(x, 2.0 * x) + 1.0
    } else {
        0.0
    }
}

/// The carrier at phase `phase` with increment `dt`, blending the shapes
/// by `weights` (sine, triangle, square).
fn carrier(phase: f32, dt: f32, weights: [f32; 3]) -> f32 {
    let [sine, triangle, square] = weights;
    let mut out = 0.0;
    if sine > 0.0 {
        out = sine.mul_add((TAU * phase).sin(), out);
    }
    if triangle > 0.0 {
        let shifted = (phase + 0.25).fract();
        out = triangle.mul_add(4.0f32.mul_add(-(shifted - 0.5).abs(), 1.0), out);
    }
    if square > 0.0 {
        let naive = if phase < 0.5 { 1.0 } else { -1.0 };
        let edges = blep(phase, dt) - blep((phase + 0.5).fract(), dt);
        out = square.mul_add(naive + edges, out);
    }
    out
}

/// What the knobs mean this sample.
#[derive(Debug, Clone, Copy)]
struct Settings {
    hz: f32,
    rate: f32,
    shapes: [f32; 3],
    diode: f32,
    spread: f64,
    mix: f32,
}

/// One channel's carrier transformer.
#[derive(Debug, Clone, Copy, Default)]
struct Transformer {
    poles: [OnePole; 2],
}

impl pair::Circuit for Transformer {
    fn reset(&mut self) {
        for pole in &mut self.poles {
            pole.reset();
        }
    }
}

/// A ring modulator. See the module documentation for the circuit.
#[derive(Debug)]
pub struct Ringmod {
    knobs: Knobs<6>,
    /// The carrier's phase, 0 up to 1, kept in f64 so it stays in tune.
    phase: f64,
    /// The carrier frequency the transformer was last set for.
    retune: Retune<1>,
    pair: Pair<Transformer>,
}

impl Default for Ringmod {
    fn default() -> Self {
        Self::new()
    }
}

impl Ringmod {
    /// A ring modulator at its default settings, ready for 48 kHz until
    /// prepared.
    #[must_use]
    pub fn new() -> Self {
        Self::build(true)
    }

    /// The same modulator run at the base rate, for the aliasing tests.
    #[cfg(test)]
    #[must_use]
    pub fn naive() -> Self {
        Self::build(false)
    }

    fn build(anti_alias: bool) -> Self {
        let mut ringmod = Self {
            knobs: Knobs::new(&PARAMS),
            phase: 0.0,
            retune: Retune::new([PARAMS[FREQUENCY].default]),
            pair: Pair::new(anti_alias),
        };
        ringmod.prepare(DEFAULT_RATE);
        ringmod
    }

    fn tune(&mut self, [hz]: [f32; 1]) {
        let rate = self.pair.rate() as f32;
        let corner = TRANSFORMER_HZ.max(TRANSFORMER_SPAN * hz);
        for transformer in self.pair.circuits() {
            for pole in &mut transformer.poles {
                pole.set_cutoff(corner, rate);
            }
        }
    }

    /// Design for where the knobs are now, with nothing left waiting:
    /// after a prepare or a reset.
    fn retune_now(&mut self) {
        let knob = self.knobs.values();
        let now = [knob[FREQUENCY]];
        self.retune.settle(now);
        self.tune(now);
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let knob = self.knobs.step();
        if self.retune.due([knob[FREQUENCY]]) {
            self.tune([knob[FREQUENCY]]);
        }
        let shape = knob[SHAPE];
        let rate = self.pair.rate();
        let settings = Settings {
            hz: knob[FREQUENCY],
            rate: rate as f32,
            shapes: [weight(shape, 0.0), weight(shape, 1.0), weight(shape, 2.0)],
            diode: weight(knob[FLAVOUR], 1.0),
            spread: fraction(knob[SPREAD]) * 0.5,
            mix: fraction(knob[MIX]) as f32,
        };
        let increment = f64::from(settings.hz) / rate;
        let out_gain = db_gain(knob[LEVEL]) as f32;
        // Both channels read the same carrier: the left moves it on, the
        // right reads the same phases, offset by the spread.
        let mut phases = [0.0f64; MAX_FACTOR];
        let mut slots = [0usize; 2];
        let phase = &mut self.phase;
        self.pair
            .process([left, right], |index, transformer, x| {
                let slot = slots[index] % MAX_FACTOR;
                slots[index] += 1;
                let at = if index == 0 {
                    let now = *phase;
                    *phase = (*phase + increment).rem_euclid(1.0);
                    phases[slot] = now;
                    now
                } else {
                    (phases[slot] + settings.spread).rem_euclid(1.0)
                };
                modulate(x, at as f32, transformer, &settings)
            })
            .map(|out| ceiling(out * out_gain))
    }
}

/// One sample of input `x` through the modulator at carrier phase
/// `phase`.
fn modulate(x: f32, phase: f32, transformer: &mut Transformer, settings: &Settings) -> f32 {
    let dt = (settings.hz / settings.rate).clamp(1e-9, 0.5);
    let raw = carrier(phase, dt, settings.shapes);
    let rounded = transformer.poles[0].lowpass(raw);
    let c = transformer.poles[1].lowpass(rounded);
    let ideal = c * x;
    let wet = settings.diode.mul_add(ring(c, x) - ideal, ideal);
    settings.mix.mul_add(wet - x, x)
}

impl Effect for Ringmod {
    fn prepare(&mut self, sample_rate: f32) {
        let base = sane_rate(sample_rate);
        self.pair.prepare(base, TARGET_RATE, 0.0);
        self.knobs.prepare(base);
        self.retune_now();
        self.phase = 0.0;
    }

    fn reset(&mut self) {
        self.knobs.settle();
        self.retune_now();
        self.phase = 0.0;
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
        Limits, RATE, built, conformance, power_near, render, render_mono, sine, spectrum,
    };
    use super::*;

    conformance!(
        KIND,
        Limits {
            worst_spur_db: None,
            hot: &[],
            hot_spur_db: None,
            unity: true,
            rough_glides: &[],
            latency_knobs: &[(MIX, 0.0)],
        }
    );

    #[test]
    fn an_ideal_ring_makes_sum_and_difference() {
        let mut ringmod = built(&KIND);
        ringmod.set_param(FLAVOUR, 0.0);
        ringmod.set_param(FREQUENCY, 1_000.0);
        let power = spectrum(&render_mono(&mut *ringmod, &sine(300.0, 0.5, 0.5)));
        let sidebands = power_near(&power, 700.0) + power_near(&power, 1_300.0);
        let input = power_near(&power, 300.0);
        assert!(10.0 * (sidebands / input).log10() > 60.0);
    }

    #[test]
    fn the_diode_ring_behaves_like_a_switch() {
        assert!((ring(1.0, 0.2) - 0.2).abs() < 1e-6);
        assert!((ring(-1.0, 0.2) + 0.2).abs() < 1e-6);
        assert!(ring(0.0, 0.1).abs() < 1e-6);
        assert!(ring(0.7, 0.0).abs() < 1e-6);
    }

    /// Power below 20 kHz that is not a true product `|k fc ± f|` of the
    /// carrier's harmonics and the input, as decibels below all of it,
    /// for carrier `shape` and `flavour`.
    fn stray_with(mut ringmod: Ringmod, shape: f32, flavour: f32) -> f64 {
        ringmod.set_param(FLAVOUR, flavour);
        let bins = 8_192.0;
        let exact = |bin: f64| bin * f64::from(RATE) / bins;
        let (fc, f) = (exact(513.0), exact(211.0));
        ringmod.set_param(FREQUENCY, fc as f32);
        ringmod.set_param(SHAPE, shape);
        ringmod.reset();
        let power = spectrum(&render_mono(&mut ringmod, &sine(f, 0.2, 0.5)));
        let audible = (20_000.0 * bins / f64::from(RATE)) as usize;
        let mut true_product = vec![false; audible];
        // A balanced diode ring distorts the input too, where the carrier
        // crosses the diodes' knees (longest for a slow triangle), so its
        // true products are every harmonic of the carrier plus or minus odd
        // multiples of the input; anything else is folded.
        for (k, m) in (0..64).flat_map(|k| [1.0f64, 3.0, 5.0, 7.0].map(|m| (k, m))) {
            let harmonic = f64::from(k) * fc;
            for product in [m.mul_add(f, harmonic), m.mul_add(-f, harmonic).abs()] {
                let bin = (product * bins / f64::from(RATE)).round() as usize;
                let from = bin.saturating_sub(3).min(audible);
                let to = (bin + 4).min(audible);
                true_product[from..to].fill(true);
            }
        }
        let total: f64 = power[4..audible].iter().sum();
        let stray: f64 = power[4..audible]
            .iter()
            .zip(&true_product[4..])
            .filter(|(_, is)| !**is)
            .map(|(p, _)| p)
            .sum();
        10.0 * (stray.max(1e-30) / total).log10()
    }

    #[test]
    fn oversampling_cuts_the_aliasing() {
        for (shape, flavour) in [(0.0, 1.0), (1.0, 1.0), (2.0, 0.0), (2.0, 1.0)] {
            let naive = stray_with(Ringmod::naive(), shape, flavour);
            let proper = stray_with(Ringmod::new(), shape, flavour);
            assert!(
                proper < naive - 12.0 && proper < -48.0,
                "shape {shape} flavour {flavour}: naive {naive:.1} dB, oversampled {proper:.1} dB"
            );
        }
    }

    #[test]
    fn spread_moves_the_right_carrier() {
        let input = sine(200.0, 0.3, 0.3);
        let mut ringmod = built(&KIND);
        ringmod.set_param(SPREAD, 100.0);
        ringmod.set_param(FREQUENCY, 50.0);
        let (left, right) = render(&mut *ringmod, &input, &input);
        let opposite: f64 = left[4_800..]
            .iter()
            .zip(&right[4_800..])
            .map(|(l, r)| f64::from(l + r).abs())
            .sum::<f64>();
        assert!(
            opposite
                < 0.2
                    * left[4_800..]
                        .iter()
                        .map(|s| f64::from(s.abs()))
                        .sum::<f64>()
        );
    }
}
