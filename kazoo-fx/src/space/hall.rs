//! `hall`: a large, lush hall built on an eight-line feedback delay network.
//!
//! The model:
//!
//! 1. **Pre-delay**, per side.
//! 2. **Early reflections**: a tapped delay after the pre-delay, eight taps
//!    a side (each alternate tap reading the opposite side, so the first
//!    reflections come from all round), gently lowpassed. They go straight
//!    to the output: they tell the ear how big the room is before the tail
//!    arrives.
//! 3. **Input diffusion**: four Schroeder allpasses a side, set by the
//!    diffusion knob, which turn a click into a dense burst before it enters
//!    the tank.
//! 4. **The tank**: eight delay lines of mutually unrelated lengths (43 to
//!    97 ms at the default size), recirculating through a Householder
//!    matrix, `I - (2/8) 1 1ᵀ`. That matrix is orthogonal, so it moves energy
//!    between the lines without adding or losing any, and every line feeds
//!    every other, which builds echo density fast (Jot and Chaigne, *Digital
//!    delay networks for designing artificial reverberators*, AES 1991; the
//!    Householder choice after Smith and Rocchesso). Each line's length is
//!    swept a fraction of a millisecond by its own slow sine, so the modes
//!    never sit still and the tail stays smooth instead of ringing.
//! 5. **Decay by band**: each line carries a gain and two shelves (Jot's
//!    absorbent filters, here second order for a cleaner split between the
//!    bands). A line `d` seconds long must lose `60 d / RT60` dB per pass, so
//!    the gain is set from the mid decay knob and the shelves' gains from the
//!    ratio of the low and high decay knobs to it: below about 100 Hz and
//!    above about 8 kHz each band's decay time is its knob's. The shelves
//!    are matched designs (see `matched`), so the treble shelf keeps its
//!    shape right up to Nyquist. Where the three knobs are far apart the two
//!    shelves can overlap and lift the middle; the combined response is
//!    checked on a grid and scaled down wherever it would pass the loudest
//!    band's gain, so the loop can never gain energy.
//! 6. **Freeze**: the input is faded out of the tank, every line's loss is
//!    faded to exactly nothing (gain 1, shelves flat), and the sweeps stop,
//!    each line gliding to a whole number of samples so its read is exact.
//!    What is in the tank then circulates for ever through a lossless
//!    orthogonal loop: energy steady, never growing.
//!
//! Size and pre-delay stretch lines that are holding sound, which bends its
//! pitch, so both move no faster than 2% of real time: at most a third of a
//! semitone of bend, and never a click.

use crate::dsp::{Phasor, flush};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

use super::kit::{
    CONTROL, Knobs, MAX_SLEW, Ramp, Slew, TapAllpass, ceiling, clean, decay_gain, equal_power,
    frames, silence, sine, unchanged, usable_rate,
};
use super::line::{Kernel, Line, MIN_SINC_READ};
use super::matched::{self, Angle, Biquad, FirstOrder};

/// Tank line lengths at the default size, in milliseconds.
const LINES_MS: [f32; 8] = [43.7, 51.3, 57.9, 64.1, 71.3, 78.7, 86.9, 97.3];
/// Each line's sweep rate, hertz.
const SWEEP_HZ: [f32; 8] = [0.13, 0.19, 0.23, 0.29, 0.31, 0.37, 0.41, 0.47];
/// The deepest sweep, in milliseconds, at full `mod`.
const SWEEP_MS: f32 = 0.9;
/// Which input side feeds each line, and with what sign.
const FEED: [(usize, f32); 8] = [
    (0, 1.0),
    (1, 1.0),
    (0, -1.0),
    (1, 1.0),
    (0, 1.0),
    (1, -1.0),
    (0, -1.0),
    (1, -1.0),
];
/// Output weights: two rows of an 8 x 8 Hadamard matrix, one per side.
const OUT_LEFT: [f32; 8] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const OUT_RIGHT: [f32; 8] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
/// Early reflection taps a side: milliseconds after the pre-delay, gain,
/// and whether the tap reads the opposite side.
const EARLY: [[(f32, f32, bool); 8]; 2] = [
    [
        (4.3, 0.84, false),
        (9.7, -0.62, true),
        (14.1, 0.53, false),
        (19.9, 0.47, true),
        (26.3, -0.40, false),
        (33.1, 0.33, true),
        (41.9, -0.27, false),
        (53.3, 0.22, true),
    ],
    [
        (5.9, 0.80, false),
        (11.3, -0.60, true),
        (16.7, 0.55, false),
        (22.1, -0.44, true),
        (28.9, 0.38, false),
        (37.3, -0.31, true),
        (46.1, 0.25, false),
        (58.7, -0.20, true),
    ],
];
/// Input diffuser lengths a side, milliseconds.
const DIFFUSERS_MS: [[f32; 4]; 2] = [[4.771, 3.595, 12.73, 9.307], [4.919, 3.713, 12.21, 9.929]];
/// The shelves' turnover frequencies.
const LOW_TURNOVER: f64 = 250.0;
const HIGH_TURNOVER: f64 = 4_000.0;
/// Points (below Nyquist) at which the combined decay filter is checked.
const GRID: usize = 24;

const MAX_PREDELAY: f32 = 0.25;
const MAX_SIZE: f32 = 1.6;

const PREDELAY: usize = 0;
const SIZE: usize = 1;
const DECAY: usize = 2;
const LOW: usize = 3;
const HIGH: usize = 4;
const DIFFUSION: usize = 5;
const EARLY_LEVEL: usize = 6;
const MOD: usize = 7;
const FREEZE: usize = 8;
const MIX: usize = 9;

static PARAMS: [ParamSpec; 10] = [
    ParamSpec {
        name: "predelay",
        min: 0.0,
        max: MAX_PREDELAY,
        default: 0.025,
        unit: "s",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "size",
        min: 0.3,
        max: MAX_SIZE,
        default: 1.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "decay",
        min: 0.3,
        max: 30.0,
        default: 3.0,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "low",
        min: 0.3,
        max: 30.0,
        default: 3.6,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "high",
        min: 0.1,
        max: 30.0,
        default: 1.6,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "diffusion",
        min: 0.0,
        max: 1.0,
        default: 0.7,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "early",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mod",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "freeze",
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
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
];

/// Pre-delay and size move under a speed limit of their own instead.
const GLIDES: [f32; 10] = [0.0, 0.0, 0.05, 0.05, 0.05, 0.03, 0.03, 0.05, 0.0, 0.03];

/// The tail's output level: sits a few dB under the input on a sustained
/// sound at the default decay.
const TAIL_LEVEL: f32 = 0.36;

/// How long freeze takes to engage or let go.
const FREEZE_SECONDS: f32 = 0.08;

/// The hall's entry in the catalogue.
pub static KIND: EffectKind = EffectKind {
    id: "hall",
    name: "Hall reverb",
    description: "A big, lush hall: eight-line feedback network, early reflections, separate low, mid and high decay, and an endless freeze.",
    params: &PARAMS,
    build: || Box::new(Hall::new()),
};

/// One tank line with its sweep and its decay filters.
#[derive(Debug, Clone, Default)]
struct TankLine {
    delay: Line,
    sweep: Phasor,
    low: Biquad,
    high: Biquad,
    gain: f32,
}

/// The eight-line hall.
#[derive(Debug, Clone)]
pub struct Hall {
    rate: f32,
    prepared: bool,
    knobs: Knobs<10>,
    predelay: [Line; 2],
    predelay_time: Slew,
    diffusers: [[TapAllpass; 4]; 2],
    early_tone: [FirstOrder; 2],
    lines: [TankLine; 8],
    freeze: Ramp,
    /// The size the tank uses: follows the knob no faster than
    /// [`MAX_SLEW`] on the longest line (so a change bends the tail by at
    /// most a third of a semitone), and holds still while frozen (a moving
    /// line would lose the frozen tail's pitch).
    size: Slew,
    /// The knob values the decay filters were last designed for.
    designed: [f32; 5],
    grid: [Angle; GRID],
    countdown: usize,
}

impl Default for Hall {
    fn default() -> Self {
        Self::new()
    }
}

/// Householder reflection across the all-ones direction: `x - (2/N) Σx`.
fn householder(values: &mut [f32; 8]) {
    let sum: f32 = values.iter().sum();
    let shift = 0.25 * sum;
    for value in values.iter_mut() {
        *value -= shift;
    }
}

impl Hall {
    /// A hall at the default settings, unprepared.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 0.0,
            prepared: false,
            knobs: Knobs::new(&PARAMS),
            predelay: Default::default(),
            diffusers: Default::default(),
            predelay_time: Slew::new(MIN_SINC_READ, MAX_SLEW),
            early_tone: [FirstOrder::default(); 2],
            lines: Default::default(),
            freeze: Ramp::new(0.0),
            size: Slew::new(1.0, 0.0),
            designed: [f32::NAN; 5],
            grid: [Angle::new(0.0); GRID],
            countdown: 0,
        }
    }

    /// A line's length in samples at the tank's size, before sweeping.
    fn length(&self, line: usize) -> f32 {
        LINES_MS[line] * 0.001 * self.size.value() * self.rate
    }

    /// Design every line's gain and shelves for the knobs and the freeze.
    fn design(&mut self) {
        let wanted = [
            self.size.value(),
            self.knobs.get(DECAY),
            self.knobs.get(LOW),
            self.knobs.get(HIGH),
            self.freeze.value(),
        ];
        if unchanged(&wanted, &self.designed) {
            return;
        }
        self.designed = wanted;
        let [_, mid_rt, low_rt, high_rt, frozen] = wanted;
        let keep = 1.0 - frozen;
        let rate = f64::from(self.rate);
        let q = std::f64::consts::FRAC_1_SQRT_2;
        for index in 0..8 {
            let seconds = self.length(index) / self.rate;
            // Fading each loss out in the log domain lands on exactly 1.
            let loss = |rt: f32| decay_gain(seconds, rt).powf(keep);
            let (mid, low, high) = (loss(mid_rt), loss(low_rt), loss(high_rt));
            let decibels = |ratio: f32| 20.0 * f64::from(ratio).log10();
            let low_shelf = matched::low_shelf(LOW_TURNOVER, decibels(low / mid), q, rate);
            let high_shelf = matched::high_shelf(HIGH_TURNOVER, decibels(high / mid), q, rate);
            let loudest = f64::from(mid.max(low).max(high));
            let peak = self.grid.iter().fold(0.0f64, |most, &angle| {
                let both = low_shelf.magnitude_at(angle) * high_shelf.magnitude_at(angle);
                most.max(f64::from(mid) * both)
            });
            let trim = if peak > loudest { loudest / peak } else { 1.0 };
            let line = &mut self.lines[index];
            line.gain = (f64::from(mid) * trim) as f32;
            line.low.set(low_shelf);
            line.high.set(high_shelf);
        }
    }

    /// One stereo frame.
    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.knobs.step();
        let frozen = self.freeze.next();
        if frozen == 0.0 {
            self.size.set(self.knobs.get(SIZE));
            self.size.next();
        }
        if self.countdown == 0 {
            let on = self.knobs.target(FREEZE) >= 0.5;
            let target = if on { 1.0 } else { 0.0 };
            if (self.freeze.target() - target).abs() > f32::EPSILON {
                self.freeze.set(target, FREEZE_SECONDS * self.rate);
            }
            self.design();
            self.countdown = CONTROL;
        }
        self.countdown -= 1;
        let keep = 1.0 - frozen;

        let input = [clean(left), clean(right)];
        for (line, &sample) in self.predelay.iter_mut().zip(&input) {
            line.push(sample);
        }
        self.predelay_time
            .set((self.knobs.get(PREDELAY) * self.rate).max(MIN_SINC_READ));
        let predelay = self.predelay_time.next();
        let delayed = [
            self.predelay[0].read(predelay),
            self.predelay[1].read(predelay),
        ];

        // Early reflections.
        let ms = 0.001 * self.size.value() * self.rate;
        let mut early = [0.0f32; 2];
        for (side, taps) in EARLY.iter().enumerate() {
            let sum = taps.iter().fold(0.0, |sum, &(at, gain, across)| {
                let source = if across { 1 - side } else { side };
                gain.mul_add(self.predelay[source].read(at.mul_add(ms, predelay)), sum)
            });
            early[side] = self.early_tone[side].process(f64::from(0.65 * sum)) as f32;
        }

        // Diffusion into the tank.
        let spread = self.knobs.get(DIFFUSION);
        let mut diffused = delayed;
        for (side, sample) in diffused.iter_mut().enumerate() {
            for (stage, allpass) in self.diffusers[side].iter_mut().enumerate() {
                let gain = if stage < 2 { 0.7 } else { 0.6 } * spread;
                *sample =
                    allpass.process(*sample, DIFFUSERS_MS[side][stage] * 0.001 * self.rate, gain);
            }
        }

        // The tank.
        // A square law: the lower half of the knob is the subtle range.
        let amount = self.knobs.get(MOD);
        let depth = SWEEP_MS * 0.001 * self.rate * amount * amount * keep;
        let mut outs = [0.0f32; 8];
        for (index, out) in outs.iter_mut().enumerate() {
            let length = self.length(index);
            let line = &mut self.lines[index];
            let swept = depth.mul_add(sine(line.sweep.next(SWEEP_HZ[index], self.rate)), length);
            let delay = frozen.mul_add(length.round(), keep * swept);
            *out = line.delay.read(delay);
        }
        let mut feedback = outs;
        for (value, line) in feedback.iter_mut().zip(&mut self.lines) {
            // The shelves keep running while frozen (they are flat then) so
            // their state is current when the freeze lets go; fully frozen,
            // the loop bypasses them so it is exactly lossless.
            let shaped = line.high.process(line.low.process(f64::from(*value)));
            if frozen < 1.0 {
                *value = shaped as f32 * line.gain;
            }
        }
        householder(&mut feedback);
        let into = 0.5 * keep;
        for ((line, value), &(side, sign)) in self.lines.iter_mut().zip(feedback).zip(&FEED) {
            let mut value = (into * sign).mul_add(diffused[side], value);
            flush(&mut value);
            line.delay.push(value);
        }

        let tail_left = OUT_LEFT
            .iter()
            .zip(&outs)
            .fold(0.0, |sum, (w, o)| w.mul_add(*o, sum));
        let tail_right = OUT_RIGHT
            .iter()
            .zip(&outs)
            .fold(0.0, |sum, (w, o)| w.mul_add(*o, sum));
        let level = self.knobs.get(EARLY_LEVEL) * keep;
        let wet_left = level.mul_add(early[0], TAIL_LEVEL * tail_left);
        let wet_right = level.mul_add(early[1], TAIL_LEVEL * tail_right);
        let (dry, wet) = equal_power(self.knobs.get(MIX));
        (
            dry.mul_add(input[0], wet * ceiling(wet_left)),
            dry.mul_add(input[1], wet * ceiling(wet_right)),
        )
    }
}

impl Effect for Hall {
    fn prepare(&mut self, sample_rate: f32) {
        let Some(rate) = usable_rate(sample_rate) else {
            self.prepared = false;
            return;
        };
        self.rate = rate;
        let kernel = Kernel::new(rate);
        let ms = 0.001 * rate;
        let longest_early = EARLY
            .iter()
            .flatten()
            .fold(0.0f32, |most, &(at, _, _)| most.max(at));
        let predelay = (longest_early * MAX_SIZE).mul_add(ms, MAX_PREDELAY * rate) as usize + 8;
        self.predelay = [Line::new(predelay, &kernel), Line::new(predelay, &kernel)];
        self.diffusers = DIFFUSERS_MS
            .map(|side| side.map(|length| TapAllpass::new((length * ms) as usize + 8, &kernel)));
        let sweep = SWEEP_MS * ms;
        for (line, &length) in self.lines.iter_mut().zip(&LINES_MS) {
            line.delay = Line::new(length.mul_add(MAX_SIZE * ms, sweep) as usize + 8, &kernel);
        }
        let tone = matched::lowpass_first_order(7_000.0, f64::from(rate));
        for side in &mut self.early_tone {
            side.set(tone);
        }
        let nyquist = f64::from(rate) * 0.5;
        for (index, point) in self.grid.iter_mut().enumerate() {
            // Log-spaced from 10 Hz up to Nyquist itself.
            let hz = 10.0 * (nyquist / 10.0).powf(index as f64 / (GRID - 1) as f64);
            *point = Angle::new(std::f64::consts::TAU * hz / f64::from(rate));
        }
        self.knobs.prepare(rate, &GLIDES);
        let longest = LINES_MS.iter().fold(0.0f32, |most, &ms| most.max(ms));
        self.size.set_speed(MAX_SLEW / (longest * 0.001 * rate));
        self.size.snap(self.knobs.get(SIZE));
        self.predelay_time
            .snap((self.knobs.get(PREDELAY) * rate).max(MIN_SINC_READ));
        let frozen = if self.knobs.target(FREEZE) >= 0.5 {
            1.0
        } else {
            0.0
        };
        self.freeze.snap(frozen);
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for line in &mut self.predelay {
            line.clear();
        }
        for side in &mut self.diffusers {
            for allpass in side {
                allpass.clear();
            }
        }
        for tone in &mut self.early_tone {
            tone.reset();
        }
        for (index, line) in self.lines.iter_mut().enumerate() {
            line.delay.clear();
            line.low.reset();
            line.high.reset();
            line.sweep.set(index as f32 / 8.0);
        }
        self.designed = [f32::NAN; 5];
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
    use super::super::testing::{
        self, RATE, build, highpassed, impulse, lowpassed, noise, rms, rt60, run_mono, silence,
    };

    fn response(knobs: &[(&str, f32)], seconds: f32) -> Vec<f32> {
        let mut all = vec![("mix", 1.0), ("early", 0.0), ("predelay", 0.0)];
        all.extend_from_slice(knobs);
        let mut hall = build("hall", &all);
        let (left, right) = run_mono(hall.as_mut(), &impulse(seconds));
        left.iter().zip(&right).map(|(l, r)| l + r).collect()
    }

    #[test]
    fn the_hall_keeps_the_contract() {
        testing::contract("hall");
    }

    #[test]
    fn the_hall_is_the_same_at_every_rate_with_long_even_decays() {
        testing::rates_agree_with("hall", &[("decay", 5.0), ("low", 5.0), ("high", 5.0)]);
    }

    #[test]
    fn size_and_predelay_never_bend_the_tail_past_the_speed_limit() {
        use crate::Effect;
        let mut hall = super::Hall::new();
        hall.set_param(super::SIZE, 0.3);
        hall.set_param(super::PREDELAY, 0.0);
        hall.prepare(RATE);
        hall.set_param(super::SIZE, 1.6);
        hall.set_param(super::PREDELAY, 0.25);
        let input = [0.1f32];
        let (mut left, mut right) = ([0.0f32], [0.0f32]);
        let mut last = (hall.length(7), hall.predelay_time.value());
        for _ in 0..(14.0 * RATE) as usize {
            hall.process(&testing::CONTEXT, [&input, &input], [&mut left, &mut right]);
            let now = (hall.length(7), hall.predelay_time.value());
            assert!((now.0 - last.0).abs() <= super::MAX_SLEW * 1.05);
            assert!((now.1 - last.1).abs() <= super::MAX_SLEW * 1.05);
            last = now;
        }
        assert!(hall.size.settled(), "the size never arrived");
    }

    #[test]
    fn the_decay_knob_is_a_true_rt60() {
        for decay in [0.8, 2.0, 5.0] {
            let tail = response(
                &[("decay", decay), ("low", decay), ("high", decay)],
                decay.mul_add(1.3, 0.5),
            );
            let measured = rt60(&tail);
            assert!(
                (measured / decay - 1.0).abs() < 0.1,
                "decay {decay} s measured {measured} s"
            );
        }
    }

    #[test]
    fn each_band_decays_at_its_own_knob() {
        let tail = response(&[("decay", 2.0), ("low", 4.0), ("high", 0.8)], 6.0);
        let low = rt60(&lowpassed(&lowpassed(&tail, 80.0), 80.0));
        let high = rt60(&highpassed(&highpassed(&tail, 12_000.0), 12_000.0));
        assert!((low / 4.0 - 1.0).abs() < 0.2, "low band {low} s");
        assert!((high / 0.8 - 1.0).abs() < 0.25, "high band {high} s");
    }

    #[test]
    fn freeze_holds_the_tail_for_a_minute_without_growing() {
        let mut hall = build("hall", &[("mix", 1.0), ("early", 0.0)]);
        run_mono(hall.as_mut(), &noise(1.5, 0.5, 21));
        hall.set_param(super::FREEZE, 1.0);
        // Keep playing into it: a frozen hall ignores new input.
        let (left, right) = run_mono(hall.as_mut(), &noise(1.0, 0.5, 22));
        let reference = rms(&left[left.len() / 2..]) + rms(&right[right.len() / 2..]);
        let second = RATE as usize;
        let (left, right) = run_mono(hall.as_mut(), &silence(60.0));
        for (l, r) in left.chunks(second).zip(right.chunks(second)) {
            let level = rms(l) + rms(r);
            assert!(level <= reference * 1.02, "grew: {level} over {reference}");
            assert!(level >= reference * 0.9, "faded: {level} under {reference}");
        }
        // And thawing lets it die.
        hall.set_param(super::FREEZE, 0.0);
        let (left, _) = run_mono(hall.as_mut(), &silence(20.0));
        assert!(rms(&left[left.len() - second..]) < 1e-4 * reference);
    }
}
