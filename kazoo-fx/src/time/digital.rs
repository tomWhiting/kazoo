//! Clean digital delay: stereo or ping-pong, in the spirit of the TC 2290
//! and the studio rack delays.
//!
//! # The model
//!
//! Two delay lines read at whole samples, so a repeat is an exact copy of
//! what went in. Each side has its own time, free (`ltime`, `rtime`) or
//! locked to a note value at the host's tempo (`lsync`, `rsync`).
//!
//! - **Time changes** crossfade, at equal power, from the old read point to
//!   the new one over 40 ms, the way the clean rack units do, so the echoes
//!   jump without a click, a dip or the pitch bend of a tape or
//!   bucket-brigade delay.
//! - **The loop.** Each side's repeats go back in through a lowcut and a
//!   highcut, so every repeat is filtered once more. `cross` sends that much
//!   of each side's repeats to the other side instead of its own.
//!   `routing` picks `stereo` (each input into its own side) or `ping-pong`
//!   (the input summed into the left only, so with `cross` up the repeats
//!   bounce left, right, left).
//! - **Ducking** follows the input and pushes the repeats down while you
//!   play, bringing them back up in the gaps (`duck`).
//! - **Freeze** stops the input, bypasses the filters and closes the loop at
//!   exactly unity, so what is in the lines repeats for as long as you hold
//!   it.
//!
//! The feedback is capped at 0.95 and soft-clipped, so it never runs away.

use std::f32::consts::FRAC_PI_2;

use super::parts::{
    Biquad, Follower, LONGEST_SYNCED_SECONDS, SYNC_LABELS, Saturator, accept, clean, defaults,
    equal_power, frames, guard, silence_from, synced_seconds,
};
use crate::dsp::{DelayLine, Smoothed, flush, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const LTIME: usize = 0;
const RTIME: usize = 1;
const LSYNC: usize = 2;
const RSYNC: usize = 3;
const FEEDBACK: usize = 4;
const CROSS: usize = 5;
const ROUTING: usize = 6;
const LOWCUT: usize = 7;
const HIGHCUT: usize = 8;
const DUCK: usize = 9;
const FREEZE: usize = 10;
const MIX: usize = 11;

const MIN_TIME: f32 = 0.001;
const MAX_FREE_TIME: f32 = 4.0;
const FADE_SECONDS: f32 = 0.04;
const REFRESH: u32 = 32;

const PARAMS: [ParamSpec; 12] = [
    ParamSpec {
        name: "ltime",
        min: MIN_TIME,
        max: MAX_FREE_TIME,
        default: 0.375,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "rtime",
        min: MIN_TIME,
        max: MAX_FREE_TIME,
        default: 0.5,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "lsync",
        min: 0.0,
        max: 14.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: SYNC_LABELS,
        },
    },
    ParamSpec {
        name: "rsync",
        min: 0.0,
        max: 14.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: SYNC_LABELS,
        },
    },
    ParamSpec {
        name: "feedback",
        min: 0.0,
        max: 0.95,
        default: 0.35,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "cross",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "routing",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["stereo", "ping-pong"],
        },
    },
    ParamSpec {
        name: "lowcut",
        min: 20.0,
        max: 2_000.0,
        default: 60.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "highcut",
        min: 500.0,
        max: 20_000.0,
        default: 12_000.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "duck",
        min: 0.0,
        max: 1.0,
        default: 0.0,
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

/// The digital delay.
pub static KIND: EffectKind = EffectKind {
    id: "digital",
    name: "Digital delay",
    description: "A clean stereo or ping-pong delay with tempo sync, cross-feedback, filters \
                  in the loop, ducking and freeze.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Digital::new())
}

/// A read point that crossfades to a new time instead of jumping.
#[derive(Debug, Clone, Copy, Default)]
struct Tap {
    now: usize,
    next: usize,
    progress: f32,
    fading: bool,
}

impl Tap {
    fn read(&mut self, line: &DelayLine, target: usize, step: f32) -> f32 {
        if !self.fading && target != self.now {
            self.next = target;
            self.progress = 0.0;
            self.fading = true;
        }
        if !self.fading {
            return line.tap(self.now);
        }
        let from = line.tap(self.now);
        let to = line.tap(self.next);
        // Equal power: the two reads are unrelated, so their powers add.
        let angle = self.progress.min(1.0) * FRAC_PI_2;
        let out = from.mul_add(angle.cos(), to * angle.sin());
        self.progress += step;
        if self.progress >= 1.0 {
            self.now = self.next;
            self.fading = false;
        }
        out
    }
}

/// One side: its line, read point and loop filters.
#[derive(Debug, Clone, Default)]
struct Side {
    line: DelayLine,
    tap: Tap,
    lowcut: Biquad,
    highcut: Biquad,
    clip: Saturator,
}

/// The digital delay.
#[derive(Debug)]
pub struct Digital {
    rate: f32,
    prepared: bool,
    settled: bool,
    values: [f32; 12],
    feedback: Smoothed,
    cross: Smoothed,
    routing: Smoothed,
    lowcut: Smoothed,
    highcut: Smoothed,
    duck: Smoothed,
    freeze: Smoothed,
    mix: Smoothed,
    follower: Follower,
    sides: [Side; 2],
    fade_step: f32,
    refresh: u32,
}

impl Default for Digital {
    fn default() -> Self {
        Self::new()
    }
}

impl Digital {
    /// A digital delay at its defaults, unprepared.
    #[must_use]
    pub fn new() -> Self {
        let values = defaults(&PARAMS);
        Self {
            rate: 48_000.0,
            prepared: false,
            settled: false,
            values,
            feedback: Smoothed::new(values[FEEDBACK]),
            cross: Smoothed::new(values[CROSS]),
            routing: Smoothed::new(values[ROUTING]),
            lowcut: Smoothed::new(values[LOWCUT]),
            highcut: Smoothed::new(values[HIGHCUT]),
            duck: Smoothed::new(values[DUCK]),
            freeze: Smoothed::new(values[FREEZE]),
            mix: Smoothed::new(values[MIX]),
            follower: Follower::default(),
            sides: [Side::default(), Side::default()],
            fade_step: 1.0,
            refresh: 0,
        }
    }

    const fn smoothers(&mut self) -> [&mut Smoothed; 8] {
        [
            &mut self.feedback,
            &mut self.cross,
            &mut self.routing,
            &mut self.lowcut,
            &mut self.highcut,
            &mut self.duck,
            &mut self.freeze,
            &mut self.mix,
        ]
    }

    /// Each side's delay in whole samples at `bpm`.
    fn targets(&self, bpm: f64) -> [usize; 2] {
        let longest = self.sides[0].line.max_delay().max(1);
        [(LTIME, LSYNC), (RTIME, RSYNC)].map(|(time, sync)| {
            let seconds = synced_seconds(self.values[sync], bpm).unwrap_or(self.values[time]);
            ((seconds * self.rate).round() as usize).clamp(1, longest)
        })
    }

    fn render(&mut self, bpm: f64, input: [&[f32]; 2], output: &mut [&mut [f32]; 2], n: usize) {
        let targets = self.targets(bpm);
        if !self.settled {
            for (side, target) in self.sides.iter_mut().zip(targets) {
                side.tap = Tap {
                    now: target,
                    next: target,
                    progress: 0.0,
                    fading: false,
                };
            }
            self.settled = true;
        }
        for i in 0..n {
            let feedback = self.feedback.step();
            let cross = self.cross.step();
            let pong = self.routing.step();
            let lowcut = self.lowcut.step();
            let highcut = self.highcut.step();
            let duck = self.duck.step();
            let frozen = self.freeze.step();
            let (dry, wet) = equal_power(self.mix.step());
            if self.refresh == 0 {
                for side in &mut self.sides {
                    side.lowcut.set_highpass(lowcut, 0.707, self.rate);
                    side.highcut.set_lowpass(highcut, 0.707, self.rate);
                }
                self.refresh = REFRESH;
            }
            self.refresh -= 1;
            let x = [clean(input[0][i]), clean(input[1][i])];
            let level = self.follower.process(x[0].abs().max(x[1].abs()));
            let ducked = duck.mul_add(-(level * 4.0).min(1.0), 1.0);
            let mono = 0.5 * (x[0] + x[1]);
            let fed = [(mono - x[0]).mul_add(pong, x[0]), x[1] * (1.0 - pong)];
            let mut heard = [0.0; 2];
            for ((side, echo), target) in self.sides.iter_mut().zip(&mut heard).zip(targets) {
                *echo = side.tap.read(&side.line, target, self.fade_step);
            }
            for (c, side) in self.sides.iter_mut().enumerate() {
                let returned = (heard[1 - c] - heard[c]).mul_add(cross, heard[c]);
                let into = fed[c] + side.clip.process(feedback * returned, 2.0);
                let filtered = side.highcut.process(side.lowcut.process(into));
                let mut write = (returned - filtered).mul_add(frozen, filtered);
                flush(&mut write);
                side.line.push(write);
                output[c][i] = guard(dry.mul_add(x[c], wet * ducked * heard[c]));
            }
        }
    }
}

impl Effect for Digital {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        self.rate = rate;
        let longest = (LONGEST_SYNCED_SECONDS.max(MAX_FREE_TIME) + 0.01) * rate;
        for side in &mut self.sides {
            side.line.resize(longest as usize);
        }
        for smoother in self.smoothers() {
            smoother.set_time(0.02, rate);
        }
        self.follower.set_times(0.005, 0.25, rate);
        self.fade_step = 1.0 / (FADE_SECONDS * rate).max(1.0);
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for side in &mut self.sides {
            side.line.clear();
            side.lowcut.reset();
            side.highcut.reset();
            side.clip.reset();
        }
        for smoother in self.smoothers() {
            smoother.snap(smoother.target());
        }
        self.follower.reset();
        self.refresh = 0;
        self.settled = false;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(value) = accept(&PARAMS, index, value) else {
            return;
        };
        self.values[index] = value;
        match index {
            FEEDBACK => self.feedback.set(value),
            CROSS => self.cross.set(value),
            ROUTING => self.routing.set(value),
            LOWCUT => self.lowcut.set(value),
            HIGHCUT => self.highcut.set(value),
            DUCK => self.duck.set(value),
            FREEZE => self.freeze.set(value),
            MIX => self.mix.set(value),
            _ => {}
        }
    }

    fn process(&mut self, context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        let mut output = output;
        let n = if self.prepared {
            frames(&input, &output)
        } else {
            0
        };
        self.render(context.bpm, input, &mut output, n);
        silence_from(&mut output, n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::testkit::{
        RATE, context, impulse, noise, peak, peak_index, prepared, render, rms,
    };

    fn plain() -> Box<dyn Effect> {
        let mut delay = prepared(&KIND);
        for (index, value) in [
            (FEEDBACK, 0.0),
            (CROSS, 0.0),
            (MIX, 1.0),
            (LOWCUT, 20.0),
            (HIGHCUT, 20_000.0),
        ] {
            delay.set_param(index, value);
        }
        delay.reset();
        delay
    }

    #[test]
    fn each_side_echoes_at_its_own_time() {
        let mut delay = plain();
        delay.set_param(LTIME, 0.25);
        delay.set_param(RTIME, 0.4);
        delay.reset();
        let input = impulse(RATE as usize, 0);
        let (left, right) = render(delay.as_mut(), context(120.0), &input, &input, 100);
        assert!(
            peak_index(&left).abs_diff(12_000) <= 1,
            "{}",
            peak_index(&left)
        );
        assert!(
            peak_index(&right).abs_diff(19_200) <= 1,
            "{}",
            peak_index(&right)
        );
    }

    #[test]
    fn synced_sides_land_on_their_notes() {
        let mut delay = plain();
        // At 90 BPM a quarter is 0.6667 s and an eighth 0.3333 s.
        delay.set_param(LSYNC, 9.0);
        delay.set_param(RSYNC, 6.0);
        delay.reset();
        let input = impulse(RATE as usize, 0);
        let (left, right) = render(delay.as_mut(), context(90.0), &input, &input, 100);
        assert!(
            peak_index(&left).abs_diff(32_000) <= 4,
            "{}",
            peak_index(&left)
        );
        assert!(
            peak_index(&right).abs_diff(16_000) <= 4,
            "{}",
            peak_index(&right)
        );
    }

    #[test]
    fn ping_pong_bounces() {
        let mut delay = plain();
        delay.set_param(LTIME, 0.1);
        delay.set_param(RTIME, 0.1);
        delay.set_param(ROUTING, 1.0);
        delay.set_param(CROSS, 1.0);
        delay.set_param(FEEDBACK, 0.8);
        delay.reset();
        let input = impulse(RATE as usize, 0);
        let (left, right) = render(delay.as_mut(), context(120.0), &input, &input, 100);
        let at = |channel: &[f32], echo: usize| peak(&channel[echo * 4_800 - 2..=echo * 4_800 + 2]);
        assert!(at(&left, 1) > 0.3 && at(&right, 1) < 1e-3);
        assert!(at(&right, 2) > 0.2 && at(&left, 2) < 1e-3);
        assert!(at(&left, 3) > 0.1 && at(&right, 3) < 1e-3);
    }

    #[test]
    fn freeze_holds_what_is_in_the_lines() {
        let mut delay = plain();
        delay.set_param(LTIME, 0.3);
        delay.set_param(RTIME, 0.3);
        let ctx = context(120.0);
        let sound = noise(RATE as usize, 0.5, 43);
        render(delay.as_mut(), ctx, &sound, &sound, 256);
        delay.set_param(FREEZE, 1.0);
        let quiet = vec![0.0; RATE as usize];
        let (first, _) = render(delay.as_mut(), ctx, &quiet, &quiet, 256);
        let mut last = first.clone();
        for _ in 0..10 {
            last = render(delay.as_mut(), ctx, &quiet, &quiet, 256).0;
        }
        let held = rms(&first[RATE as usize / 2..]);
        assert!(held > 0.1, "{held}");
        assert!(
            (rms(&last) - held).abs() < held * 0.05,
            "{held} {}",
            rms(&last)
        );
        // Input is ignored while frozen.
        let (loud, _) = render(delay.as_mut(), ctx, &sound, &sound, 256);
        assert!((rms(&loud) - held).abs() < held * 0.05);
    }

    #[test]
    fn ducking_pushes_the_repeats_down_while_playing() {
        let level = |duck: f32| {
            let mut delay = plain();
            delay.set_param(LTIME, 0.05);
            delay.set_param(DUCK, duck);
            delay.reset();
            let sound = noise(RATE as usize, 0.5, 47);
            let (left, _) = render(delay.as_mut(), context(120.0), &sound, &sound, 256);
            rms(&left[RATE as usize / 2..])
        };
        assert!(level(1.0) < level(0.0) * 0.1);
    }

    #[test]
    fn a_time_change_does_not_click() {
        let mut delay = plain();
        delay.set_param(LTIME, 0.1);
        delay.reset();
        let ctx = context(120.0);
        let tone: Vec<f32> = (0..RATE as usize)
            .map(|n| 0.5 * (std::f32::consts::TAU * 200.0 * n as f32 / RATE).sin())
            .collect();
        let (a, _) = render(delay.as_mut(), ctx, &tone[..24_000], &tone[..24_000], 64);
        delay.set_param(LTIME, 0.1013);
        let (b, _) = render(delay.as_mut(), ctx, &tone[24_000..], &tone[24_000..], 64);
        let joined: Vec<f32> = a.into_iter().chain(b).collect();
        let jump = joined[5_000..]
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max);
        assert!(jump < 0.02, "{jump}");
    }
}
