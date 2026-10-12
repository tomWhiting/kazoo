//! Fuzz: a two-transistor fuzz after the Dallas Arbiter Fuzz Face, with an
//! Octavia-style rectifier for the octave up.
//!
//! The Fuzz Face is two common-emitter transistor stages in a row. A
//! transistor's collector current is exponential in its base voltage,
//! `Ic = Iq e^(v / Ve)`, so each stage's collector swings
//! `A (1 - e^(v / Ve))` around its resting voltage, where `A = Iq Rc` is
//! the drop across the collector resistor at rest. That swing is soft on
//! one side (the transistor turning off can only take the collector up to
//! the 9 V supply, `A` away) and hard on the other (it saturates about
//! 0.2 V above ground). That asymmetry is the Fuzz Face.
//!
//! The model runs at an internal rate of at least 352.8 kHz
//! (`INTERNAL_RATE`: 8x at 44.1 and 48 kHz, 4x at 88.2 and 96 kHz, 2x at
//! 176.4 and 192 kHz; lower only below 22.05 kHz, where the 16x cap bites):
//!
//! 1. The guitar's volume knob against the pedal's low input impedance.
//!    The Fuzz Face loads the guitar with only about 10 kΩ, so the
//!    guitar's 250 kΩ log volume pot (plus 6 kΩ of pickup) forms a divider
//!    with it that drops the level much faster than the pot alone: rolling
//!    the `input` knob back cleans the fuzz up, as it does on a real one.
//! 2. Input coupling: 2.2 µF into the first base (7 Hz).
//! 3. The octave, when switched in: a full-wave rectifier, as in the
//!    Octavia, folds the negative half up, doubling the pitch before the
//!    transistors. It is DC-blocked at 20 Hz and blended in.
//! 4. The first transistor: `A1` = 4.5 V at rest, with an effective `Ve1`
//!    of 1.5 V standing for the global feedback that tames its gain (-3).
//! 5. The feedback network's slow DC correction, a 5 Hz highpass: after a
//!    loud note the second stage's bias takes a moment to recover, which
//!    is the Fuzz Face's sputter.
//! 6. The second transistor, where the knobs live. `bias` sets its resting
//!    current (`A2` from 0.05 V, starved, to 8.3 V, saturating): starved,
//!    small signals barely register and big ones blast through, a gated,
//!    spitting fuzz. `fuzz` is the pot bypassing its 1 kΩ emitter
//!    resistor, which sets `Ve2 = Vt + Iq Re (1 - fuzz)`: fully up, the
//!    emitter is grounded and the gain is `A2 / Vt`: over 40 dB at the
//!    default bias, falling with it to 2x fully starved.
//! 7. The 10 nF output coupling into the 500 kΩ volume pot (32 Hz), and
//!    the output level.
//!
//! A fuzz is nearly a square wave, with harmonics far past any sensible
//! oversampling rate, so each transistor stage is also anti-aliased by its
//! antiderivative (first-order ADAA): its output is the stage's mean over
//! the step from the last input to this one, `(F(x) - F(x')) / (x - x')`.
//! Below the saturation corner `xs = Ve ln((9 V - 0.2 V) / A)` the stage is
//! `A (1 - e^(x/Ve))`, whose integral is `A (x - Ve e^(x/Ve))`; above it
//! the collector sits on its floor and the integral runs on in a straight
//! line.
//!
//! Each stage keeps its last antiderivative, so an ordinary step evaluates
//! only the new endpoint. The quotient divides differences of integrals
//! of order ten by steps as small as 1e-7 V. It needs f64's sixteen digits;
//! f32's seven would turn that cancellation into full scale noise. The
//! shared filters and the internal oversampling rate also stay unchanged.
//!
//! Sources: R. G. Keen, "The Technology of the Fuzz Face" (geofex.com);
//! Electrosmash, "Fuzz Face Analysis"; Ebers and Moll's transistor model.

use std::f64::consts::TAU;

use super::filter::{Analogue, Iir3};
use super::kit::{
    DEFAULT_RATE, Knobs, Retune, audio_taper, ceiling, for_each_frame, fraction, gain as db_gain,
    weight,
};
use super::oversample::TARGET_RATE;
use super::pair::{self, Pair};
use super::solve::{THERMAL_VOLTAGE, safe_exp};
use crate::dsp::sane_rate;
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

/// The rate the transistors run at: twice the family's usual target. Even
/// with the antiderivative, a fuzz's edges are sharp enough that from
/// 176.4 kHz a -6 dBFS tone near 5 kHz folds a spur back at -62 dBc
/// (-52 dBc with the fuzz and bias full up); from 352.8 kHz the worst is
/// -83 dBc (-65 dBc pushed), for twice the work.
const INTERNAL_RATE: f32 = 2.0 * TARGET_RATE;

/// How far the antiderivatives delay, in samples at the circuit's rate:
/// half a sample for each of the two transistors.
const ADAA_DELAY: f64 = 1.0;

const FUZZ: usize = 0;
const BIAS: usize = 1;
const INPUT: usize = 2;
const OCTAVE: usize = 3;
const LEVEL: usize = 4;

static PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "fuzz",
        min: 0.0,
        max: 100.0,
        default: 70.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "bias",
        min: 0.0,
        max: 100.0,
        default: 50.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "input",
        min: 0.0,
        max: 100.0,
        default: 100.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "octave",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["off", "on"],
        },
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

/// The Fuzz's entry in the catalogue.
pub const KIND: EffectKind = EffectKind {
    id: "fuzz",
    name: "Fuzz",
    description: "A Fuzz Face-style two-transistor fuzz that cleans up as the input comes down, with a starvable bias and an octave-up rectifier.",
    params: &PARAMS,
    build: || Box::new(Fuzz::new()),
};

/// Volts at the pedal's input for a full-scale sample.
const INPUT_VOLTS: f64 = 1.0;
/// Full scale per volt at the output, set so the default settings come
/// out about as loud as they went in.
const OUTPUT_SCALE: f64 = 0.0228;

const SUPPLY: f64 = 9.0;
const SATURATION: f64 = 0.2;
/// Steps closer than this are evaluated directly, not by antiderivative.
const TINY_STEP: f64 = 1e-7;
const A1: f64 = 4.5;
const VE1: f64 = 1.5;
const A2_STARVED: f64 = 0.05;
const A2_HOT: f64 = 8.3;
/// Q2's collector load (8.2 kΩ plus 470 Ω) and emitter resistor.
const RC2: f64 = 8.67e3;
const RE2: f64 = 1.0e3;
const VOLUME_POT: f64 = 250e3;
const PICKUP: f64 = 6.0e3;
const INPUT_IMPEDANCE: f64 = 10e3;
const INPUT_COUPLING_HZ: f64 = 7.0;
const OCTAVE_BLOCK_HZ: f64 = 20.0;
const OCTAVE_GAIN: f64 = 1.5;
const RECOVERY_HZ: f64 = 5.0;
const OUTPUT_COUPLING_HZ: f64 = 32.0;

/// The fraction of the pickup's voltage that reaches the pedal with the
/// guitar's volume at `travel`, relative to full volume.
fn guitar_volume(travel: f64) -> f64 {
    let divide = |travel: f64| {
        let lower = VOLUME_POT * audio_taper(travel);
        let upper = VOLUME_POT.mul_add(-audio_taper(travel), VOLUME_POT) + PICKUP;
        let loaded = lower * INPUT_IMPEDANCE / (lower + INPUT_IMPEDANCE).max(1e-9);
        loaded / (upper + loaded)
    };
    divide(travel) / divide(1.0)
}

/// One transistor stage: `rest` volts across the collector resistor at
/// rest and an effective thermal voltage `ve`.
#[derive(Debug, Clone, Copy)]
struct Transistor {
    rest: f64,
    ve: f64,
    /// Where the collector reaches saturation, in base volts.
    corner: f64,
    /// The collector's swing once saturated.
    floor: f64,
    /// The antiderivative at the corner.
    at_corner: f64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Memory {
    volts: f64,
    integral: f64,
}

#[derive(Debug, Clone, Copy, Default)]
enum Cache {
    #[default]
    On,
    #[cfg(test)]
    Off,
}

impl Transistor {
    fn new(rest: f64, ve: f64) -> Self {
        let corner = ve * ((SUPPLY - SATURATION) / rest).ln();
        Self {
            rest,
            ve,
            corner,
            floor: -(SUPPLY - rest - SATURATION),
            at_corner: rest * ve.mul_add(-safe_exp(corner / ve), corner),
        }
    }

    /// The collector's swing for base signal `volts`.
    fn swing(&self, volts: f64) -> f64 {
        if volts >= self.corner {
            self.floor
        } else {
            self.rest * (1.0 - safe_exp(volts / self.ve))
        }
    }

    /// The antiderivative of [`Self::swing`].
    fn integral(&self, volts: f64) -> f64 {
        if volts >= self.corner {
            self.floor.mul_add(volts - self.corner, self.at_corner)
        } else {
            self.rest * self.ve.mul_add(-safe_exp(volts / self.ve), volts)
        }
    }

    /// The stage's mean swing over the step from `last` to `volts`.
    fn smoothed(&self, volts: f64, last: Memory) -> (f64, Memory) {
        let integral = self.integral(volts);
        let step = volts - last.volts;
        let out = if step.abs() > TINY_STEP {
            (integral - last.integral) / step
        } else {
            self.swing(0.5 * (volts + last.volts))
        };
        (out, Memory { volts, integral })
    }

    #[cfg(test)]
    fn smoothed_recomputed(&self, volts: f64, last_volts: f64) -> f64 {
        let step = volts - last_volts;
        if step.abs() > TINY_STEP {
            (self.integral(volts) - self.integral(last_volts)) / step
        } else {
            self.swing(0.5 * (volts + last_volts))
        }
    }
}

/// What the knobs mean to the circuit this sample.
#[derive(Debug, Clone, Copy)]
struct Settings {
    input: f64,
    octave: f64,
    first: Transistor,
    second: Transistor,
    anti_alias: bool,
    cache: Cache,
}

fn second_transistor(fuzz: f32, bias: f32) -> Transistor {
    let rest = (A2_HOT - A2_STARVED).mul_add(fraction(bias), A2_STARVED);
    let degeneration = rest / RC2 * RE2 * (1.0 - fraction(fuzz));
    Transistor::new(rest, THERMAL_VOLTAGE + degeneration)
}

#[derive(Debug, Clone, Copy, Default)]
struct Circuit {
    coupling: Iir3,
    octave_block: Iir3,
    recovery: Iir3,
    output: Iir3,
    /// The last base voltage and its antiderivative for each stage.
    last: [Memory; 2],
    fresh: bool,
    octave_stale: bool,
}

impl pair::Circuit for Circuit {
    /// Zero in through every stage is zero out: the filters start at rest,
    /// each stage's swing and antiderivative memory are zero at zero, and a
    /// fresh stage's first step reads its antiderivative at zero either way.
    const QUIET_AT_REST: bool = true;

    fn reset(&mut self) {
        self.coupling.reset();
        self.octave_block.reset();
        self.recovery.reset();
        self.output.reset();
        self.last = [Memory::default(); 2];
        self.fresh = true;
        self.octave_stale = false;
    }
}

impl Circuit {
    /// Stage `index` (`transistor`) for base voltage `volts`.
    fn stage(
        &mut self,
        index: usize,
        transistor: &Transistor,
        volts: f64,
        anti_alias: bool,
        cache: Cache,
    ) -> f64 {
        if anti_alias {
            match cache {
                Cache::On => {
                    let (out, memory) = transistor.smoothed(volts, self.last[index]);
                    self.last[index] = memory;
                    out
                }
                #[cfg(test)]
                Cache::Off => {
                    let out = transistor.smoothed_recomputed(volts, self.last[index].volts);
                    self.last[index].volts = volts;
                    out
                }
            }
        } else {
            self.last[index].volts = volts;
            transistor.swing(volts)
        }
    }

    fn tick(&mut self, sample: f32, settings: &Settings, run_octave: bool) -> f32 {
        if self.fresh {
            if settings.anti_alias {
                self.last[0].integral = settings.first.integral(0.0);
                self.last[1].integral = settings.second.integral(0.0);
            }
            self.fresh = false;
        }
        let volts = self
            .coupling
            .process(f64::from(sample) * INPUT_VOLTS * settings.input);
        let base = if run_octave {
            if self.octave_stale {
                self.octave_block.reset();
                self.octave_stale = false;
            }
            let doubled = self.octave_block.process(volts.abs() * OCTAVE_GAIN);
            settings.octave.mul_add(doubled - volts, volts)
        } else {
            self.octave_stale = true;
            volts
        };
        let first = self.stage(
            0,
            &settings.first,
            base,
            settings.anti_alias,
            settings.cache,
        );
        let first = self.recovery.process(first);
        let second = self.stage(
            1,
            &settings.second,
            first,
            settings.anti_alias,
            settings.cache,
        );
        self.output.process(second) as f32
    }

    fn design(&mut self, c: f64) {
        self.coupling
            .design(&Analogue::highpass1(TAU * INPUT_COUPLING_HZ), c);
        self.octave_block
            .design(&Analogue::highpass1(TAU * OCTAVE_BLOCK_HZ), c);
        self.recovery
            .design(&Analogue::highpass1(TAU * RECOVERY_HZ), c);
        self.output
            .design(&Analogue::highpass1(TAU * OUTPUT_COUPLING_HZ), c);
    }
}

/// A Fuzz Face-style fuzz. See the module documentation for the circuit.
#[derive(Debug)]
pub struct Fuzz {
    knobs: Knobs<5>,
    /// The first transistor, whose operating point never changes.
    first: Transistor,
    second: Transistor,
    pair: Pair<Circuit>,
    retune: Retune<2>,
    cache: Cache,
}

impl Default for Fuzz {
    fn default() -> Self {
        Self::new()
    }
}

impl Fuzz {
    /// A Fuzz at its default settings, ready for 48 kHz until prepared.
    #[must_use]
    pub fn new() -> Self {
        Self::build(true)
    }

    /// The same model with every channel run even while it hears nothing,
    /// for the check that skipping idle channels changes nothing.
    #[cfg(test)]
    #[must_use]
    pub fn running_idle() -> Self {
        let mut model = Self::new();
        model.pair = model.pair.running_idle();
        model
    }

    /// The same circuit run at the base rate without its antiderivatives,
    /// for the aliasing tests.
    #[cfg(test)]
    #[must_use]
    pub fn naive() -> Self {
        Self::build(false)
    }

    /// The original antiderivatives and continuously running octave path.
    #[cfg(test)]
    #[must_use]
    pub fn reference() -> Self {
        let mut fuzz = Self::build(true);
        fuzz.cache = Cache::Off;
        fuzz
    }

    fn build(anti_alias: bool) -> Self {
        let mut fuzz = Self {
            knobs: Knobs::new(&PARAMS),
            first: Transistor::new(A1, VE1),
            second: second_transistor(PARAMS[FUZZ].default, PARAMS[BIAS].default),
            pair: Pair::new(anti_alias),
            retune: Retune::new([PARAMS[FUZZ].default, PARAMS[BIAS].default]),
            cache: Cache::On,
        };
        fuzz.prepare(DEFAULT_RATE);
        fuzz
    }

    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        let knob = self.knobs.step();
        if self.retune.due([knob[FUZZ], knob[BIAS]]) {
            self.second = second_transistor(knob[FUZZ], knob[BIAS]);
            // A moving operating point changes the integral at both ends.
            if self.pair.anti_alias() {
                for circuit in self.pair.circuits() {
                    circuit.last[1].integral = self.second.integral(circuit.last[1].volts);
                }
            }
        }
        let settings = Settings {
            input: guitar_volume(fraction(knob[INPUT])),
            octave: f64::from(weight(knob[OCTAVE], 1.0)),
            first: self.first,
            second: self.second,
            anti_alias: self.pair.anti_alias(),
            cache: self.cache,
        };
        let run_octave = match self.cache {
            Cache::On => settings.octave > 0.0,
            #[cfg(test)]
            Cache::Off => true,
        };
        let out_gain = db_gain(knob[LEVEL]) * OUTPUT_SCALE;
        self.pair
            .process([left, right], |_, circuit, x| {
                circuit.tick(x, &settings, run_octave)
            })
            .map(|out| ceiling((f64::from(out) * out_gain) as f32))
    }
}

impl Effect for Fuzz {
    fn prepare(&mut self, sample_rate: f32) {
        let base = sane_rate(sample_rate);
        let c = 2.0 * self.pair.prepare(base, INTERNAL_RATE, ADAA_DELAY);
        self.knobs.prepare(base);
        for circuit in self.pair.circuits() {
            circuit.design(c);
        }
    }

    fn reset(&mut self) {
        self.knobs.settle();
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
        Limits, RATE, aliasing_db, built, conformance, db, null_db, power_near, render_mono, rms,
        sine, spectrum,
    };
    use super::*;

    conformance!(
        KIND,
        Limits {
            worst_spur_db: Some(-70.0),
            hot: &[
                &[(FUZZ, 100.0), (BIAS, 100.0)],
                &[(FUZZ, 100.0), (BIAS, 100.0), (OCTAVE, 1.0)],
            ],
            hot_spur_db: Some(-48.0),
            unity: true,
            rough_glides: &[],
            latency_knobs: &[],
        }
    );

    #[test]
    fn caching_the_integral_changes_nothing() {
        for transistor in [
            Transistor::new(A1, VE1),
            Transistor::new(A2_HOT, THERMAL_VOLTAGE),
        ] {
            let mut memory = Memory {
                volts: 0.0,
                integral: transistor.integral(0.0),
            };
            let mut state = 0x5eed_090a_u32;
            for index in 0..200_000 {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let signed = 2.0 * f64::from(state) / f64::from(u32::MAX) - 1.0;
                // Pulled back towards zero so the walk stays in the bend
                // around the corner instead of drifting onto the straight
                // lines far from it.
                let volts = if index % 10 == 0 {
                    signed.mul_add(TINY_STEP * 0.5, memory.volts)
                } else {
                    0.9f64.mul_add(memory.volts, signed * 0.5)
                };
                let expected = transistor.smoothed_recomputed(volts, memory.volts);
                let (actual, next) = transistor.smoothed(volts, memory);
                assert_eq!(actual.to_bits(), expected.to_bits(), "step {index}");
                assert_eq!(next.volts.to_bits(), volts.to_bits());
                assert_eq!(
                    next.integral.to_bits(),
                    transistor.integral(volts).to_bits()
                );
                memory = next;
            }
        }
    }

    fn circuit_settings(octave: f64) -> Settings {
        Settings {
            input: 1.0,
            octave,
            first: Transistor::new(A1, VE1),
            second: Transistor::new(A2_HOT, THERMAL_VOLTAGE),
            anti_alias: true,
            cache: Cache::On,
        }
    }

    #[test]
    fn the_doubler_skipped_is_the_doubler_at_zero() {
        let mut running = Circuit::default();
        running.design(2.0 * 352_800.0);
        pair::Circuit::reset(&mut running);
        let mut skipped = running;
        let pristine = format!("{:?}", skipped.octave_block);
        let input = crate::bench::fixture(352_800.0);
        let settings = circuit_settings(0.0);
        for &sample in &input {
            assert_eq!(
                running.tick(sample, &settings, true).to_bits(),
                skipped.tick(sample, &settings, false).to_bits()
            );
        }
        assert_eq!(format!("{:?}", skipped.octave_block), pristine);
        assert!(skipped.octave_stale);
        // Restart both filters from the same discharged capacitor state.
        running.octave_stale = true;
        let settings = circuit_settings(1.0);
        for &sample in &input {
            assert_eq!(
                running.tick(sample, &settings, true).to_bits(),
                skipped.tick(sample, &settings, true).to_bits()
            );
        }
        assert!(!skipped.octave_stale);
    }

    #[test]
    fn the_octave_coming_back_does_not_click() {
        let mut fuzz = Fuzz::new();
        fuzz.prepare(RATE);
        let input = sine(110.0, 10.0f32.powf(-12.0 / 20.0), 2.03);
        let switch = 96_000;
        let before = render_mono(&mut fuzz, &input[..switch]);
        fuzz.set_param(OCTAVE, 1.0);
        let after = render_mono(&mut fuzz, &input[switch..]);
        let largest = |samples: &[f32]| {
            samples
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).abs())
                .fold(0.0f32, f32::max)
        };
        let resting = largest(&before[switch - 4_801..]);
        let during = largest(&after).max((after[0] - before[switch - 1]).abs());
        assert!(during <= 3.0 * resting, "step {during}, resting {resting}");
    }

    #[test]
    fn the_null_against_the_old_model_holds() {
        for knobs in [&[][..], &[(FUZZ, 100.0), (BIAS, 100.0), (OCTAVE, 1.0)][..]] {
            let null = null_db(&mut Fuzz::reference(), &mut Fuzz::new(), knobs);
            assert!(null < -120.0, "null {null} dB, knobs {knobs:?}");
        }
    }

    #[test]
    fn flat_out_square_wave_stays_bounded_and_the_tail_dies() {
        let mut fuzz = Fuzz::new();
        fuzz.prepare(RATE);
        for (index, value) in [(FUZZ, 100.0), (BIAS, 100.0), (LEVEL, PARAMS[LEVEL].max)] {
            fuzz.set_param(index, value);
        }
        fuzz.reset();
        let input: Vec<f32> = (0..48_000)
            .map(|index| if index % 960 < 480 { 16.0 } else { -16.0 })
            .collect();
        let loud = render_mono(&mut fuzz, &input);
        assert!(
            loud.iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 2.0)
        );
        let tail = render_mono(&mut fuzz, &vec![0.0; 48_000]);
        let residual = rms(&tail[43_200..]);
        assert!(residual < 1e-7, "tail RMS {residual}");
    }

    #[test]
    fn changing_the_transistor_refreshes_its_integral() {
        let mut reference = Fuzz::reference();
        let mut cached = Fuzz::new();
        let input = crate::bench::fixture(RATE);
        for block in input.chunks(256) {
            for &(index, value) in &[(FUZZ, 100.0), (BIAS, 0.0)] {
                reference.set_param(index, value);
                cached.set_param(index, value);
            }
            let old = render_mono(&mut reference, block);
            let new = render_mono(&mut cached, block);
            assert!(
                old.iter()
                    .zip(&new)
                    .all(|(a, b)| a.to_bits() == b.to_bits())
            );
        }
    }

    #[test]
    fn oversampling_cuts_the_aliasing() {
        let naive = aliasing_db(&mut Fuzz::naive(), 2_999.0, 0.1);
        let proper = aliasing_db(&mut Fuzz::new(), 2_999.0, 0.1);
        assert!(
            proper < naive - 15.0,
            "naive {naive:.1} dB, oversampled {proper:.1} dB"
        );
        assert!(proper < -60.0, "oversampled {proper:.1} dB");
    }

    /// Harmonic power over fundamental for a 200 Hz sine with the guitar
    /// volume at `input`.
    fn dirt(input: f32) -> f64 {
        let mut fuzz = built(&KIND);
        fuzz.set_param(INPUT, input);
        let power = spectrum(&render_mono(&mut *fuzz, &sine(200.0, 0.25, 0.5)));
        (power_near(&power, 400.0) + power_near(&power, 600.0)) / power_near(&power, 200.0)
    }

    #[test]
    fn rolling_the_input_back_cleans_it_up() {
        let full = dirt(100.0);
        let rolled = dirt(35.0);
        assert!(rolled < full / 10.0, "full {full}, rolled back {rolled}");
    }

    #[test]
    fn a_starved_bias_gates_quiet_notes() {
        let quiet = sine(200.0, 0.002, 0.5);
        let at = |bias: f32| {
            let mut fuzz = built(&KIND);
            fuzz.set_param(BIAS, bias);
            rms(&render_mono(&mut *fuzz, &quiet)[12_000..])
        };
        let gated = db(at(0.0) / at(50.0));
        assert!(gated < -15.0, "{gated:.1} dB");
    }

    #[test]
    fn the_octave_doubles_the_pitch() {
        let input = sine(200.0, 0.25, 0.5);
        let second = |octave: f32| {
            let mut fuzz = built(&KIND);
            fuzz.set_param(INPUT, 30.0);
            fuzz.set_param(FUZZ, 0.0);
            fuzz.set_param(OCTAVE, octave);
            let power = spectrum(&render_mono(&mut *fuzz, &input));
            power_near(&power, 400.0) / power_near(&power, 200.0)
        };
        let lift = 10.0 * (second(1.0) / second(0.0)).log10();
        assert!(lift > 20.0, "{lift:.1} dB");
    }

    /// A side that is silent until halfway is skipped while silent, and
    /// both sides come out bit for bit as they do with it running.
    #[test]
    fn an_idle_side_is_skipped_without_a_bit_of_difference() {
        let played = super::super::testkit::guitar(1.0, 0.5);
        let skipped = super::super::testkit::render_right_late(&mut Fuzz::new(), &played);
        let running = super::super::testkit::render_right_late(&mut Fuzz::running_idle(), &played);
        assert_eq!(skipped, running);
        assert!(skipped.iter().any(|[_, right]| *right != 0));
    }
}
