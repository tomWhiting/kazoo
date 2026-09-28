//! `shimmer`: a reverb whose tail climbs (or falls) in pitch as it rings.
//!
//! The sound Brian Eno and Daniel Lanois made famous by patching a pitch
//! shifter into the feedback of a reverb: every time the tail goes round,
//! part of it comes back an octave (or a fifth, or two octaves) higher, so a
//! held note blooms into a choir of its own overtones.
//!
//! The model is an eight-line feedback delay network like `hall`'s
//! (Householder matrix, per-line damping and a decay gain set from the RT60
//! knob), with the pitch shifter built into the lines themselves.
//!
//! **The shifter.** A delay line read at a steadily changing delay plays
//! back at a different pitch: a delay that shrinks by one sample every
//! sample plays an octave up. So each line is also read by two moving taps,
//! half a grain apart, whose delay sweeps through a grain and jumps back,
//! each fading in and out on a window that is silent at the jump. A plain
//! overlap-add shifter like that chops a steady note: the two taps read the
//! note a fixed distance apart, so depending on the note they sit in or out
//! of phase, and the output throbs at the grain rate (the "cheap pedal"
//! sound). Here every new grain is *aligned* before it starts, as in
//! Verhelst and Roelands' WSOLA (1993): its start is slid by up to 12 ms to
//! wherever the audio best matches what the other tap is playing, found by
//! cross-correlation (coarse, then refined to the sample). The two taps
//! then play in phase, and the crossfade between them is chosen from how
//! alike they are: equal-amplitude for matching audio, equal-power for
//! unrelated audio (a noisy tail), and the blend between, so the level
//! holds either way. Grains last as long as it takes the windows to repeat
//! ten times a second (100 ms an octave up, 25 ms for a fourth), long
//! enough to be smooth and short enough to track the tail. The eight
//! lines' grains are staggered so their seams never line up.
//!
//! **Aliasing.** Reading faster than real time raises every frequency in
//! the line, and anything that would land past Nyquist folds back as
//! inharmonic grit. So the shifter reads its own copy of each line, fed
//! through an eighth-order lowpass at 9.3 kHz over the shift ratio (4.65 kHz
//! for an octave up): nothing it raises can pass 9.3 kHz, which is safe
//! with more than 60 dB to spare down to 44.1 kHz, and the same at every
//! rate. The plain path keeps the tank's full band, to 18 kHz.
//!
//! **Changing the shift.** A new interval takes effect in three steps, so
//! it never clicks and never aliases: the shifted path fades out, the
//! shifter's band and grain change while it is silent and its copy of the
//! line refills with audio filtered for the new ratio, and it fades back in.
//!
//! **Decay.** With the shifter out, the decay knob is a true RT60. With it
//! in, each pass moves part of the tail up (or down), into the damping
//! lowpass (or out through the 40 Hz floor), so the more shimmer and the
//! darker the damping, the sooner the climbing voices fade: the classic
//! shimmer behaviour. Open the damping for long, rising tails.
//!
//! **Energy.** The shimmer knob crossfades each line between its plain and
//! its shifted read at equal power, and the decay gain is set from each
//! line's average delay (a shifted read is half a grain further back), so
//! each pass round the loop keeps the tail's energy. The tank runs at a
//! quarter of the signal's level and every line passes through the
//! family's soft ceiling before it is written back: it stays out of the way
//! even for a full-scale input, and it bounds the stored tail whatever the
//! knobs and the input do.
//!
//! **Size** stretches the lines, which bends what is in them, so it moves
//! no faster than 2% of real time: at most a third of a semitone of bend.

use crate::dsp::{OnePole, flush};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

use super::kit::{
    CONTROL, Knobs, MAX_SLEW, Ramp, Slew, TapAllpass, ceiling, clean, decay_gain, equal_power,
    frames, silence, usable_rate,
};
use super::line::{Kernel, Line};
use super::matched::{self, Biquad, FirstOrder, lowpass_first_order};

/// Tank line lengths at the default size, milliseconds.
const LINES_MS: [f32; 8] = [37.1, 41.9, 47.3, 53.9, 59.3, 66.7, 71.9, 79.1];
/// Which input side feeds each line, and its sign.
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
const OUT_LEFT: [f32; 8] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const OUT_RIGHT: [f32; 8] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];
/// Input diffuser lengths a side, milliseconds.
const DIFFUSERS_MS: [[f32; 4]; 2] = [[4.771, 3.595, 12.73, 9.307], [4.919, 3.713, 12.21, 9.929]];
/// The shift of each step of the shift knob, semitones.
const SHIFTS: [f32; 8] = [-24.0, -12.0, -5.0, 5.0, 7.0, 12.0, 19.0, 24.0];
const MAX_SIZE: f32 = 1.5;
/// How often the grain windows repeat, hertz.
const WINDOW_HZ: f32 = 10.0;
/// The longest grain: two octaves up, `|1 - 4| / WINDOW_HZ`.
const MAX_GRAIN: f32 = 0.3;
/// How far a new grain may slide to line up with the other, seconds each
/// way, and how much audio the alignment compares.
const SEARCH: f32 = 0.012;
const MATCH: f32 = 0.01;
/// The coarse alignment pass looks at the audio at about this rate.
const COARSE_RATE: f32 = 12_000.0;
/// The top of the tank's band.
const PLAIN_TOP: f32 = 18_000.0;
/// The top of the shifter's band, times the shift ratio, for a rising
/// shift: nothing it raises passes this, which clears Nyquist by more than
/// 60 dB from 44.1 kHz up.
const SHIFT_TOP: f32 = 9_300.0;
/// Q of the four sections of an eighth-order Butterworth lowpass.
const BUTTERWORTH_8: [f64; 4] = [0.509_795_6, 0.601_344_9, 0.899_976_3, 2.562_915_4];
/// How the tank's level sits against the signal's (so the in-loop ceiling
/// is never reached by ordinary material).
const STATE: f32 = 0.25;
/// The tail's output level: sustained noise comes back about as loud as
/// it went in, at the defaults.
const TAIL_LEVEL: f32 = 0.42;
/// How long the shifted path takes to fade out or in on a shift change.
const SWITCH_FADE: f32 = 0.03;

const SHIFT: usize = 0;
const AMOUNT: usize = 1;
const DECAY: usize = 2;
const DAMPING: usize = 3;
const SIZE: usize = 4;
const MIX: usize = 5;

static PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "shift",
        min: 0.0,
        max: 7.0,
        default: 5.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &[
                "-24 st", "-12 st", "-5 st", "+5 st", "+7 st", "+12 st", "+19 st", "+24 st",
            ],
        },
    },
    ParamSpec {
        name: "shimmer",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "decay",
        min: 0.5,
        max: 20.0,
        default: 6.0,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "damping",
        min: 1_000.0,
        max: 16_000.0,
        default: 6_000.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "size",
        min: 0.5,
        max: MAX_SIZE,
        default: 1.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.4,
        unit: "",
        curve: Curve::Linear,
    },
];
/// Glide times; size moves under a speed limit of its own.
const GLIDES: [f32; 6] = [0.0, 0.05, 0.05, 0.03, 0.0, 0.03];

/// The shimmer's entry in the catalogue.
pub static KIND: EffectKind = EffectKind {
    id: "shimmer",
    name: "Shimmer reverb",
    description: "A reverb with a pitch shifter in its feedback: the tail blooms up an octave (or a fifth, or down) every time it goes round.",
    params: &PARAMS,
    build: || Box::new(Shimmer::new()),
};

/// The shift ratio of knob step `step`.
fn ratio_of(step: usize) -> f32 {
    (SHIFTS[step.min(SHIFTS.len() - 1)] / 12.0).exp2()
}

/// The grain length in samples for `ratio` at `rate`: the windows repeat
/// [`WINDOW_HZ`] times a second.
fn grain_of(ratio: f32, rate: f32) -> f32 {
    ((ratio - 1.0).abs() * rate / WINDOW_HZ).max(4.0)
}

/// The top of the shifter's band for `ratio` at `rate`.
fn shift_band(ratio: f32, rate: f32) -> f32 {
    let top = if ratio > 1.0 {
        SHIFT_TOP / ratio
    } else {
        PLAIN_TOP
    };
    top.min(0.42 * rate)
}

/// The two taps' gains for tap A at window phase `phase`, when the two
/// taps' audio correlates by `alike` (0 to 1). Each tap's window is a sine
/// arch; the pair is scaled so its power is steady whether the taps play
/// the same audio (their amplitudes then add, and the gains sum to one) or
/// unrelated audio (their powers add, and the squares sum to one).
fn blend(phase: f32, alike: f32) -> (f32, f32) {
    let a = (std::f32::consts::PI * phase).sin().abs();
    let b = (std::f32::consts::PI * (phase + 0.5).fract()).sin().abs();
    let scale = 1.0 / (2.0 * alike.clamp(0.0, 1.0) * a).mul_add(b, 1.0).sqrt();
    (a * scale, b * scale)
}

/// A two-tap granular reader with aligned grains.
#[derive(Debug, Clone, Copy, Default)]
struct Shifter {
    /// Tap A's window phase, 0 up to 1; tap B is half a grain on.
    phase: f32,
    /// Each tap's alignment, in samples, fixed for its grain.
    slide: [f32; 2],
    /// How alike the two taps' audio was when the newer grain started.
    alike: f32,
    /// Whether the taps have been idle, and must be aligned afresh before
    /// they are heard.
    stale: bool,
}

/// What a shifter needs to know about its line each sample.
#[derive(Debug, Clone, Copy)]
struct Reading {
    /// The line's plain delay, samples.
    base: f32,
    /// The grain, samples.
    grain: f32,
    /// Window phase step per sample.
    step: f32,
    /// Alignment search, each way, and the stretch compared, samples.
    search: i32,
    span: usize,
    /// The coarse pass's stride, samples.
    stride: i32,
}

impl Shifter {
    const fn reset(&mut self, phase: f32) {
        self.phase = phase;
        self.slide = [0.0; 2];
        self.alike = 0.0;
        self.stale = true;
    }

    /// Keep time without reading, while the shifted path is silent.
    fn idle(&mut self, step: f32) {
        self.phase = (self.phase + step).rem_euclid(1.0);
        self.stale = true;
    }

    /// Tap `tap`'s delay now.
    fn delay(&self, tap: usize, reading: Reading) -> f32 {
        let phase = if tap == 0 {
            self.phase
        } else {
            (self.phase + 0.5).fract()
        };
        reading.grain.mul_add(phase, reading.base) + self.slide[tap]
    }

    /// One sample of shifted audio from `source`.
    fn read(&mut self, source: &Line, reading: Reading) -> f32 {
        if self.stale {
            self.slide = [0.0; 2];
            self.align(1, source, reading);
            self.stale = false;
        }
        let before = self.phase;
        let after = (before + reading.step).rem_euclid(1.0);
        self.phase = after;
        let wrapped = |from: f32, to: f32| {
            if reading.step < 0.0 {
                to > from
            } else if reading.step > 0.0 {
                to < from
            } else {
                false
            }
        };
        if wrapped(before, after) {
            self.align(0, source, reading);
        }
        if wrapped((before + 0.5).fract(), (after + 0.5).fract()) {
            self.align(1, source, reading);
        }
        let (a, b) = blend(self.phase, self.alike);
        a.mul_add(
            source.read(self.delay(0, reading)),
            b * source.read(self.delay(1, reading)),
        )
    }

    /// Slide tap `tap`'s new grain to where its audio best matches the other
    /// tap's, and note how alike they are.
    fn align(&mut self, tap: usize, source: &Line, reading: Reading) {
        self.slide[tap] = 0.0;
        let other = self.delay(1 - tap, reading).round() as usize;
        let start = self.delay(tap, reading).round() as usize;
        let search = reading.search;
        let score = |lag: i32, stride: usize| {
            let at = start.saturating_add_signed(lag as isize);
            let (mut both, mut mine, mut theirs) = (0.0f32, 0.0f32, 0.0f32);
            let mut i = 0;
            while i < reading.span {
                let (x, y) = (source.tap(at + i), source.tap(other + i));
                both = x.mul_add(y, both);
                mine = x.mul_add(x, mine);
                theirs = y.mul_add(y, theirs);
                i += stride;
            }
            (both, mine, theirs)
        };
        let stride = reading.stride.max(1);
        let step = stride.unsigned_abs() as usize;
        let mut best = (f32::MIN, 0i32);
        let mut lag = -search;
        while lag <= search {
            let (both, _, _) = score(lag, step);
            if both > best.0 {
                best = (both, lag);
            }
            lag += stride;
        }
        let coarse = best.1;
        best = (f32::MIN, coarse);
        for lag in (coarse - stride).max(-search)..=(coarse + stride).min(search) {
            let (both, _, _) = score(lag, 1);
            if both > best.0 {
                best = (both, lag);
            }
        }
        let (both, mine, theirs) = score(best.1, 1);
        let energy = (mine * theirs).sqrt();
        self.alike = if energy > 1e-12 && both.is_finite() {
            (both / energy).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.slide[tap] = best.1 as f32;
    }
}

/// One tank line with its shifter and filters.
#[derive(Debug, Clone, Default)]
struct TankLine {
    delay: Line,
    /// The same audio, band-limited for the shifter.
    source: Line,
    source_band: [Biquad; 4],
    band: Biquad,
    damping: FirstOrder,
    low_cut: OnePole,
    shifter: Shifter,
    gain: f32,
}

/// The shimmer reverb.
#[derive(Debug, Clone)]
pub struct Shimmer {
    rate: f32,
    prepared: bool,
    knobs: Knobs<6>,
    diffusers: [[TapAllpass; 4]; 2],
    lines: [TankLine; 8],
    size: Slew,
    /// The shift step in use, which lags the knob while a change fades
    /// through.
    active: usize,
    /// The shifted path's gate, faded down and up around a shift change.
    gate: Ramp,
    /// Samples left before the shifter's line holds only audio filtered
    /// for the new shift.
    refill: usize,
    countdown: usize,
}

impl Default for Shimmer {
    fn default() -> Self {
        Self::new()
    }
}

/// Householder reflection across the all-ones direction.
fn householder(values: &mut [f32; 8]) {
    let shift = 0.25 * values.iter().sum::<f32>();
    for value in values.iter_mut() {
        *value -= shift;
    }
}

impl Shimmer {
    /// A shimmer at the default settings, unprepared.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 0.0,
            prepared: false,
            knobs: Knobs::new(&PARAMS),
            diffusers: Default::default(),
            lines: Default::default(),
            size: Slew::new(1.0, 0.0),
            active: 5,
            gate: Ramp::new(1.0),
            refill: 0,
            countdown: 0,
        }
    }

    /// The shift step the knob asks for.
    fn wanted(&self) -> usize {
        (self.knobs.target(SHIFT).max(0.0) as usize).min(SHIFTS.len() - 1)
    }

    fn length(&self, line: usize) -> f32 {
        LINES_MS[line] * 0.001 * self.size.value() * self.rate
    }

    /// The longest delay the shifter ever reads, samples: what must pass
    /// through its line before it holds only fresh audio.
    fn longest_read(&self) -> usize {
        let ms = 0.001 * self.rate;
        (LINES_MS[7] * MAX_SIZE).mul_add(ms, (MAX_GRAIN + SEARCH + MATCH) * self.rate) as usize
    }

    /// Point every shifter's band at the active shift.
    fn design_band(&mut self) {
        let top = f64::from(shift_band(ratio_of(self.active), self.rate));
        let rate = f64::from(self.rate);
        let designs = BUTTERWORTH_8.map(|q| matched::lowpass(top, q, rate));
        for line in &mut self.lines {
            for (section, &design) in line.source_band.iter_mut().zip(&designs) {
                section.set(design);
            }
        }
    }

    /// Step the shift-change sequence: fade out, switch, refill, fade in.
    fn follow_shift(&mut self) {
        let fade = SWITCH_FADE * self.rate;
        let wanted = self.wanted();
        if wanted != self.active {
            if self.gate.value() > 0.0 {
                if self.gate.target() > 0.0 {
                    self.gate.set(0.0, fade);
                }
            } else {
                self.active = wanted;
                self.design_band();
                self.refill = self.longest_read();
            }
        } else if self.refill == 0 && self.gate.target() < 1.0 {
            self.gate.set(1.0, fade);
        }
    }

    fn update(&mut self) {
        self.follow_shift();
        let decay = self.knobs.get(DECAY);
        let grain = grain_of(ratio_of(self.active), self.rate);
        // The share of each line's energy that comes through the shifter...
        let through = equal_power(self.knobs.get(AMOUNT)).1 * self.gate.value();
        let share = through * through;
        let damping = lowpass_first_order(f64::from(self.knobs.get(DAMPING)), f64::from(self.rate));
        for index in 0..8 {
            // ...which reads on average half a grain further back.
            let average = share.mul_add(0.5 * grain, self.length(index));
            let rate = self.rate;
            let line = &mut self.lines[index];
            line.gain = decay_gain(average / rate, decay);
            line.damping.set(damping);
        }
    }

    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.knobs.step();
        self.size.set(self.knobs.get(SIZE));
        self.size.next();
        if self.countdown == 0 {
            self.update();
            self.countdown = CONTROL;
        }
        self.countdown -= 1;
        self.refill = self.refill.saturating_sub(1);
        let gate = self.gate.next();
        let input = [clean(left), clean(right)];
        let mut diffused = input;
        for (side, sample) in diffused.iter_mut().enumerate() {
            for (stage, allpass) in self.diffusers[side].iter_mut().enumerate() {
                let gain = if stage < 2 { 0.7 } else { 0.6 };
                *sample =
                    allpass.process(*sample, DIFFUSERS_MS[side][stage] * 0.001 * self.rate, gain);
            }
        }

        let ratio = ratio_of(self.active);
        let grain = grain_of(ratio, self.rate);
        let stride = (self.rate / COARSE_RATE).round().max(1.0) as i32;
        // The gate takes the shifted share down; the plain read takes up the
        // power it gives away, so the crossfade stays equal-power.
        let shift_in = equal_power(self.knobs.get(AMOUNT)).1 * gate;
        let keep = shift_in.mul_add(-shift_in, 1.0).max(0.0).sqrt();
        let mut outs = [0.0f32; 8];
        for (index, out) in outs.iter_mut().enumerate() {
            let reading = Reading {
                base: self.length(index),
                grain,
                step: (1.0 - ratio) / grain,
                search: (SEARCH * self.rate) as i32,
                span: (MATCH * self.rate) as usize,
                stride,
            };
            let line = &mut self.lines[index];
            let plain = line.delay.read(reading.base);
            let shifted = if shift_in > 0.0 {
                line.shifter.read(&line.source, reading)
            } else {
                line.shifter.idle(reading.step);
                0.0
            };
            *out = keep.mul_add(plain, shift_in * shifted);
        }
        let mut feedback = outs;
        for (value, line) in feedback.iter_mut().zip(&mut self.lines) {
            let damped = line.damping.process(f64::from(*value)) as f32;
            *value = line.low_cut.highpass(damped) * line.gain;
        }
        householder(&mut feedback);
        for ((line, value), &(side, sign)) in self.lines.iter_mut().zip(feedback).zip(&FEED) {
            let into = (0.5 * STATE * sign).mul_add(diffused[side], value);
            let mut written = line.band.process(f64::from(ceiling(into))) as f32;
            flush(&mut written);
            line.delay.push(written);
            let mut source = f64::from(written);
            for section in &mut line.source_band {
                source = section.process(source);
            }
            let mut source = source as f32;
            flush(&mut source);
            line.source.push(source);
        }

        let level = TAIL_LEVEL / STATE;
        let tail_left = OUT_LEFT
            .iter()
            .zip(&outs)
            .fold(0.0, |sum, (w, o)| w.mul_add(*o, sum));
        let tail_right = OUT_RIGHT
            .iter()
            .zip(&outs)
            .fold(0.0, |sum, (w, o)| w.mul_add(*o, sum));
        let (dry, wet) = equal_power(self.knobs.get(MIX));
        (
            dry.mul_add(input[0], wet * ceiling(level * tail_left)),
            dry.mul_add(input[1], wet * ceiling(level * tail_right)),
        )
    }
}

impl Effect for Shimmer {
    fn prepare(&mut self, sample_rate: f32) {
        let Some(rate) = usable_rate(sample_rate) else {
            self.prepared = false;
            return;
        };
        self.rate = rate;
        let kernel = Kernel::new(rate);
        let ms = 0.001 * rate;
        self.diffusers = DIFFUSERS_MS
            .map(|side| side.map(|length| TapAllpass::new((length * ms) as usize + 8, &kernel)));
        let plain = matched::lowpass(
            f64::from(PLAIN_TOP.min(0.42 * rate)),
            std::f64::consts::FRAC_1_SQRT_2,
            f64::from(rate),
        );
        let reach = (2.0f32.mul_add(SEARCH, MAX_GRAIN) + MATCH) * rate;
        for (line, &length) in self.lines.iter_mut().zip(&LINES_MS) {
            let longest = length * MAX_SIZE * ms;
            line.delay = Line::new(longest as usize + 8, &kernel);
            line.source = Line::new((longest + reach) as usize + 8, &kernel);
            line.low_cut.set_cutoff(40.0, rate);
            line.band.set(plain);
        }
        self.knobs.prepare(rate, &GLIDES);
        self.size.set_speed(MAX_SLEW / (LINES_MS[7] * ms));
        self.size.snap(self.knobs.get(SIZE));
        self.active = self.wanted();
        self.design_band();
        self.gate.snap(1.0);
        self.refill = 0;
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for side in &mut self.diffusers {
            for allpass in side {
                allpass.clear();
            }
        }
        for (index, line) in self.lines.iter_mut().enumerate() {
            line.delay.clear();
            line.source.clear();
            for section in &mut line.source_band {
                section.reset();
            }
            line.band.reset();
            line.damping.reset();
            line.low_cut.reset();
            line.shifter.reset(index as f32 / 8.0);
        }
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
    use super::super::line::{Kernel, Line};
    use super::super::testing::{self, RATE, build, noise, rms, run_mono, silence, sine};

    /// The strength of `hz` in `signal` at `rate` (a Hann-windowed DFT bin).
    fn strength(signal: &[f32], hz: f32, rate: f32) -> f32 {
        let size = signal.len() as f64;
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, &x) in signal.iter().enumerate() {
            let hann = 0.5f64.mul_add(-(std::f64::consts::TAU * n as f64 / size).cos(), 0.5);
            let angle = std::f64::consts::TAU * f64::from(hz) * n as f64 / f64::from(rate);
            re = (f64::from(x) * hann).mul_add(angle.cos(), re);
            im = (f64::from(x) * hann).mul_add(angle.sin(), im);
        }
        re.hypot(im) as f32
    }

    #[test]
    fn the_shimmer_keeps_the_contract() {
        testing::contract("shimmer");
    }

    #[test]
    fn the_blend_holds_power_for_unlike_audio_and_amplitude_for_like() {
        for n in 0..1_000 {
            let phase = n as f32 / 1_000.0;
            let (a, b) = super::blend(phase, 0.0);
            assert!((a.hypot(b) - 1.0).abs() < 1e-5);
            let (a, b) = super::blend(phase, 1.0);
            assert!((a + b - 1.0).abs() < 1e-5, "{phase}: {a} + {b}");
        }
    }

    #[test]
    fn one_pass_of_the_shifter_does_not_chop_a_steady_note() {
        // A steady tone read an octave up by one aligned shifter: the
        // octave comes through at full level with no throbbing sidebands.
        for hz in [200.0f32, 460.0, 1_234.0] {
            let ratio = 2.0f32;
            let grain = super::grain_of(ratio, RATE);
            let reading = super::Reading {
                base: 0.02 * RATE,
                grain,
                step: (1.0 - ratio) / grain,
                search: (super::SEARCH * RATE) as i32,
                span: (super::MATCH * RATE) as usize,
                stride: 4,
            };
            let kernel = Kernel::new(RATE);
            let mut line = Line::new((0.4 * RATE) as usize, &kernel);
            let mut shifter = super::Shifter::default();
            shifter.reset(0.0);
            let total = (3.0 * RATE) as usize;
            let mut out = Vec::with_capacity(total);
            for n in 0..total {
                line.push(0.5 * (std::f32::consts::TAU * hz * n as f32 / RATE).sin());
                out.push(shifter.read(&line, reading));
            }
            let window = &out[total - 2 * RATE as usize..];
            let carrier = strength(window, 2.0 * hz, RATE);
            let expected = strength(&sine(2.0, 2.0 * hz, 0.5), 2.0 * hz, RATE);
            assert!(
                carrier > 0.7 * expected,
                "{hz} Hz: the octave came through at {}",
                carrier / expected
            );
            for offset in (4..300).step_by(2) {
                for side in [-1.0f32, 1.0] {
                    let near = strength(window, side.mul_add(offset as f32, 2.0 * hz), RATE);
                    let level = 20.0 * (near / carrier).log10();
                    assert!(level < -40.0, "{hz} Hz: {level} dBc at {offset} Hz off");
                }
            }
        }
    }

    #[test]
    fn nothing_folds_back_past_nyquist() {
        // 15 kHz an octave up at 44.1 kHz would fold to 14.1 kHz.
        let rate = 44_100.0f32;
        let mut shimmer = testing::build_at(
            "shimmer",
            rate,
            &[("mix", 1.0), ("shimmer", 1.0), ("damping", 16_000.0)],
        );
        let tone: Vec<f32> = (0..(2.0 * rate) as usize)
            .map(|n| 0.5 * (std::f32::consts::TAU * 15_000.0 * n as f32 / rate).sin())
            .collect();
        let (left, _) = run_mono(shimmer.as_mut(), &tone);
        let window = &left[rate as usize..];
        let folded = strength(window, 14_100.0, rate);
        let reference = strength(&tone[rate as usize..], 15_000.0, rate);
        let level = 20.0 * (folded / reference).log10();
        assert!(level < -60.0, "folded energy at {level} dB");
    }

    #[test]
    fn the_shimmer_is_the_same_at_every_rate_two_octaves_up_and_open() {
        testing::rates_agree_with(
            "shimmer",
            &[("shift", 7.0), ("shimmer", 1.0), ("damping", 16_000.0)],
        );
    }

    #[test]
    fn an_octave_up_puts_energy_an_octave_up() {
        // A 400 Hz tone in; the tail should hold 800 Hz.
        let mut shimmer = build("shimmer", &[("mix", 1.0), ("shimmer", 1.0), ("shift", 5.0)]);
        run_mono(shimmer.as_mut(), &sine(1.0, 400.0, 0.5));
        let (tail, _) = run_mono(shimmer.as_mut(), &silence(1.0));
        assert!(
            strength(&tail, 800.0, RATE) > 3.0 * strength(&tail, 600.0, RATE),
            "no octave"
        );
    }

    #[test]
    fn changing_the_shift_does_not_click() {
        let jumps = |change: bool| {
            let mut shimmer = build("shimmer", &[("mix", 1.0), ("shimmer", 1.0)]);
            run_mono(shimmer.as_mut(), &sine(1.0, 330.0, 0.3));
            let mut worst = 0.0f32;
            for step in [7.0, 0.0, 3.0, 6.0] {
                if change {
                    shimmer.set_param(super::SHIFT, step);
                }
                let (left, _) = run_mono(shimmer.as_mut(), &sine(0.3, 330.0, 0.3));
                worst = left
                    .windows(2)
                    .fold(worst, |m, w| m.max((w[1] - w[0]).abs()));
            }
            worst
        };
        let (changed, still) = (jumps(true), jumps(false));
        assert!(
            changed <= 1.5f32.mul_add(still, 1e-3),
            "{changed} against {still}"
        );
    }

    #[test]
    fn a_hot_input_stays_clean_in_the_tank() {
        // With the shifter out the tank is linear. Played hot enough to
        // bring the output right up to full scale, it must still give back
        // exactly a scaled copy of what it gives back played quietly: the
        // in-loop ceiling is nowhere near.
        let run = |level: f32| {
            let mut shimmer = build(
                "shimmer",
                &[("mix", 1.0), ("shimmer", 0.0), ("damping", 16_000.0)],
            );
            let (left, _) = run_mono(shimmer.as_mut(), &sine(2.0, 1_000.0, level));
            left
        };
        let quiet = run(0.01);
        let loudest = testing::peak(&quiet);
        let level = 0.01 * 0.95 / loudest;
        let hot = run(level);
        let scale = level / 0.01;
        let residual: Vec<f32> = hot.iter().zip(&quiet).map(|(h, q)| h - scale * q).collect();
        let error = 20.0 * (rms(&residual) / rms(&hot)).log10();
        assert!(level > 0.5, "the tank is louder than expected ({level})");
        assert!(error < -60.0, "{error} dB of distortion at {level}");
    }

    #[test]
    fn every_shift_stays_bounded_and_dies_away_at_the_longest_decay() {
        for step in 0..8 {
            let mut shimmer = build(
                "shimmer",
                &[
                    ("mix", 1.0),
                    ("shimmer", 1.0),
                    ("decay", 20.0),
                    ("shift", step as f32),
                    ("damping", 16_000.0),
                ],
            );
            run_mono(shimmer.as_mut(), &noise(3.0, 1.0, 31));
            let (left, _) = run_mono(shimmer.as_mut(), &silence(8.0));
            let second = RATE as usize;
            let first = rms(&left[..second]);
            let last = rms(&left[left.len() - second..]);
            // A 20 s RT60 falls 3 dB a second; anything self-sustaining
            // would not fall at all.
            assert!(
                last < 0.5 * first,
                "shift step {step} held on: {last} after {first}"
            );
        }
    }

    fn decay(knobs: &[(&str, f32)]) -> f32 {
        let mut all = vec![("mix", 1.0), ("decay", 3.0)];
        all.extend_from_slice(knobs);
        let mut shimmer = build("shimmer", &all);
        let (left, right) = run_mono(shimmer.as_mut(), &testing::impulse(5.0));
        let mono: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l + r).collect();
        testing::rt60(&testing::lowpassed(&mono, 2_000.0))
    }

    #[test]
    fn with_the_shifter_out_the_decay_knob_is_a_true_rt60() {
        let measured = decay(&[("shimmer", 0.0), ("damping", 16_000.0)]);
        assert!((measured / 3.0 - 1.0).abs() < 0.12, "{measured} s");
    }

    #[test]
    fn the_shifter_never_stretches_the_tail_past_the_knob() {
        for step in 0..8 {
            let measured = decay(&[("shimmer", 1.0), ("shift", step as f32)]);
            assert!(
                measured < 1.2 * 3.0 && measured > 0.1 * 3.0,
                "shift step {step}: {measured} s"
            );
        }
    }

    #[test]
    fn at_a_moderate_decay_the_tail_dies() {
        let mut shimmer = build("shimmer", &[("mix", 1.0), ("shimmer", 0.8), ("decay", 3.0)]);
        let (fed, _) = run_mono(shimmer.as_mut(), &noise(1.0, 0.5, 33));
        let (left, _) = run_mono(shimmer.as_mut(), &silence(12.0));
        let end = rms(&left[left.len() - RATE as usize..]);
        assert!(end < 1e-3 * rms(&fed), "{end}");
    }
}
