//! Screamer: a mid-hump overdrive after the Ibanez Tube Screamer (TS808).
//!
//! The Tube Screamer's clipping stage is a non-inverting op-amp with two
//! silicon diodes in its feedback loop. Its gain leg to ground is 4.7 kΩ in
//! series with 47 nF, a highpass at `1 / (2π 4.7k 47n)` = 720 Hz, so only
//! the mids and treble are amplified into the diodes; the bass passes at
//! unity, clean. That, and the lowpass after it, is the famous hump.
//!
//! The model runs at four times the family's usual internal rate, at
//! least 705.6 kHz (`INTERNAL_RATE`; lower only below 44.1 kHz, where the
//! 16x cap bites):
//!
//! 1. Input coupling (20 Hz).
//! 2. The clipping stage. The op-amp drives its inverting input towards
//!    the input voltage, so the gain leg (4.7 kΩ, 47 nF) draws current in
//!    proportion to the mids and treble. That current must all flow
//!    through the feedback network: 51 kΩ plus the 500 kΩ log drive pot,
//!    51 pF, and a pair of 1N914 diodes (Is = 2.52 nA, n = 1.752), all in
//!    parallel, whose voltage obeys
//!    `C4 dVf/dt = i - Vf / Rf - 2 Is sinh(Vf / n Vt)`.
//!    The op-amp is a JRC4558 with a 3 MHz gain-bandwidth, not an ideal
//!    one: its output integrates the difference between its inputs,
//!    `dVout/dt = wt (Vin - V-)`, which rounds the fastest edges as the
//!    real part does. Op-amp, leg capacitor and feedback network are all
//!    stepped together with the backward Euler rule (which stays stable
//!    however hard the diodes conduct) and solved each sample by bracketed
//!    Newton. The stage's output is close to `Vin + Vf`: the clean input
//!    plus the clipped mids, soft-limited by the op-amp's 4.5 V of
//!    headroom.
//! 3. The tone stage: the fixed 1 kΩ / 220 nF lowpass at 723 Hz, and the
//!    active tone control, whose pot slides a zero across it. With the
//!    second pole at 220 Ω / 220 nF (3.3 kHz) that gives
//!    `H(s) = (1 + s Tz) / ((1 + s T1)(1 + s T2))`, `Tz` running from 0
//!    (dark: two poles from 723 Hz) through `T1` at noon (flat to 3.3 kHz)
//!    to `2 T1` (up to 6 dB of bite above 723 Hz).
//! 4. The output level.
//!
//! Sources: Electrosmash, "Tube Screamer Analysis"; D. T. Yeh, "Digital
//! Implementation of Musical Distortion Circuits by Analysis and
//! Simulation" (Stanford, 2009), chapter 3.

use std::f64::consts::TAU;

use super::filter::{Analogue, Iir3};
use super::kit::{
    DEFAULT_RATE, Knobs, Retune, audio_taper, ceiling, flush64, for_each_frame, fraction,
    gain as db_gain, rail,
};
use super::oversample::TARGET_RATE;
use super::pair::{self, Pair};
use super::solve::{Diode, rising_root};
use crate::dsp::sane_rate;
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

/// The rate the clipping stage runs at: four times the family's usual
/// target. Its feedback voltage slews through zero in well under a
/// microsecond on a loud note, as the real circuit's does, and the
/// harmonics of edges that fast are still strong far past 176.4 kHz: run
/// there, a -6 dBFS tone at 4987 Hz (a 38.5th of 192 kHz) folds its 38th
/// and 39th harmonics onto half its own pitch at -51 dBc, which sounds like
/// period doubling. From 352.8 kHz the worst fold is still -65 dBc (the
/// 71st harmonic, at 44.1 kHz); from 705.6 kHz it is -93 dBc.
const INTERNAL_RATE: f32 = 4.0 * TARGET_RATE;

const DRIVE: usize = 0;
const TONE: usize = 1;
const LEVEL: usize = 2;

static PARAMS: [ParamSpec; 3] = [
    ParamSpec {
        name: "drive",
        min: 0.0,
        max: 100.0,
        default: 50.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "tone",
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

/// The Screamer's entry in the catalogue.
pub const KIND: EffectKind = EffectKind {
    id: "screamer",
    name: "Screamer overdrive",
    description: "A TS808-style overdrive: silicon diodes soft-clip the mids in an op-amp's feedback loop while the bass stays clean.",
    params: &PARAMS,
    build: || Box::new(Screamer::new()),
};

/// Volts at the pedal's input for a full-scale sample.
const INPUT_VOLTS: f64 = 1.0;
/// Full scale per volt at the output, set so the default settings come
/// out about as loud as they went in.
const OUTPUT_SCALE: f64 = 0.239;

const COUPLING_HZ: f64 = 20.0;
const R4: f64 = 4.7e3;
const C3: f64 = 47e-9;
const R6: f64 = 51e3;
const DRIVE_POT: f64 = 500e3;
const C4: f64 = 51e-12;
const SILICON: Diode = Diode::new(2.52e-9, 1.752);
/// The JRC4558's gain-bandwidth product.
const OPAMP_GBW: f64 = 3.0e6;
const HEADROOM: f64 = 4.5;
const HEADROOM_KNEE: f64 = 0.5;
/// R7 and C5: the fixed lowpass after the clipper.
const T1: f64 = 1.0e3 * 220e-9;
/// R8 and C6: the tone stage's own corner.
const T2: f64 = 220.0 * 220e-9;

/// The clipping stage's constants this sample.
#[derive(Debug, Clone, Copy)]
struct Stage {
    /// The sample period.
    period: f64,
    /// The sample period over C4.
    step: f64,
    /// The feedback resistance: R6 plus the drive pot.
    rf: f64,
    /// The gain leg's backward Euler conductance, `1 / (R4 + T / C3)`.
    leg: f64,
    /// The op-amp's gain-bandwidth over one sample, `T wt`.
    loop_gain: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Circuit {
    coupling: Iir3,
    tone: Iir3,
    /// The op-amp's output.
    out: f64,
    /// The voltage across C3, in the gain leg.
    leg: f64,
    /// The voltage across the feedback network.
    feedback: f64,
    /// Its voltage the sample before: with `feedback`, it extrapolates
    /// the next solve's starting point.
    previous: f64,
}

impl Circuit {
    /// One sample of the clipping stage, every part stepped with the
    /// backward Euler rule at period `T`:
    ///
    /// - the op-amp, `Vout = Vout' + T wt (Vin - V-)`, `V- = Vout - Vf`;
    /// - the gain leg, `i = (V- - Vc3') g` with `g = 1 / (R4 + T / C3)`;
    /// - the feedback network, `C4 (Vf - Vf') / T = i - Vf / Rf - I(Vf)`.
    ///
    /// The first two are linear in `Vf`, which leaves one rising equation
    /// in `Vf` for the solver.
    fn tick(&mut self, sample: f32, stage: &Stage) -> f32 {
        let input = self.coupling.process(f64::from(sample) * INPUT_VOLTS);
        let loop_gain = stage.loop_gain;
        // V- = base - Vf / (1 + T wt).
        let base = loop_gain.mul_add(input, self.out) / (1.0 + loop_gain);
        let step = stage.step;
        let pull = stage.leg / (1.0 + loop_gain);
        let target = (step * stage.leg).mul_add(base - self.leg, self.feedback);
        let linear = step.mul_add(1.0 / stage.rf + pull, 1.0);
        let bound = target.abs() / linear;
        let guess = 2.0f64.mul_add(self.feedback, -self.previous);
        let volts = rising_root(-bound, bound, guess, |v| {
            let (diode, slope) = SILICON.pair(v);
            (
                v.mul_add(linear, step.mul_add(diode, -target)),
                step.mul_add(slope, linear),
            )
        });
        self.previous = self.feedback;
        self.feedback = volts;
        self.out = loop_gain.mul_add(input + volts, self.out) / (1.0 + loop_gain);
        let current = stage.leg * (self.out - volts - self.leg);
        self.leg = stage.period.mul_add(current / C3, self.leg);
        for state in [
            &mut self.out,
            &mut self.leg,
            &mut self.feedback,
            &mut self.previous,
        ] {
            flush64(state);
        }
        let out = rail(self.out, HEADROOM, HEADROOM_KNEE);
        self.tone.process(out) as f32
    }

    fn design(&mut self, tone: f64, c: f64) {
        self.coupling
            .design(&Analogue::highpass1(TAU * COUPLING_HZ), c);
        let zero = 2.0 * tone * T1;
        self.tone.design(
            &Analogue {
                b: [1.0, zero, 0.0, 0.0],
                a: [1.0, T1 + T2, T1 * T2, 0.0],
            },
            c,
        );
    }
}

impl pair::Circuit for Circuit {
    fn reset(&mut self) {
        self.coupling.reset();
        self.tone.reset();
        self.out = 0.0;
        self.leg = 0.0;
        self.feedback = 0.0;
        self.previous = 0.0;
    }
}

/// A Tube Screamer-style overdrive. See the module documentation for the
/// circuit.
#[derive(Debug)]
pub struct Screamer {
    knobs: Knobs<3>,
    /// The tone the filters were last designed for.
    retune: Retune<1>,
    pair: Pair<Circuit>,
}

impl Default for Screamer {
    fn default() -> Self {
        Self::new()
    }
}

impl Screamer {
    /// A Screamer at its default settings, ready for 48 kHz until
    /// prepared.
    #[must_use]
    pub fn new() -> Self {
        Self::build(true)
    }

    /// The same circuit run at the base rate, for the aliasing tests.
    #[cfg(test)]
    #[must_use]
    pub fn naive() -> Self {
        Self::build(false)
    }

    fn build(anti_alias: bool) -> Self {
        let mut screamer = Self {
            knobs: Knobs::new(&PARAMS),
            retune: Retune::new([PARAMS[TONE].default]),
            pair: Pair::new(anti_alias),
        };
        screamer.prepare(DEFAULT_RATE);
        screamer
    }

    fn design(&mut self, [tone]: [f32; 1]) {
        let c = 2.0 * self.pair.rate();
        for circuit in self.pair.circuits() {
            circuit.design(fraction(tone), c);
        }
    }

    /// Design for where the knobs are now, with nothing left waiting:
    /// after a prepare or a reset.
    fn retune_now(&mut self) {
        let knob = self.knobs.values();
        let now = [knob[TONE]];
        self.retune.settle(now);
        self.design(now);
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let knob = self.knobs.step();
        if self.retune.due([knob[TONE]]) {
            self.design([knob[TONE]]);
        }
        let period = 1.0 / self.pair.rate();
        let stage = Stage {
            period,
            step: period / C4,
            rf: DRIVE_POT.mul_add(audio_taper(fraction(knob[DRIVE])), R6),
            leg: 1.0 / (R4 + period / C3),
            loop_gain: period * TAU * OPAMP_GBW,
        };
        let out_gain = db_gain(knob[LEVEL]) * OUTPUT_SCALE;
        self.pair
            .process([left, right], |_, circuit, x| circuit.tick(x, &stage))
            .map(|out| ceiling((f64::from(out) * out_gain) as f32))
    }
}

impl Effect for Screamer {
    fn prepare(&mut self, sample_rate: f32) {
        let base = sane_rate(sample_rate);
        self.pair.prepare(base, INTERNAL_RATE, 0.0);
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
    use super::super::oversample::factor_for;
    use super::super::testkit::{
        Limits, aliasing_db, built, conformance, db, power_near, render_mono, rms, sine, spectrum,
        subharmonic_db,
    };
    use super::*;

    conformance!(
        KIND,
        Limits {
            worst_spur_db: Some(-80.0),
            hot: &[&[(DRIVE, 100.0), (TONE, 100.0)]],
            hot_spur_db: Some(-48.0),
            unity: true,
            rough_glides: &[],
            latency_knobs: &[],
        }
    );

    #[test]
    fn oversampling_cuts_the_aliasing() {
        let naive = aliasing_db(&mut Screamer::naive(), 4_999.0, 0.5);
        let proper = aliasing_db(&mut Screamer::new(), 4_999.0, 0.5);
        assert!(
            proper < naive - 20.0,
            "naive {naive:.1} dB, oversampled {proper:.1} dB"
        );
        assert!(proper < -40.0, "oversampled {proper:.1} dB");
    }

    /// No period doubling at any host rate, pitched where it would show:
    /// what lands there is the clipping stage's own aliasing, held under
    /// the family's -80 dBc like the rest of its folds (see
    /// `INTERNAL_RATE`).
    #[test]
    fn no_subharmonics() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0, 192_000.0] {
            let internal = f64::from(rate) * factor_for(rate, INTERNAL_RATE) as f64;
            let level = subharmonic_db(&KIND, rate, internal, &[]);
            assert!(level < -80.0, "at {rate} Hz: {level:.1} dBc");
        }
    }

    /// Third harmonic over fundamental for a sine at `hz`.
    fn distortion(hz: f64) -> f64 {
        let mut screamer = built(&KIND);
        let power = spectrum(&render_mono(&mut *screamer, &sine(hz, 0.03, 0.5)));
        power_near(&power, 3.0 * hz) / power_near(&power, hz)
    }

    #[test]
    fn the_mids_clip_and_the_bass_stays_clean() {
        let bass = distortion(80.0);
        let mids = distortion(1_000.0);
        assert!(mids > bass * 10.0, "bass {bass}, mids {mids}");
    }

    #[test]
    fn tone_opens_the_top() {
        let input = sine(3_000.0, 0.005, 0.3);
        let at = |tone: f32| {
            let mut screamer = built(&KIND);
            screamer.set_param(TONE, tone);
            rms(&render_mono(&mut *screamer, &input)[4_800..])
        };
        let change = db(at(100.0) / at(0.0));
        assert!(change > 10.0, "{change:.1} dB");
    }
}
