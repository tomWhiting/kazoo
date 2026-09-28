//! Phaser, modelled on the MXR Phase 90 and the Electro-Harmonix Small
//! Stone.
//!
//! # The model
//!
//! Both pedals run the signal through a chain of first-order allpass
//! stages, each an op-amp with a capacitor and a voltage-controlled
//! resistance (a JFET in the Phase 90, an OTA in the Small Stone), and sum
//! the result with the dry signal. Each stage leaves the level alone but
//! turns the phase by 90° at its corner and 180° far above it, so the sum
//! cancels wherever the chain has turned an odd multiple of 180°: one notch
//! per two stages, sweeping as the LFO moves the corners.
//!
//! - **Stages.** Each stage here is the bilinear image of that op-amp
//!   allpass, `(a + z⁻¹) / (1 + a z⁻¹)`, with its corner recomputed every
//!   sample. `stages` picks 4 (Phase 90, Small Stone), 6, 8 or 12; the
//!   choice crossfades between taps of one 12-stage chain, so it never
//!   clicks.
//! - **Parts tolerance.** Real stages are matched by hand, never exactly:
//!   each stage's corner sits a few per cent off the others, which softens
//!   and widens the notches the way the pedals' do.
//! - **The sweep** is a triangle about `centre`, by up to two and a half
//!   octaves either way at full `depth`, made exponential in the LFO so the
//!   notches travel evenly in pitch, as the ear hears them. (An OTA's gain
//!   is linear in its bias current, and the Phase 90's JFETs bend the sweep
//!   their own lopsided way; the even, musical sweep here is a choice, not a
//!   copy of either.)
//! - **Colour** is the Small Stone's colour switch and the block-logo Phase
//!   90's feedback resistor made continuous: the chain's output fed back to
//!   its input, from -0.9 to 0.9, for sharper, vocal resonances. The
//!   feedback is soft-clipped (anti-aliased), like an overdriven OTA.
//! - `spread` runs the right side's LFO up to half a cycle ahead.
//!
//! At a `mix` of a half the dry and wet are equal, as in the pedals, for the
//! deepest notches.
//!
//! Sources: the Phase 90 and Small Stone schematics and R. G. Keen's
//! analysis of the Phase 90.

use super::parts::{
    Saturator, accept, clean, defaults, frames, guard, prewarp, silence_from, triangle,
};
use crate::dsp::{Phasor, Smoothed, flush, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const RATE: usize = 0;
const DEPTH: usize = 1;
const CENTRE: usize = 2;
const COLOUR: usize = 3;
const STAGES: usize = 4;
const SPREAD: usize = 5;
const MIX: usize = 6;

/// The longest chain, and where the shorter ones are tapped from it.
const CHAIN: usize = 12;
const TAPS: [usize; 4] = [4, 6, 8, 12];
/// How far each stage's corner sits from the others: hand-matched parts.
const TOLERANCE: [f32; CHAIN] = [
    1.0, 0.93, 1.06, 0.97, 1.04, 0.95, 1.08, 0.99, 1.02, 0.94, 1.05, 0.96,
];
const OCTAVES: f32 = 2.5;

const PARAMS: [ParamSpec; 7] = [
    ParamSpec {
        name: "rate",
        min: 0.02,
        max: 10.0,
        default: 0.4,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "depth",
        min: 0.0,
        max: 1.0,
        default: 0.8,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "centre",
        min: 100.0,
        max: 4_000.0,
        default: 600.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "colour",
        min: -0.9,
        max: 0.9,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "stages",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["4", "6", "8", "12"],
        },
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
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The phaser.
pub static KIND: EffectKind = EffectKind {
    id: "phaser",
    name: "Phaser",
    description: "An allpass-stage phaser after the Phase 90 and Small Stone: 4 to 12 \
                  stages, with colour feedback and stereo spread.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Phaser::new())
}

/// One side's allpass chain.
#[derive(Debug, Clone, Copy, Default)]
struct Chain {
    states: [f32; CHAIN],
    last: f32,
    clip: Saturator,
}

impl Chain {
    /// Run `input` through every stage at corner coefficient `warped` (the
    /// prewarped corner); returns the output after each tap.
    fn process(&mut self, input: f32, warped: f32) -> [f32; 4] {
        let mut signal = input;
        let mut taps = [0.0; 4];
        let mut next_tap = 0;
        for (stage, (state, tolerance)) in self.states.iter_mut().zip(TOLERANCE).enumerate() {
            let corner = warped * tolerance;
            let a = (corner - 1.0) / (corner + 1.0);
            let out = a.mul_add(signal, *state);
            *state = (-a).mul_add(out, signal);
            flush(state);
            signal = out;
            if TAPS.get(next_tap) == Some(&(stage + 1)) {
                taps[next_tap] = signal;
                next_tap += 1;
            }
        }
        taps
    }
}

/// The phaser.
#[derive(Debug)]
pub struct Phaser {
    rate: f32,
    prepared: bool,
    speed: Smoothed,
    depth: Smoothed,
    centre: Smoothed,
    colour: Smoothed,
    spread: Smoothed,
    mix: Smoothed,
    taps: [Smoothed; 4],
    lfo: Phasor,
    chains: [Chain; 2],
}

impl Default for Phaser {
    fn default() -> Self {
        Self::new()
    }
}

impl Phaser {
    /// A phaser at its defaults, unprepared.
    #[must_use]
    pub fn new() -> Self {
        let values = defaults(&PARAMS);
        Self {
            rate: 48_000.0,
            prepared: false,
            speed: Smoothed::new(values[RATE]),
            depth: Smoothed::new(values[DEPTH]),
            centre: Smoothed::new(values[CENTRE]),
            colour: Smoothed::new(values[COLOUR]),
            spread: Smoothed::new(values[SPREAD]),
            mix: Smoothed::new(values[MIX]),
            taps: tap_weights(values[STAGES]).map(Smoothed::new),
            lfo: Phasor::default(),
            chains: [Chain::default(); 2],
        }
    }

    fn smoothers(&mut self) -> impl Iterator<Item = &mut Smoothed> {
        [
            &mut self.speed,
            &mut self.depth,
            &mut self.centre,
            &mut self.colour,
            &mut self.spread,
            &mut self.mix,
        ]
        .into_iter()
        .chain(&mut self.taps)
    }

    fn render(&mut self, input: [&[f32]; 2], output: &mut [&mut [f32]; 2], n: usize) {
        let rate = self.rate;
        for i in 0..n {
            let depth = self.depth.step();
            let centre = self.centre.step();
            let colour = self.colour.step();
            let spread = self.spread.step() * 0.005;
            let mix = self.mix.step();
            let weights = [
                self.taps[0].step(),
                self.taps[1].step(),
                self.taps[2].step(),
                self.taps[3].step(),
            ];
            let phase = self.lfo.next(self.speed.step(), rate);
            for (c, chain) in self.chains.iter_mut().enumerate() {
                let x = clean(input[c][i]);
                let sweep = triangle(spread.mul_add(c as f32, phase).fract());
                let hz = centre * (OCTAVES * depth * sweep).exp2();
                let warped = prewarp(hz, rate);
                let into = x + chain.clip.process(colour * chain.last, 1.5);
                let taps = chain.process(into, warped);
                let wet: f32 = taps
                    .iter()
                    .zip(weights)
                    .map(|(tap, weight)| tap * weight)
                    .sum();
                chain.last = wet;
                flush(&mut chain.last);
                output[c][i] = guard((wet - x).mul_add(mix, x));
            }
        }
    }
}

/// Which tap is heard at `stages` step `step`.
fn tap_weights(step: f32) -> [f32; 4] {
    let chosen = (step.round().max(0.0) as usize).min(TAPS.len() - 1);
    let mut weights = [0.0; 4];
    weights[chosen] = 1.0;
    weights
}

impl Effect for Phaser {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        self.rate = rate;
        for smoother in self.smoothers() {
            smoother.set_time(0.02, rate);
        }
        for tap in &mut self.taps {
            tap.set_time(0.03, rate);
        }
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        self.chains = [Chain::default(); 2];
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
            RATE => self.speed.set(value),
            DEPTH => self.depth.set(value),
            CENTRE => self.centre.set(value),
            COLOUR => self.colour.set(value),
            SPREAD => self.spread.set(value),
            MIX => self.mix.set(value),
            STAGES => {
                for (tap, weight) in self.taps.iter_mut().zip(tap_weights(value)) {
                    tap.set(weight);
                }
            }
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
    use crate::time::testkit::{RATE as HOST, context, prepared, render, sine, tone_level};

    fn still(stages: f32) -> Box<dyn Effect> {
        let mut phaser = prepared(&KIND);
        phaser.set_param(DEPTH, 0.0);
        phaser.set_param(COLOUR, 0.0);
        phaser.set_param(CENTRE, 1_000.0);
        phaser.set_param(STAGES, stages);
        phaser.reset();
        phaser
    }

    fn level(phaser: &mut dyn Effect, hz: f32) -> f32 {
        let input = sine(HOST as usize / 5, hz, 0.5);
        let (left, _) = render(phaser, context(120.0), &input, &input, 256);
        tone_level(&left[4_800..], hz) / 0.5
    }

    #[test]
    fn four_stages_notch_where_the_chain_turns_half_a_cycle() {
        // Four stages at 1 kHz turn 180° where each turns 45°, at
        // 1 kHz · tan(22.5°) ≈ 414 Hz, and again at 1 kHz · tan(67.5°) ≈ 2414 Hz;
        // at 1 kHz they turn a whole cycle and the sum doubles back to unity.
        let mut phaser = still(0.0);
        assert!(level(phaser.as_mut(), 414.0) < 0.1);
        assert!(level(phaser.as_mut(), 2_414.0) < 0.1);
        assert!(level(phaser.as_mut(), 1_000.0) > 0.9);
    }

    #[test]
    fn more_stages_make_more_notches() {
        let count = |stages: f32| {
            let mut phaser = still(stages);
            let mut notches = 0;
            let mut last = 1.0;
            let mut falling = false;
            for step in 0..195 {
                let hz = 50.0 * 1.03f32.powi(step);
                let now = level(phaser.as_mut(), hz);
                if now > last && falling && last < 0.3 {
                    notches += 1;
                }
                falling = now < last;
                last = now;
            }
            notches
        };
        assert_eq!(count(0.0), 2);
        assert_eq!(count(3.0), 6);
    }
}
