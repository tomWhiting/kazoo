//! Granular delay, in the spirit of Mutable Instruments' Clouds and Beads.
//!
//! # The model
//!
//! The input is written, all the time, into an eight-second stereo buffer.
//! Grains, short windowed snippets of that buffer, are started `density`
//! times a second (each gap jittered by up to half either way) and played over the top of
//! each other:
//!
//! - **Where from.** Each grain starts `position` seconds back in the
//!   buffer, pushed further back by up to `spread` seconds at random.
//! - **How long and what shape.** `size` sets the grain's length and
//!   `texture` its window, from a nearly square window with short
//!   raised-cosine fades (texture 0) to a full Hann bell (texture 1). The
//!   fades are never shorter than a millisecond, so no grain clicks.
//! - **Pitch.** Grains play faster or slower by `pitch` semitones, each
//!   detuned at random by up to `detune` more, and backwards with
//!   probability `reverse`.
//! - **Stereo.** Each grain is panned at random, as wide as `width` allows.
//! - **Feedback** writes the cloud back into the buffer with the input,
//!   soft-clipped and capped at 0.9, for smears that build on themselves.
//! - **Freeze** stops the writing, so the grains keep chewing on what is
//!   there for as long as it is held.
//!
//! A grain's start is placed so that, for its whole life, it reads only
//! audio that is already written and not yet overwritten, whatever its
//! pitch or direction. When freeze is switched, the grains in flight fade
//! out over 5 ms under the state they were placed for (the writing only
//! stops, or starts again, once they have gone), and then new grains take
//! over.
//!
//! Up to 64 grains play at once, from a fixed pool that never allocates; a
//! grain due when the pool is full is skipped. The random choices come from
//! a seeded generator, so the same input always gives the same cloud.
//!
//! **Level.** What goes back into the buffer is divided by the sum of the
//! live grains' window heights, so the loop can never gain, whatever the
//! grains read: with feedback capped at 0.9 it always dies away. What you
//! hear is divided by the geometric mean of that sum and the root of the
//! summed squares, so a cloud of grains all reading the same audio is at
//! most 9 dB over its source (with all 64 grains; 6 dB with 16) and a
//! scattered cloud as far under it; the output guard holds the peaks.

use std::f32::consts::{FRAC_PI_2, PI};

use super::parts::{Saturator, accept, clean, defaults, equal_power, frames, guard, silence_from};
use crate::dsp::{Noise, Smoothed, flush, hermite, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const POSITION: usize = 0;
const SIZE: usize = 1;
const DENSITY: usize = 2;
const SPREAD: usize = 3;
const PITCH: usize = 4;
const DETUNE: usize = 5;
const REVERSE: usize = 6;
const TEXTURE: usize = 7;
const FEEDBACK: usize = 8;
const WIDTH: usize = 9;
const FREEZE: usize = 10;
const MIX: usize = 11;

const GRAINS: usize = 64;
const BUFFER_SECONDS: f32 = 8.0;
const MAX_POSITION: f32 = 3.0;
const MAX_SPREAD: f32 = 1.0;
const MIN_SIZE: f32 = 0.01;
const MAX_SIZE: f32 = 1.0;
/// The furthest any grain may be pitched, in semitones.
const PITCH_LIMIT: f32 = 24.0;
const MIN_FADE: f32 = 0.001;
const RELEASE: f32 = 0.005;
/// Keep this many samples between a grain and the write head.
const MARGIN: f32 = 4.0;

const PARAMS: [ParamSpec; 12] = [
    ParamSpec {
        name: "position",
        min: 0.0,
        max: MAX_POSITION,
        default: 0.25,
        unit: "s",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "size",
        min: MIN_SIZE,
        max: MAX_SIZE,
        default: 0.1,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "density",
        min: 0.5,
        max: 100.0,
        default: 16.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "spread",
        min: 0.0,
        max: MAX_SPREAD,
        default: 0.2,
        unit: "s",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "pitch",
        min: -PITCH_LIMIT,
        max: PITCH_LIMIT,
        default: 0.0,
        unit: "st",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "detune",
        min: 0.0,
        max: 12.0,
        default: 0.1,
        unit: "st",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "reverse",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "texture",
        min: 0.0,
        max: 1.0,
        default: 0.7,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "feedback",
        min: 0.0,
        max: 0.9,
        default: 0.2,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "width",
        min: 0.0,
        max: 1.0,
        default: 0.6,
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
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The granular delay.
pub static KIND: EffectKind = EffectKind {
    id: "granular",
    name: "Granular delay",
    description: "A cloud of grains from the last few seconds: size, density, scatter, \
                  pitch, reverse, feedback and freeze.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Granular::new())
}

/// One grain in flight.
#[derive(Debug, Clone, Copy, Default)]
struct Grain {
    live: bool,
    /// Where it reads, as a fractional index into the buffer, in double
    /// precision so a pitched grain deep in the buffer reads as finely as
    /// one near the start.
    at: f64,
    /// How far it moves per sample: negative plays backwards.
    step: f64,
    age: f32,
    length: f32,
    fade: f32,
    /// While fading out early, the gain left, from 1 down to 0; zero
    /// otherwise.
    release: f32,
    left: f32,
    right: f32,
}

impl Grain {
    /// The window's height now.
    fn window(&self) -> f32 {
        let from_edge = self.age.min(self.length - self.age).max(0.0);
        let shape = if from_edge >= self.fade {
            1.0
        } else {
            0.5f32.mul_add(-(PI * from_edge / self.fade).cos(), 0.5)
        };
        if self.release > 0.0 {
            shape * self.release
        } else {
            shape
        }
    }
}

/// The granular delay.
#[derive(Debug)]
pub struct Granular {
    rate: f32,
    prepared: bool,
    values: [f32; 12],
    feedback: Smoothed,
    mix: Smoothed,
    buffer: [Vec<f32>; 2],
    mask: usize,
    write: usize,
    /// Whether the writing is stopped.
    frozen: bool,
    /// Samples left while the grains fade out before a freeze switch takes
    /// hold; zero when settled.
    settling: u32,
    grains: [Grain; GRAINS],
    until_next: f32,
    noise: Noise,
    last_cloud: [f32; 2],
    clips: [Saturator; 2],
}

impl Default for Granular {
    fn default() -> Self {
        Self::new()
    }
}

impl Granular {
    /// A granular delay at its defaults, unprepared.
    #[must_use]
    pub fn new() -> Self {
        let values = defaults(&PARAMS);
        Self {
            rate: 48_000.0,
            prepared: false,
            values,
            feedback: Smoothed::new(values[FEEDBACK]),
            mix: Smoothed::new(values[MIX]),
            buffer: [Vec::new(), Vec::new()],
            mask: 0,
            write: 0,
            frozen: false,
            settling: 0,
            grains: [Grain::default(); GRAINS],
            until_next: 0.0,
            noise: Noise::new(0xC10D_5EED),
            last_cloud: [0.0; 2],
            clips: [Saturator::default(); 2],
        }
    }

    /// A random number from 0 up to 1.
    fn unit(&mut self) -> f32 {
        self.noise.sample().mul_add(0.5, 0.5)
    }

    /// Start a grain, if the pool has room.
    fn spawn(&mut self) {
        let Some(slot) = self.grains.iter().position(|grain| !grain.live) else {
            return;
        };
        let rate = self.rate;
        let size = self.buffer[0].len() as f32;
        let semitones = self
            .noise
            .sample()
            .mul_add(self.values[DETUNE], self.values[PITCH]);
        let ratio = (semitones.clamp(-PITCH_LIMIT, PITCH_LIMIT) / 12.0).exp2();
        let backwards = self.unit() < self.values[REVERSE];
        let step = if backwards { -ratio } else { ratio };
        // The gap to the write head changes by this much per sample.
        let drift = if self.frozen { 0.0 } else { 1.0 } - step;
        let room = 2.0f32.mul_add(-MARGIN, size);
        let mut length = self.values[SIZE] * rate;
        if drift.abs() > 1e-6 {
            length = length.min(room / drift.abs());
        }
        let wanted = self
            .unit()
            .mul_add(self.values[SPREAD], self.values[POSITION])
            * rate;
        // The gap at the start and at the end must both stay inside the room.
        let end_shift = drift * length;
        let lowest = MARGIN.max(MARGIN - end_shift);
        let highest = (size - MARGIN).min(size - MARGIN - end_shift);
        let gap = wanted.clamp(lowest, highest.max(lowest));
        let fade_share = (1.0 - self.values[TEXTURE]).mul_add(-0.9, 1.0) * 0.5;
        let fade = (length * fade_share).max(MIN_FADE * rate).min(length * 0.5);
        let angle = self
            .unit()
            .mul_add(2.0, -1.0)
            .mul_add(self.values[WIDTH], 1.0)
            * FRAC_PI_2
            * 0.5;
        self.grains[slot] = Grain {
            live: true,
            // The newest sample sits just behind the write index.
            at: (self.write as f64 - 1.0 - f64::from(gap)).rem_euclid(f64::from(size)),
            step: f64::from(step),
            age: 0.0,
            length,
            fade: fade.max(1.0),
            release: 0.0,
            left: angle.cos() * std::f32::consts::SQRT_2,
            right: angle.sin() * std::f32::consts::SQRT_2,
        };
    }

    fn read(&self, channel: usize, at: f64) -> f32 {
        let buffer = &self.buffer[channel];
        let whole = at.floor();
        let frac = (at - whole) as f32;
        let base = whole as usize;
        let sample = |offset: usize| buffer[base.wrapping_add(offset) & self.mask];
        hermite(sample(usize::MAX), sample(0), sample(1), sample(2), frac)
    }

    /// Everything the live grains play this sample, side by side: the
    /// sum, the sum of their gains and the sum of their squared gains.
    fn cloud(&mut self) -> [[f32; 3]; 2] {
        let size = self.buffer[0].len() as f64;
        let release_step = 1.0 / (RELEASE * self.rate).max(1.0);
        let mut sum = [[0.0; 3]; 2];
        for index in 0..GRAINS {
            let grain = self.grains[index];
            if !grain.live {
                continue;
            }
            let height = grain.window();
            for (c, pan) in [grain.left, grain.right].into_iter().enumerate() {
                let weight = height * pan;
                let [played, weights, squares] = &mut sum[c];
                *played = weight.mul_add(self.read(c, grain.at), *played);
                *weights += weight;
                *squares = weight.mul_add(weight, *squares);
            }
            let grain = &mut self.grains[index];
            grain.at = (grain.at + grain.step).rem_euclid(size);
            grain.age += 1.0;
            if grain.release > 0.0 {
                grain.release -= release_step;
                if grain.release <= 0.0 {
                    grain.live = false;
                }
            }
            if grain.age >= grain.length {
                grain.live = false;
            }
        }
        sum
    }

    /// Start every live grain's release.
    fn release_all(&mut self) {
        for grain in &mut self.grains {
            if grain.live && grain.release <= 0.0 {
                grain.release = 1.0;
            }
        }
    }

    /// Start fading the grains out when the freeze knob has moved; the
    /// switch itself happens when they have gone.
    fn follow_freeze(&mut self) {
        let wanted = self.values[FREEZE] >= 0.5;
        if wanted != self.frozen && self.settling == 0 {
            self.release_all();
            self.settling = (RELEASE * self.rate).ceil() as u32 + 1;
        }
    }

    /// One sample of the freeze switch's settling; flips the writing when
    /// it is done.
    const fn settle(&mut self) {
        if self.settling > 0 {
            self.settling -= 1;
            if self.settling == 0 {
                self.frozen = !self.frozen;
            }
        }
    }

    fn render(&mut self, input: [&[f32]; 2], output: &mut [&mut [f32]; 2], n: usize) {
        self.follow_freeze();
        let interval = self.rate / self.values[DENSITY];
        for i in 0..n {
            let feedback = self.feedback.step();
            let (dry, wet) = equal_power(self.mix.step());
            let x = [clean(input[0][i]), clean(input[1][i])];
            if !self.frozen {
                for (c, sample) in x.iter().enumerate() {
                    let returned = self.clips[c].process(feedback * self.last_cloud[c], 1.5);
                    let mut written = sample + returned;
                    flush(&mut written);
                    self.buffer[c][self.write] = written;
                }
                self.write = (self.write + 1) & self.mask;
            }
            self.settle();
            self.until_next -= 1.0;
            if self.until_next <= 0.0 {
                if self.settling == 0 {
                    self.spawn();
                }
                self.until_next = interval.mul_add(self.unit() + 0.5, self.until_next);
            }
            let cloud = self.cloud();
            for (c, channel) in output.iter_mut().enumerate() {
                let [played, gains, squares] = cloud[c];
                // Back into the buffer: never louder than what was read.
                let mut returned = played / gains.max(1.0);
                flush(&mut returned);
                self.last_cloud[c] = returned;
                let heard = played / heard_divisor(gains, squares);
                channel[i] = guard(dry.mul_add(x[c], wet * heard));
            }
        }
    }
}

/// What the heard cloud is divided by: the geometric mean of the sum of the
/// gains and the root of their summed squares, never below 1. For N grains
/// of equal gain that leaves the fourth root of N either way: reading the
/// same audio (which adds up in step) at most +9 dB with all 64 grains and
/// +6 dB with 16; reading unrelated audio (which adds up in power) as far
/// under.
fn heard_divisor(gains: f32, squares: f32) -> f32 {
    (gains.max(0.0) * squares.max(0.0).sqrt()).sqrt().max(1.0)
}

impl Effect for Granular {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        self.rate = rate;
        let size = ((BUFFER_SECONDS * rate) as usize).next_power_of_two();
        self.buffer = [vec![0.0; size], vec![0.0; size]];
        self.mask = size - 1;
        self.feedback.set_time(0.02, rate);
        self.mix.set_time(0.02, rate);
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for channel in &mut self.buffer {
            channel.fill(0.0);
        }
        self.write = 0;
        self.grains = [Grain::default(); GRAINS];
        self.until_next = 0.0;
        self.last_cloud = [0.0; 2];
        self.frozen = self.values[FREEZE] >= 0.5;
        self.settling = 0;
        for clip in &mut self.clips {
            clip.reset();
        }
        self.noise = Noise::new(0xC10D_5EED);
        self.feedback.snap(self.feedback.target());
        self.mix.snap(self.mix.target());
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(value) = accept(&PARAMS, index, value) else {
            return;
        };
        self.values[index] = value;
        match index {
            FEEDBACK => self.feedback.set(value),
            MIX => self.mix.set(value),
            _ => {}
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
    use crate::time::testkit::{
        RATE as HOST, context, impulse, noise, peak, prepared, render, rms,
    };

    fn steady() -> Box<dyn Effect> {
        let mut cloud = prepared(&KIND);
        for (index, value) in [
            (SPREAD, 0.0),
            (DETUNE, 0.0),
            (FEEDBACK, 0.0),
            (WIDTH, 0.0),
            (MIX, 1.0),
        ] {
            cloud.set_param(index, value);
        }
        cloud.reset();
        cloud
    }

    #[test]
    fn an_impulse_comes_back_at_the_position() {
        let mut cloud = steady();
        cloud.set_param(POSITION, 0.25);
        cloud.set_param(DENSITY, 40.0);
        cloud.set_param(SIZE, 0.2);
        let input = impulse(HOST as usize, 1_000);
        let (left, _) = render(cloud.as_mut(), context(120.0), &input, &input, 256);
        let at = 1_000 + 12_000;
        let near = peak(&left[at - 3..=at + 3]);
        assert!(near > 0.1, "{near}");
        let elsewhere = peak(&left[..at - 3]).max(peak(&left[at + 4..]));
        assert!(elsewhere < near * 0.05, "{elsewhere} {near}");
    }

    #[test]
    fn freeze_keeps_the_cloud_going() {
        let mut cloud = steady();
        let ctx = context(120.0);
        let sound = noise(HOST as usize, 0.5, 59);
        render(cloud.as_mut(), ctx, &sound, &sound, 256);
        cloud.set_param(FREEZE, 1.0);
        let quiet = vec![0.0; HOST as usize];
        let mut last = Vec::new();
        for _ in 0..5 {
            last = render(cloud.as_mut(), ctx, &quiet, &quiet, 256).0;
        }
        assert!(rms(&last) > 0.05, "{}", rms(&last));
        cloud.set_param(FREEZE, 0.0);
        for _ in 0..6 {
            last = render(cloud.as_mut(), ctx, &quiet, &quiet, 256).0;
        }
        assert!(peak(&last) < 1e-4, "{}", peak(&last));
    }

    #[test]
    fn pitched_and_reversed_grains_stay_clean() {
        let mut cloud = steady();
        cloud.set_param(PITCH, 24.0);
        cloud.set_param(REVERSE, 0.5);
        cloud.set_param(SIZE, 1.0);
        cloud.set_param(POSITION, 0.0);
        let tone = crate::time::testkit::sine(HOST as usize * 4, 110.0, 0.5);
        let (left, _) = render(cloud.as_mut(), context(120.0), &tone, &tone, 256);
        // Two octaves up, a 440 Hz sine moves at most 0.029 per sample at
        // this level; a grain reading across the write head would jump.
        let jump = left
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max);
        assert!(jump < 0.2, "{jump}");
    }
}
