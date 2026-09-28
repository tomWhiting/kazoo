//! Tremolo and auto-pan, with the Fender brownface harmonic tremolo.
//!
//! # The model
//!
//! A tremolo turns the level up and down with an LFO. `shape` picks how:
//!
//! - **sine** and **triangle**: the smooth throb of an optical or bias
//!   tremolo.
//! - **square**: the choppy one, with its edges rounded evenly (a `tanh`
//!   of a sine), so it never clicks.
//! - **harmonic**: the early-1960s Fender brownface circuit. The signal is
//!   split at about 650 Hz by a gentle first-order crossover, and the lows
//!   and highs are pulsed in opposite phase, so the level barely moves while
//!   the tone swings from dark to bright: a pulsing, phasey shimmer rather
//!   than a throb.
//!
//! `rate` is free, or with `sync` a note value at the host's tempo (the
//! host gives tempo but not position, so the phase runs free). `stereo`
//! moves the right side's LFO up to half a cycle away: at 100 % one side is
//! up while the other is down, which is an auto-pan, and the gain law bends
//! from linear toward equal power as the sides spread, so the pan keeps its
//! loudness. At full `depth` the
//! level dips to silence. Changing shape crossfades between the shapes, so
//! nothing clicks.
//!
//! Sources: the Fender 6G-series (brownface) schematics.

use super::parts::{
    SYNC_LABELS, accept, clean, defaults, frames, guard, silence_from, sine, synced_seconds,
    triangle,
};
use crate::dsp::{OnePole, Phasor, Smoothed, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const RATE: usize = 0;
const SYNC: usize = 1;
const DEPTH: usize = 2;
const SHAPE: usize = 3;
const STEREO: usize = 4;

const SHAPES: usize = 4;
const HARMONIC: usize = 3;
/// How hard the square is driven before it is rounded off.
const SQUARE_DRIVE: f32 = 6.0;
const CROSSOVER_HZ: f32 = 650.0;
const MIN_RATE: f32 = 0.1;
const MAX_RATE: f32 = 20.0;

const PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "rate",
        min: MIN_RATE,
        max: MAX_RATE,
        default: 5.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "sync",
        min: 0.0,
        max: 14.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: SYNC_LABELS,
        },
    },
    ParamSpec {
        name: "depth",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "shape",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["sine", "triangle", "square", "harmonic"],
        },
    },
    ParamSpec {
        name: "stereo",
        min: 0.0,
        max: 100.0,
        default: 0.0,
        unit: "%",
        curve: Curve::Linear,
    },
];

/// The tremolo.
pub static KIND: EffectKind = EffectKind {
    id: "trem",
    name: "Tremolo",
    description: "Sine, triangle, soft-square and Fender harmonic tremolo, tempo-synced, \
                  turning into an auto-pan as the stereo phase opens.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Trem::new())
}

/// The tremolo.
#[derive(Debug)]
pub struct Trem {
    rate: f32,
    prepared: bool,
    values: [f32; 5],
    speed: Smoothed,
    depth: Smoothed,
    stereo: Smoothed,
    shapes: [Smoothed; SHAPES],
    lfo: Phasor,
    crossover: [OnePole; 2],
}

impl Default for Trem {
    fn default() -> Self {
        Self::new()
    }
}

impl Trem {
    /// A tremolo at its defaults, unprepared.
    #[must_use]
    pub fn new() -> Self {
        let values = defaults(&PARAMS);
        Self {
            rate: 48_000.0,
            prepared: false,
            values,
            speed: Smoothed::new(values[RATE]),
            depth: Smoothed::new(values[DEPTH]),
            stereo: Smoothed::new(values[STEREO]),
            shapes: shape_weights(values[SHAPE]).map(Smoothed::new),
            lfo: Phasor::default(),
            crossover: [OnePole::default(); 2],
        }
    }

    fn smoothers(&mut self) -> impl Iterator<Item = &mut Smoothed> {
        [&mut self.speed, &mut self.depth, &mut self.stereo]
            .into_iter()
            .chain(&mut self.shapes)
    }

    /// The LFO rate at `bpm`: the knob, or one cycle per note value.
    fn target_rate(&self, bpm: f64) -> f32 {
        synced_seconds(self.values[SYNC], bpm)
            .map_or(self.values[RATE], |seconds| 1.0 / seconds)
            .clamp(0.01, MAX_RATE * 4.0)
    }

    fn render(&mut self, bpm: f64, input: [&[f32]; 2], output: &mut [&mut [f32]; 2], n: usize) {
        self.speed.set(self.target_rate(bpm));
        for i in 0..n {
            let depth = self.depth.step();
            let stereo = self.stereo.step() * 0.005;
            let weights = [
                self.shapes[0].step(),
                self.shapes[1].step(),
                self.shapes[2].step(),
                self.shapes[HARMONIC].step(),
            ];
            let plain = weights[0] + weights[1] + weights[2];
            let phase = self.lfo.next(self.speed.step(), self.rate);
            for (c, crossover) in self.crossover.iter_mut().enumerate() {
                let x = clean(input[c][i]);
                let place = stereo.mul_add(c as f32, phase).fract();
                // Every shape runs -1 to 1, peaking at a phase of zero.
                let round = (TAU_QUARTER + place).fract();
                let smooth = sine(round);
                let pointed = triangle(place);
                let square = (SQUARE_DRIVE * smooth).tanh() / SQUARE_DRIVE.tanh();
                let lfo = if plain > 1e-6 {
                    weights[2].mul_add(square, weights[0].mul_add(smooth, weights[1] * pointed))
                        / plain
                } else {
                    smooth
                };
                // As the sides spread apart the law bends from linear (a
                // tremolo) to a square root (an equal-power pan), so a full
                // auto-pan keeps its loudness as it swings.
                let linear = depth.mul_add(-0.5 * (1.0 - lfo), 1.0).max(0.0);
                let level = linear.powf(1.0 - stereo);
                let throb = x * level;
                let low = crossover.lowpass(x);
                let high = x - low;
                let low_gain = depth.mul_add(-0.5 * (1.0 - smooth), 1.0);
                let high_gain = depth.mul_add(-0.5 * (1.0 + smooth), 1.0);
                let harmonic = low.mul_add(low_gain, high * high_gain);
                let mixed = (harmonic - throb).mul_add(weights[HARMONIC], throb);
                output[c][i] = guard(mixed);
            }
        }
    }
}

/// A quarter turn, so the sine peaks at a phase of zero like the triangle.
const TAU_QUARTER: f32 = 0.25;

/// Which shape is heard at `shape` step `step`.
fn shape_weights(step: f32) -> [f32; SHAPES] {
    let chosen = (step.round().max(0.0) as usize).min(SHAPES - 1);
    let mut weights = [0.0; SHAPES];
    weights[chosen] = 1.0;
    weights
}

impl Effect for Trem {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        self.rate = rate;
        for crossover in &mut self.crossover {
            crossover.set_cutoff(CROSSOVER_HZ, rate);
        }
        for smoother in self.smoothers() {
            smoother.set_time(0.02, rate);
        }
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for crossover in &mut self.crossover {
            crossover.reset();
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
        self.values[index] = value;
        match index {
            DEPTH => self.depth.set(value),
            STEREO => self.stereo.set(value),
            SHAPE => {
                for (shape, weight) in self.shapes.iter_mut().zip(shape_weights(value)) {
                    shape.set(weight);
                }
            }
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
    use crate::time::testkit::{RATE as HOST, context, prepared, render, rms};

    /// Sample indices where the gain on a steady input bottoms out.
    fn troughs(signal: &[f32]) -> Vec<usize> {
        let mut found: Vec<usize> = Vec::new();
        for n in 1..signal.len() - 1 {
            let lowest = signal[n] < signal[n - 1] && signal[n] <= signal[n + 1] && signal[n] < 0.1;
            if lowest && found.last().is_none_or(|last| n - last > 1_000) {
                found.push(n);
            }
        }
        found
    }

    #[test]
    fn a_synced_tremolo_throbs_on_the_beat() {
        let mut trem = prepared(&KIND);
        trem.set_param(SYNC, 9.0);
        trem.set_param(DEPTH, 1.0);
        trem.reset();
        let steady = vec![1.0; HOST as usize * 2];
        let (left, _) = render(trem.as_mut(), context(120.0), &steady, &steady, 256);
        let dips = troughs(&left);
        assert!(dips.len() >= 3, "{dips:?}");
        for pair in dips.windows(2) {
            assert!(pair[1].abs_diff(pair[0]).abs_diff(24_000) < 10, "{dips:?}");
        }
    }

    #[test]
    fn harmonic_tremolo_keeps_the_level_and_moves_the_tone() {
        let mut trem = prepared(&KIND);
        trem.set_param(SHAPE, 3.0);
        trem.set_param(DEPTH, 1.0);
        trem.reset();
        let noise = crate::time::testkit::noise(HOST as usize, 0.5, 53);
        let (left, _) = render(trem.as_mut(), context(120.0), &noise, &noise, 256);
        // The two bands trade places, so the level stays near half the
        // input's, never dropping out as a plain tremolo at full depth does.
        let window = 480;
        let levels: Vec<f32> = left.chunks(window).skip(4).map(rms).collect();
        let lowest = levels.iter().fold(f32::MAX, |a, b| a.min(*b));
        assert!(lowest > 0.05, "{lowest}");
    }
}
