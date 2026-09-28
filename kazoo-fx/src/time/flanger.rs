//! Flanger, from the two-tape-machine trick to the bucket-brigade pedals,
//! with true through-zero flanging.
//!
//! # The model
//!
//! A flanger sums a signal with a copy of itself delayed by a few
//! milliseconds and sweeps the delay, so the comb of notches that the sum
//! makes slides up and down the spectrum.
//!
//! - **Classic** is the pedal flanger (the Electric Mistress, the BF-2): the
//!   dry path is direct and the wet delay sweeps around `manual`, by up to
//!   two and a half octaves either way at full `depth`. The sweep is
//!   exponential in the LFO, which spreads the notches' travel evenly in
//!   pitch, as the ear hears it.
//! - **Through-zero** is the original tape trick: two machines, one held a
//!   fixed distance behind, the other swept either side of it, so the wet
//!   delay passes through the dry one and the comb's notches sweep out past
//!   the top of the spectrum and back. Here the dry is held a fixed 10 ms
//!   back (a whole number of samples, so it stays bit-exact), and `manual`
//!   sets how far either side of it the wet swings at full `depth`. That
//!   hold, with the few samples both paths run behind so the band-limited
//!   read has what it needs, is reported as the effect's latency (classic
//!   mode reports just those few).
//! - **Feedback** from the wet output back into its delay sharpens the comb
//!   into resonant peaks; negative feedback moves the peaks to where the
//!   notches were, for the hollow, metallic flange. It is capped at ±0.95
//!   and soft-clipped (anti-aliased).
//! - A triangle LFO sweeps both sides, the right side `spread` of a half
//!   cycle ahead (100 % is opposite phase, for a swirling stereo sweep).
//!
//! Switching mode glides the dry delay and the sweep, so it does not click.
//! At a `mix` of a half the dry and wet are equal, for the deepest notches.

use super::parts::{
    Saturator, Sinc, accept, clean, defaults, frames, guard, silence_from, triangle,
};
use crate::dsp::{DelayLine, Phasor, Smoothed, flush, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const MANUAL: usize = 0;
const RATE: usize = 1;
const DEPTH: usize = 2;
const FEEDBACK: usize = 3;
const SPREAD: usize = 4;
const MODE: usize = 5;
const MIX: usize = 6;

const MIN_MANUAL: f32 = 0.000_2;
const MAX_MANUAL: f32 = 0.01;
/// How far the classic sweep reaches either side, in octaves of delay.
const OCTAVES: f32 = 2.5;
/// The classic sweep's shortest and longest delay, in seconds.
const SHORTEST: f32 = 0.000_05;
/// The shortest feedback loop, in seconds: the eight samples a
/// band-limited read needs, at 44.1 kHz, so that it is the same at every
/// rate.
const SHORTEST_LOOP: f32 = 8.0 / 44_100.0;
/// Samples both paths run behind, so the band-limited wet read always has
/// the newer samples it needs: the flanger's latency in classic mode.
const LOOKAHEAD: usize = 7;
const LONGEST: f32 = 0.02;

const PARAMS: [ParamSpec; 7] = [
    ParamSpec {
        name: "manual",
        min: MIN_MANUAL,
        max: MAX_MANUAL,
        default: 0.002,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "rate",
        min: 0.02,
        max: 10.0,
        default: 0.2,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "depth",
        min: 0.0,
        max: 1.0,
        default: 0.6,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "feedback",
        min: -0.95,
        max: 0.95,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "spread",
        min: 0.0,
        max: 100.0,
        default: 25.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mode",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["classic", "through-zero"],
        },
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The flanger.
pub static KIND: EffectKind = EffectKind {
    id: "flanger",
    name: "Flanger",
    description: "A swept comb with positive or negative feedback, as a pedal or as \
                  through-zero tape flanging.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Flanger::new())
}

/// One side's delays.
#[derive(Debug, Clone, Default)]
struct Side {
    wet: DelayLine,
    dry: DelayLine,
    clip: Saturator,
}

/// The flanger.
#[derive(Debug)]
pub struct Flanger {
    rate: f32,
    prepared: bool,
    manual: Smoothed,
    speed: Smoothed,
    depth: Smoothed,
    feedback: Smoothed,
    spread: Smoothed,
    zero: Smoothed,
    mix: Smoothed,
    lfo: Phasor,
    /// The through-zero dry hold, in whole samples: 10 ms at the rate.
    hold: usize,
    sides: [Side; 2],
    sinc: Sinc,
}

impl Default for Flanger {
    fn default() -> Self {
        Self::new()
    }
}

impl Flanger {
    /// A flanger at its defaults, unprepared.
    #[must_use]
    pub fn new() -> Self {
        let values = defaults(&PARAMS);
        Self {
            rate: 48_000.0,
            prepared: false,
            manual: Smoothed::new(values[MANUAL]),
            speed: Smoothed::new(values[RATE]),
            depth: Smoothed::new(values[DEPTH]),
            feedback: Smoothed::new(values[FEEDBACK]),
            spread: Smoothed::new(values[SPREAD]),
            zero: Smoothed::new(values[MODE]),
            mix: Smoothed::new(values[MIX]),
            lfo: Phasor::default(),
            hold: 0,
            sides: [Side::default(), Side::default()],
            sinc: Sinc::default(),
        }
    }

    const fn smoothers(&mut self) -> [&mut Smoothed; 7] {
        [
            &mut self.manual,
            &mut self.speed,
            &mut self.depth,
            &mut self.feedback,
            &mut self.spread,
            &mut self.zero,
            &mut self.mix,
        ]
    }

    fn render(&mut self, input: [&[f32]; 2], output: &mut [&mut [f32]; 2], n: usize) {
        let rate = self.rate;
        let hold = self.hold;
        let sinc = &self.sinc;
        for i in 0..n {
            let manual = self.manual.step();
            let depth = self.depth.step();
            let feedback = self.feedback.step();
            let spread = self.spread.step() * 0.005;
            let zero = self.zero.step();
            let mix = self.mix.step();
            let phase = self.lfo.next(self.speed.step(), rate);
            for (c, side) in self.sides.iter_mut().enumerate() {
                let x = clean(input[c][i]);
                let sweep = triangle(spread.mul_add(c as f32, phase).fract());
                let classic = (manual * (OCTAVES * depth * sweep).exp2()).clamp(SHORTEST, LONGEST);
                let through = (manual * depth * sweep).mul_add(rate, hold as f32);
                let delay = (through - classic * rate).mul_add(zero, classic * rate);
                // The feedback goes round the loop in exactly the wet delay.
                // In classic mode that is the comb's own delay, so the
                // resonances sit on its peaks (or, negative, its notches).
                // In through-zero it is the swept machine's whole delay,
                // about 10 ms, as when a tape machine's output is fed back
                // to its own input: resonances every 100 Hz or so, which
                // the moving comb sweeps across. Read before this sample
                // goes in, `delay` back is the sample `delay` ago. The
                // band-limited read needs eight samples, so no loop is
                // shorter than eight at 44.1 kHz (0.18 ms), at every rate.
                let looped = f64::from(delay).max(f64::from(SHORTEST_LOOP * rate));
                let mut returned = sinc.read(&side.wet, looped);
                flush(&mut returned);
                side.wet
                    .push(x + side.clip.process(feedback * returned, 1.5));
                // Both paths run LOOKAHEAD behind, so the band-limited read
                // always has the newer samples it needs, even at zero
                // relative delay in through-zero.
                let at = f64::from(delay) + LOOKAHEAD as f64 + 1.0;
                let wet = sinc.read(&side.wet, at);
                side.dry.push(x);
                let direct = side.dry.tap(LOOKAHEAD + 1);
                let behind = side.dry.tap(hold + LOOKAHEAD + 1);
                let dry = (behind - direct).mul_add(zero, direct);
                output[c][i] = guard((wet - dry).mul_add(mix, dry));
            }
        }
    }
}

impl Effect for Flanger {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        self.rate = rate;
        self.sinc = Sinc::new();
        for side in &mut self.sides {
            side.wet
                .resize((LONGEST.max(2.0 * MAX_MANUAL) * rate) as usize + LOOKAHEAD + 20);
            side.dry
                .resize((MAX_MANUAL * rate) as usize + LOOKAHEAD + 2);
        }
        self.hold = (MAX_MANUAL * rate).round() as usize;
        for smoother in self.smoothers() {
            smoother.set_time(0.02, rate);
        }
        self.zero.set_time(0.05, rate);
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for side in &mut self.sides {
            side.wet.clear();
            side.dry.clear();
            side.clip.reset();
        }
        for smoother in self.smoothers() {
            smoother.snap(smoother.target());
        }
        self.lfo.set(0.0);
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(value) = accept(&PARAMS, index, value) else {
            return;
        };
        match index {
            MANUAL => self.manual.set(value),
            RATE => self.speed.set(value),
            DEPTH => self.depth.set(value),
            FEEDBACK => self.feedback.set(value),
            SPREAD => self.spread.set(value),
            MODE => self.zero.set(value),
            MIX => self.mix.set(value),
            _ => {}
        }
    }

    /// Both paths run [`LOOKAHEAD`] samples behind, so the band-limited
    /// read has the newer samples it needs: that is the classic mode's
    /// latency. In through-zero mode the dry path is also held a fixed
    /// 10 ms back (a whole number of samples). It changes only with the
    /// rate and the mode switch.
    fn latency(&self) -> usize {
        if self.zero.target() >= 0.5 {
            self.hold + LOOKAHEAD
        } else {
            LOOKAHEAD
        }
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        let mut output = output;
        let n = if self.prepared {
            frames(&input, &output)
        } else {
            0
        };
        self.render(input, &mut output, n);
        silence_from(&mut output, n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::testkit::{RATE as HOST, context, prepared, render, sine, tone_level};

    fn still(mode: f32) -> Box<dyn Effect> {
        let mut flanger = prepared(&KIND);
        flanger.set_param(MANUAL, 0.001);
        flanger.set_param(DEPTH, 0.0);
        flanger.set_param(FEEDBACK, 0.0);
        flanger.set_param(MODE, mode);
        flanger.reset();
        flanger
    }

    #[test]
    fn a_still_comb_notches_at_half_the_inverse_delay() {
        // A 1 ms delay summed with the dry cancels 500 Hz and passes 1 kHz.
        let mut flanger = still(0.0);
        let ctx = context(120.0);
        let notch = sine(HOST as usize, 500.0, 0.5);
        let (left, _) = render(flanger.as_mut(), ctx, &notch, &notch, 256);
        assert!(tone_level(&left[4_800..], 500.0) < 0.01);
        let pass = sine(HOST as usize, 1_000.0, 0.5);
        let (left, _) = render(flanger.as_mut(), ctx, &pass, &pass, 256);
        assert!(tone_level(&left[4_800..], 1_000.0) > 0.45);
    }

    #[test]
    fn through_zero_at_rest_matches_the_dry() {
        // With no sweep the wet sits exactly on the dry: no notches at all.
        let mut flanger = still(1.0);
        let ctx = context(120.0);
        for hz in [500.0, 1_500.0, 5_000.0] {
            let input = sine(HOST as usize, hz, 0.5);
            let (left, _) = render(flanger.as_mut(), ctx, &input, &input, 256);
            let level = tone_level(&left[4_800..], hz);
            assert!((level - 0.5).abs() < 0.01, "{hz} {level}");
        }
    }

    #[test]
    fn negative_feedback_moves_the_peaks() {
        let level = |feedback: f32, hz: f32| {
            let mut flanger = still(0.0);
            flanger.set_param(FEEDBACK, feedback);
            flanger.reset();
            let input = sine(HOST as usize, hz, 0.2);
            let (left, _) = render(flanger.as_mut(), context(120.0), &input, &input, 256);
            tone_level(&left[9_600..], hz)
        };
        // 1 kHz is a peak of the 1 ms comb, 500 Hz a notch; negative
        // feedback turns 1 kHz down and lifts the comb between.
        assert!(level(0.8, 1_000.0) > level(-0.8, 1_000.0) * 2.0);
    }
}
