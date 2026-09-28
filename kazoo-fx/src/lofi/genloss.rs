//! Generation loss: a chain of dubs.
//!
//! # The model
//!
//! Copy a tape onto another machine, then copy the copy, and every pass
//! leaves its mark on top of the last. This runs up to eight simulated dubs
//! in a row, each its own machine with its own transport, and
//! `generations` picks which one you hear. Changing it crossfades. Only the
//! dubs up to the one you hear run; one woken by turning the knob up starts
//! on blank tape, so the crossfade brings it in from silence.
//!
//! Each dub, in the order the machine does it:
//!
//! 1. **Transport.** Its own wow (reel and hub rotation, about 0.5 to
//!    1.6 Hz) and flutter (roller and capstan, about 3.5 to 13 Hz), from
//!    [`Wobble`]s seeded apart so the machines drift independently and the
//!    errors pile up rather than cancel.
//! 2. **Record.** Treble emphasis, then the tape's anhysteretic curve
//!    ([`Magnetic::curve`], what well-biased tape records; oversampled
//!    twice), then the matching de-emphasis: loud treble squashes first.
//! 3. **Tape.** Its own hiss, and with `failure` its own dropouts (level and
//!    treble lost together) and ripples (the slow level flutter of creased
//!    tape).
//! 4. **Playback.** Gap and spacing loss at the model's tape speed
//!    ([`head_corner`]), its low-end roll-off and a small head bump. These
//!    compound: eight passes through a cassette's treble loss is far more
//!    than eight times the damage.
//!
//! `model` picks the machine: a cassette deck, a VHS linear track, a
//! studio reel at 15 ips or a dictation microcassette at 15/16 ips.
//!
//! Each dub delays the sound by its transport's centre delay (room for the
//! worst wow of any model) and its oversampler, about 6.6 ms, so eight
//! generations sit about 55 ms late; the dry signal for `mix` is delayed to
//! match.

use super::parts::{
    self, Biquad, Butterworth, CONTROL, Decibels, Dropouts, Head, Hiss, Magnetic, Oversampler2,
    Partial, Wobble,
};
use crate::dsp::{DelayLine, Noise, OnePole, Smoothed};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const MODEL: usize = 0;
const GENERATIONS: usize = 1;
const WOW: usize = 2;
const FLUTTER: usize = 3;
const SATURATE: usize = 4;
const HISS: usize = 5;
const FAILURE: usize = 6;
const MIX: usize = 7;
const OUTPUT: usize = 8;
const COUNT: usize = 9;

/// The most dubs in the chain.
const DUBS: usize = 8;

const fn percent(name: &'static str, default: f32) -> ParamSpec {
    ParamSpec {
        name,
        min: 0.0,
        max: 100.0,
        default,
        unit: "%",
        curve: Curve::Linear,
    }
}

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "model",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["cassette", "vhs", "reel", "microcassette"],
        },
    },
    ParamSpec {
        name: "generations",
        min: 1.0,
        max: 8.0,
        default: 3.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["1", "2", "3", "4", "5", "6", "7", "8"],
        },
    },
    percent("wow", 30.0),
    percent("flutter", 30.0),
    percent("saturate", 30.0),
    percent("hiss", 30.0),
    percent("failure", 10.0),
    percent("mix", 100.0),
    ParamSpec {
        name: "output",
        min: -18.0,
        max: 6.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
];

/// Generation loss.
pub static KIND: EffectKind = EffectKind {
    id: "genloss",
    name: "Generation loss",
    description: "A chain of up to eight simulated tape dubs (cassette, VHS, reel or \
                  microcassette), each adding its own wow, flutter, saturation, treble loss, \
                  hiss and failures.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(GenLoss::new())
}

/// What a machine model changes.
#[derive(Debug, Clone, Copy)]
struct Machine {
    /// Tape speed, metres per second.
    speed: f32,
    /// Playback gap and spacing, metres.
    gap: f32,
    spacing: f32,
    /// Where the low end rolls off, Hz.
    low: f32,
    /// Head bump: where and how much, Hz and dB.
    bump: f32,
    bump_db: f32,
    /// Record emphasis: where and how much, Hz and dB.
    turnover: f32,
    emphasis: f32,
    /// Hiss at full `hiss`, linear RMS per dub.
    hiss: f32,
    /// Peak speed errors at full `wow` and `flutter`.
    wow: f32,
    flutter: f32,
    /// How fast its parts turn relative to the table.
    turn: f32,
}

const MACHINES: [Machine; 4] = [
    // Cassette, 1⅞ ips.
    Machine {
        speed: 0.047_6,
        gap: 1.2e-6,
        spacing: 0.34e-6,
        low: 30.0,
        bump: 70.0,
        bump_db: 1.0,
        turnover: 2_652.0,
        emphasis: 12.0,
        hiss: 0.01,
        wow: 0.012,
        flutter: 0.005,
        turn: 1.0,
    },
    // VHS linear track, SP.
    Machine {
        speed: 0.033_35,
        gap: 1.0e-6,
        spacing: 0.25e-6,
        low: 90.0,
        bump: 60.0,
        bump_db: 0.0,
        turnover: 4_000.0,
        emphasis: 6.0,
        hiss: 0.016,
        wow: 0.012,
        flutter: 0.006,
        turn: 0.8,
    },
    // Studio reel, 15 ips.
    Machine {
        speed: 0.381,
        gap: 3.0e-6,
        spacing: 0.4e-6,
        low: 20.0,
        bump: 52.0,
        bump_db: 1.5,
        turnover: 9_094.0,
        emphasis: 8.0,
        hiss: 0.003,
        wow: 0.004,
        flutter: 0.003,
        turn: 2.0,
    },
    // Microcassette, 15/16 ips.
    Machine {
        speed: 0.023_8,
        gap: 1.5e-6,
        spacing: 0.5e-6,
        low: 200.0,
        bump: 110.0,
        bump_db: 0.0,
        turnover: 2_000.0,
        emphasis: 10.0,
        hiss: 0.025,
        wow: 0.025,
        flutter: 0.01,
        turn: 1.3,
    },
];

static WOW_PARTS: [Partial; 3] = [
    Partial {
        hz: 0.5,
        weight: 0.45,
        wander: 0.2,
        steady: 0.5,
    },
    Partial {
        hz: 0.9,
        weight: 0.3,
        wander: 0.2,
        steady: 0.4,
    },
    Partial {
        hz: 1.6,
        weight: 0.25,
        wander: 0.25,
        steady: 0.3,
    },
];

static FLUTTER_PARTS: [Partial; 3] = [
    Partial {
        hz: 3.5,
        weight: 0.3,
        wander: 0.1,
        steady: 0.5,
    },
    Partial {
        hz: 7.5,
        weight: 0.45,
        wander: 0.05,
        steady: 0.7,
    },
    Partial {
        hz: 13.0,
        weight: 0.25,
        wander: 0.15,
        steady: 0.3,
    },
];

/// The filters of one dub, for one channel.
#[derive(Debug, Clone)]
struct Pass {
    line: DelayLine,
    emphasis: Biquad,
    oversampler: Oversampler2,
    deemphasis: Biquad,
    hiss: Hiss,
    dropout: OnePole,
    head: Butterworth,
    bump: Biquad,
    low_cut: Biquad,
}

impl Pass {
    fn new(seed: u32) -> Self {
        Self {
            line: DelayLine::default(),
            emphasis: Biquad::new(),
            oversampler: Oversampler2::new(),
            deemphasis: Biquad::new(),
            hiss: Hiss::new(seed),
            dropout: OnePole::default(),
            head: Butterworth::default(),
            bump: Biquad::new(),
            low_cut: Biquad::new(),
        }
    }

    fn reset(&mut self) {
        self.line.clear();
        self.emphasis.reset();
        self.oversampler.reset();
        self.deemphasis.reset();
        self.hiss.reset();
        self.dropout.reset();
        self.head.reset();
        self.bump.reset();
        self.low_cut.reset();
    }

    fn design(&mut self, shape: &Shape, rate: f32) {
        self.emphasis
            .high_shelf(shape.turnover, shape.emphasis, rate);
        self.deemphasis
            .high_shelf(shape.turnover, -shape.emphasis, rate);
        self.head.lowpass(1, shape.corner, rate);
        self.bump.peak(shape.bump, 1.0, shape.bump_db, rate);
        self.low_cut.highpass(shape.low, 0.7, rate);
    }

    /// Dub `x`, read back at `delay` samples.
    fn dub(&mut self, x: f32, delay: f32, frame: Frame, lost: Lost) -> f32 {
        self.line.push(x);
        let played = self.line.read(delay);
        let boosted = self.emphasis.process(played);
        let drive = frame.drive;
        let recorded = self
            .oversampler
            .process(boosted, |v| Magnetic::curve(v, drive));
        let flat = self.deemphasis.process(recorded);
        let on_tape = self.hiss.next().mul_add(frame.hiss, flat * lost.ripple);
        let lifted = parts::lerp(on_tape, self.dropout.lowpass(on_tape), lost.loss);
        let read = self
            .head
            .process(lifted * (-0.8f32).mul_add(lost.loss, 1.0));
        self.low_cut.process(self.bump.process(read))
    }
}

/// The filter settings every dub shares.
#[derive(Debug, Clone, Copy)]
struct Shape {
    turnover: f32,
    emphasis: f32,
    corner: f32,
    bump: f32,
    bump_db: f32,
    low: f32,
}

/// What one sample shares between the dubs.
#[derive(Debug, Clone, Copy)]
struct Frame {
    drive: f32,
    hiss: f32,
}

/// What a dub's failures take this sample.
#[derive(Debug, Clone, Copy)]
struct Lost {
    loss: f32,
    ripple: f32,
}

/// One machine in the chain: its transport and failures, and a pass per
/// channel.
#[derive(Debug, Clone)]
struct Dub {
    passes: [Pass; 2],
    wow: Wobble,
    flutter: Wobble,
    dropouts: Dropouts,
    ripple: OnePole,
    ripple_noise: Noise,
    seed: u32,
}

impl Dub {
    fn new(index: usize) -> Self {
        let seed = 0x6E11_0000 + (index as u32) * 16;
        Self {
            passes: [Pass::new(seed + 1), Pass::new(seed + 2)],
            wow: Wobble::new(&WOW_PARTS, seed + 3),
            flutter: Wobble::new(&FLUTTER_PARTS, seed + 4),
            dropouts: Dropouts::new(seed + 5),
            ripple: OnePole::default(),
            ripple_noise: Noise::new(seed + 6),
            seed,
        }
    }

    fn reset(&mut self) {
        for pass in &mut self.passes {
            pass.reset();
        }
        self.wow.reset();
        self.flutter.reset();
        self.dropouts.reset();
        self.ripple.reset();
        self.ripple_noise = Noise::new(self.seed + 6);
    }
}

/// Generation loss. See the module documentation for the model.
#[derive(Debug, Clone)]
pub struct GenLoss {
    rate: f32,
    knobs: [Smoothed; COUNT],
    shape: [Smoothed; 6],
    taps: [Smoothed; DUBS],
    dubs: [Dub; DUBS],
    awake: [bool; DUBS],
    dry: [DelayLine; 2],
    base: f32,
    output_gain: Decibels,
    until_control: usize,
    designed: bool,
    frame: Frame,
}

/// Knobs that move filters glide at control rate; the rest every sample.
const fn at_control_rate(index: usize) -> bool {
    !matches!(index, MIX | OUTPUT)
}

impl GenLoss {
    /// A chain at 48 kHz with every knob at its default.
    #[must_use]
    pub fn new() -> Self {
        let mut chain = Self {
            rate: 48_000.0,
            knobs: PARAMS.map(|spec| Smoothed::new(spec.default)),
            shape: [Smoothed::new(0.0); 6],
            taps: [Smoothed::new(0.0); DUBS],
            dubs: std::array::from_fn(Dub::new),
            awake: [false; DUBS],
            dry: [DelayLine::default(), DelayLine::default()],
            base: 0.0,
            output_gain: Decibels::new(),
            until_control: 0,
            designed: false,
            frame: Frame {
                drive: 1.0,
                hiss: 0.0,
            },
        };
        chain.prepare(48_000.0);
        chain
    }

    fn machine(&self) -> Machine {
        MACHINES[(self.knobs[MODEL].target() as usize).min(3)]
    }

    /// The shape the chosen machine asks for (corner still in speed terms).
    fn aim(&mut self) {
        let machine = self.machine();
        let head = Head {
            speed: machine.speed,
            gap: machine.gap,
            skew: 0.0,
            spacing: machine.spacing,
        };
        let corner = parts::head_corner(head, 40_000.0);
        let targets = [
            machine.turnover,
            machine.emphasis,
            corner,
            machine.bump,
            machine.bump_db,
            machine.low,
        ];
        for (glide, target) in self.shape.iter_mut().zip(targets) {
            glide.set(target);
        }
        let chosen = self.knobs[GENERATIONS].target() as usize;
        for (n, tap) in self.taps.iter_mut().enumerate() {
            tap.set(if n + 1 == chosen { 1.0 } else { 0.0 });
        }
    }

    /// Every [`CONTROL`] samples: glide the slow knobs and, while the shape
    /// is moving, redesign the filters.
    fn control(&mut self) {
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            if at_control_rate(index) {
                knob.step();
            }
        }
        let moving = self.shape.iter().any(|glide| {
            (glide.value() - glide.target()).abs() > 1e-4f32.mul_add(glide.target().abs(), 1e-4)
        });
        for glide in &mut self.shape {
            glide.step();
        }
        if moving || !self.designed {
            self.designed = true;
            let rate = self.rate;
            let [turnover, emphasis, corner, bump, bump_db, low] = self.shape.map(|g| g.value());
            let shape = Shape {
                turnover,
                emphasis,
                corner: corner.min(rate * 0.45),
                bump,
                bump_db,
                low,
            };
            for dub in &mut self.dubs {
                for pass in &mut dub.passes {
                    pass.design(&shape, rate);
                }
            }
        }
        let machine = self.machine();
        let saturate = self.knobs[SATURATE].value() / 100.0;
        self.frame.drive = 3.2f32.mul_add(saturate, 0.8);
        let hiss = self.knobs[HISS].value() / 100.0;
        self.frame.hiss = machine.hiss * hiss * hiss.sqrt();
    }

    /// One stereo sample.
    fn tick(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.until_control == 0 {
            self.aim();
            self.control();
            self.until_control = CONTROL;
        }
        self.until_control -= 1;
        let rate = self.rate;
        let mix = self.knobs[MIX].step() / 100.0;
        let output = self.output_gain.gain(self.knobs[OUTPUT].step());
        let machine = self.machine();
        let wow = machine.wow * self.knobs[WOW].value() / 100.0;
        let flutter = machine.flutter * self.knobs[FLUTTER].value() / 100.0;
        let failure = self.knobs[FAILURE].value() / 100.0;
        let frame = self.frame;
        let base = self.base;
        let step = base.mul_add(rate, Oversampler2::LATENCY);

        for (line, input) in self.dry.iter_mut().zip([left, right]) {
            line.push(input);
        }
        let weights = self.taps.each_mut().map(Smoothed::step);
        let deepest = weights.iter().rposition(|w| *w > 1e-6).unwrap_or(0);
        let mut signal = [left, right];
        let mut wet = [0.0f32; 2];
        let mut dry = [0.0f32; 2];
        for (n, ((dub, awake), weight)) in self
            .dubs
            .iter_mut()
            .zip(&mut self.awake)
            .zip(weights)
            .enumerate()
            .take(deepest + 1)
        {
            if !*awake {
                dub.reset();
                *awake = true;
            }
            let swing = dub.wow.next(wow, machine.turn, 1.0, rate)
                + dub.flutter.next(flutter, machine.turn, 1.0, rate);
            let delay = (base + swing) * rate;
            let crease = dub.ripple.lowpass(dub.ripple_noise.sample() * 4.0);
            let lost = Lost {
                loss: dub.dropouts.next(4.0 * failure, 0.95, 0.08, rate),
                ripple: (0.3 * failure).mul_add(-crease.abs(), 1.0).max(0.2),
            };
            for (x, pass) in signal.iter_mut().zip(&mut dub.passes) {
                *x = pass.dub(*x, delay, frame, lost);
            }
            if weight > 1e-6 {
                let latency = step * (n + 1) as f32;
                for ((w, d), (x, line)) in wet
                    .iter_mut()
                    .zip(&mut dry)
                    .zip(signal.iter().zip(&self.dry))
                {
                    *w = x.mul_add(weight, *w);
                    *d = line.read(latency).mul_add(weight, *d);
                }
            }
        }
        for awake in self.awake.iter_mut().skip(deepest + 1) {
            *awake = false;
        }
        [0, 1]
            .map(|c| parts::lerp(dry[c], wet[c], mix) * output)
            .into()
    }
}

impl Default for GenLoss {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for GenLoss {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = parts::sane_rate(sample_rate);
        self.rate = rate;
        let control_rate = rate / CONTROL as f32;
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            if at_control_rate(index) {
                knob.set_time(0.05, control_rate);
            } else {
                knob.set_time(0.02, rate);
            }
        }
        for glide in &mut self.shape {
            glide.set_time(0.06, control_rate);
        }
        for tap in &mut self.taps {
            tap.set_time(0.03, rate);
        }
        let slowest = MACHINES.iter().map(|m| (m.wow, m.flutter, m.turn)).fold(
            0.0f32,
            |most, (wow, flutter, turn)| {
                let dub = &self.dubs[0];
                most.max(dub.wow.reach(wow, turn) + dub.flutter.reach(flutter, turn))
            },
        );
        self.base = slowest + 0.000_3 + 4.0 / rate;
        let longest = (2.0 * self.base * rate) as usize + 16;
        let dry_longest =
            (self.base.mul_add(rate, Oversampler2::LATENCY) * DUBS as f32) as usize + 16;
        for dub in &mut self.dubs {
            for pass in &mut dub.passes {
                pass.line.resize(longest);
                pass.hiss.band(60.0, rate * 0.45, rate);
                pass.dropout.set_cutoff(1_000.0, rate);
            }
            dub.ripple.set_cutoff(6.0, rate);
        }
        for line in &mut self.dry {
            line.resize(dry_longest);
        }
        self.designed = false;
        self.reset();
    }

    fn reset(&mut self) {
        for knob in &mut self.knobs {
            knob.snap(knob.target());
        }
        self.aim();
        for glide in self.shape.iter_mut().chain(&mut self.taps) {
            glide.snap(glide.target());
        }
        for dub in &mut self.dubs {
            dub.reset();
        }
        self.awake = [false; DUBS];
        for line in &mut self.dry {
            line.clear();
        }
        self.designed = false;
        self.until_control = 0;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(spec) = PARAMS.get(index) {
            if value.is_finite() {
                self.knobs[index].set(spec.clamp(value));
            }
        }
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        parts::run_block(input, output, |left, right| self.tick(left, right));
    }
}

#[cfg(test)]
mod tests {
    use super::super::parts::testkit::{
        self, CONTEXT, built, frequency_deviation, peak, render, rms, silence, sine,
    };
    use super::*;
    use crate::dsp::gain_to_db;

    #[test]
    fn it_keeps_the_effect_contract() {
        testkit::contract(&KIND);
    }

    #[test]
    fn silence_stays_silent_without_hiss_or_failure() {
        let input = silence(1.0);
        let mut chain = built(&KIND, &[(HISS, 0.0), (FAILURE, 0.0), (GENERATIONS, 8.0)]);
        let (left, right) = render(chain.as_mut(), &input, &input);
        assert!(peak(&left) < 1e-6 && peak(&right) < 1e-6);
    }

    #[test]
    fn hiss_builds_with_each_generation_and_stays_bounded() {
        let input = silence(1.0);
        let level = |model: f32, generations: f32| {
            let mut chain = built(
                &KIND,
                &[
                    (MODEL, model),
                    (GENERATIONS, generations),
                    (HISS, 100.0),
                    (FAILURE, 100.0),
                ],
            );
            let (left, right) = render(chain.as_mut(), &input, &input);
            assert!(peak(&left) < 0.3 && peak(&right) < 0.3);
            rms(&left[9_600..])
        };
        for model in 0..4 {
            let (one, eight) = (level(model as f32, 1.0), level(model as f32, 8.0));
            assert!(
                one > 1e-4 && eight > one * 1.5 && eight < 0.05,
                "{model}: {one} {eight}"
            );
        }
    }

    #[test]
    fn wow_tracks_its_knob() {
        let tone = sine(1_000.0, 0.25, 3.0);
        let deviation = |wow: f32| {
            let mut chain = built(
                &KIND,
                &[
                    (GENERATIONS, 1.0),
                    (WOW, wow),
                    (FLUTTER, 0.0),
                    (HISS, 0.0),
                    (FAILURE, 0.0),
                    (SATURATE, 0.0),
                ],
            );
            let (left, _) = render(chain.as_mut(), &tone, &tone);
            frequency_deviation(&left, 1_000.0, 0.5)
        };
        assert!(deviation(0.0) < 2e-4);
        let (half, full) = (deviation(50.0), deviation(100.0));
        let most = MACHINES[0].wow;
        assert!(full > most * 0.3 && full < most * 1.1, "{full}");
        assert!(half > full * 0.3 && half < full * 0.7, "{half} {full}");
    }

    #[test]
    fn each_generation_loses_more_treble() {
        let tone = sine(8_000.0, 0.1, 1.0);
        let level = |generations: f32| {
            let mut chain = built(
                &KIND,
                &[
                    (GENERATIONS, generations),
                    (WOW, 0.0),
                    (FLUTTER, 0.0),
                    (HISS, 0.0),
                    (FAILURE, 0.0),
                ],
            );
            let (left, _) = render(chain.as_mut(), &tone, &tone);
            gain_to_db(rms(&left[24_000..]))
        };
        let (one, four, eight) = (level(1.0), level(4.0), level(8.0));
        assert!(
            one > four + 3.0 && four > eight + 3.0,
            "{one} {four} {eight}"
        );
    }

    #[test]
    fn changing_generations_does_not_click() {
        let tone = sine(150.0, 0.3, 1.0);
        let mut chain = built(
            &KIND,
            &[(WOW, 0.0), (FLUTTER, 0.0), (HISS, 0.0), (FAILURE, 0.0)],
        );
        let mut out_l = vec![0.0; tone.len()];
        let mut out_r = vec![0.0; tone.len()];
        for (n, ((input, o_l), o_r)) in tone
            .chunks(480)
            .zip(out_l.chunks_mut(480))
            .zip(out_r.chunks_mut(480))
            .enumerate()
        {
            if n == 40 {
                chain.set_param(GENERATIONS, 8.0);
                chain.set_param(MODEL, 3.0);
            }
            chain.process(&CONTEXT, [input, input], [o_l, o_r]);
        }
        let jump = out_l
            .windows(2)
            .skip(9_600)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(jump < 0.03, "{jump}");
    }
}
