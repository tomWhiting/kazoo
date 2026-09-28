//! Probability sequencer: every step has a chance of firing, and a firing
//! step may ratchet into quick repeats.
//!
//! Each of up to sixteen steps has its own chance (`p1` to `p16`). The
//! density knob leans on all of them together: at 0.5 each step fires with
//! exactly its own chance; toward 0 every chance shrinks to never, toward 1
//! every chance grows to always. So density 0 is silence and density 1
//! fires every step, whatever the steps say.
//!
//! A firing step ratchets with the chance set by `ratchet`: instead of one
//! gate it plays `repeats` evenly spaced short gates across the step, each
//! half its slot long, the drum-roll fill of a drum machine. Ratchets need
//! the step length, so they start from the second clock edge (the first
//! only starts the measurement); a clock that changes speed is followed
//! from edge to edge. A step that does not ratchet follows the clock's own
//! width. The accent output goes high with the gate on steps that win the
//! `accent` chance.
//!
//! **Lock.** Free, the dice keep rolling and no two cycles are alike.
//! Locked, the dice are reloaded at the first step of every cycle, so the
//! same random pattern repeats until lock is released or a knob changes
//! what it means, the way a Turing Machine locks its loop.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Inputs, Knobs, Meter, Stepper, drive, gate, uniform};
use crate::parts::numbers;
use crate::{MAX_OUTPUTS, Output, Rhythm, RhythmKind, Signal};

const STEPS: usize = 0;
const DENSITY: usize = 1;
const RATCHET: usize = 2;
const REPEATS: usize = 3;
const ACCENT: usize = 4;
const LOCK: usize = 5;
const FIRST_CHANCE: usize = 6;

/// How many steps there are.
pub const MAX_STEPS: usize = 16;

/// The seed the dice are reloaded with when locked.
const SEED: u32 = 0x5052_4F42;

const fn chance(name: &'static str, default: f32) -> ParamSpec {
    ParamSpec {
        name,
        min: 0.0,
        max: 1.0,
        default,
        unit: "",
        curve: Curve::Linear,
    }
}

static PARAMS: [ParamSpec; 22] = [
    ParamSpec {
        name: "steps",
        min: 1.0,
        max: 16.0,
        default: 16.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(1, 16),
        },
    },
    chance("density", 0.5),
    chance("ratchet", 0.1),
    ParamSpec {
        name: "repeats",
        min: 2.0,
        max: 8.0,
        default: 3.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(2, 8),
        },
    },
    chance("accent", 0.25),
    ParamSpec {
        name: "lock",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["free", "locked"],
        },
    },
    chance("p1", 1.0),
    chance("p2", 0.1),
    chance("p3", 0.4),
    chance("p4", 0.15),
    chance("p5", 0.8),
    chance("p6", 0.1),
    chance("p7", 0.4),
    chance("p8", 0.25),
    chance("p9", 0.9),
    chance("p10", 0.1),
    chance("p11", 0.4),
    chance("p12", 0.15),
    chance("p13", 0.8),
    chance("p14", 0.2),
    chance("p15", 0.5),
    chance("p16", 0.35),
];

static OUTPUTS: [Output; 2] = [
    Output {
        name: "gate",
        signal: Signal::Gate,
    },
    Output {
        name: "accent",
        signal: Signal::Gate,
    },
];

/// The probability sequencer.
pub const KIND: RhythmKind = RhythmKind {
    id: "prob",
    name: "Probability sequencer",
    description: "Sixteen steps each with a chance of firing, a density knob over them all, \
                  ratchet rolls and a lockable loop.",
    params: &PARAMS,
    outputs: &OUTPUTS,
    build,
};

fn build() -> Box<dyn Rhythm> {
    Box::new(Prob::new())
}

/// A step's chance leant on by the density knob.
#[must_use]
pub fn lean(chance: f32, density: f32) -> f32 {
    if density <= 0.5 {
        chance * density * 2.0
    } else {
        (1.0 - chance).mul_add(density.mul_add(2.0, -1.0), chance)
    }
}

/// The probability sequencer.
#[derive(Debug, Clone)]
pub struct Prob {
    knobs: Knobs<22>,
    inputs: Inputs,
    meter: Meter,
    dice: Noise,
    next: usize,
    fire: bool,
    accent: bool,
    /// A ratchet in progress: how many repeats, each slot's length, and
    /// the samples since the step began.
    repeats: u32,
    slot: u32,
    since: u32,
}

impl Default for Prob {
    fn default() -> Self {
        Self::new()
    }
}

impl Prob {
    /// A sequencer at the default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            knobs: Knobs::new(&PARAMS),
            inputs: Inputs::default(),
            meter: Meter::default(),
            dice: Noise::new(SEED),
            next: 0,
            fire: false,
            accent: false,
            repeats: 0,
            slot: 0,
            since: 0,
        }
    }

    fn step(&mut self) {
        let steps = self.knobs.count(STEPS).clamp(1, MAX_STEPS);
        let step = self.next % steps;
        self.next = (step + 1) % steps;
        if step == 0 && self.knobs.count(LOCK) == 1 {
            self.dice = Noise::new(SEED);
        }
        // Always roll all three dice, so a locked loop stays the same
        // whichever of them matter this time.
        let fire_roll = uniform(&mut self.dice);
        let ratchet_roll = uniform(&mut self.dice);
        let accent_roll = uniform(&mut self.dice);
        let chance = lean(self.knobs.get(FIRST_CHANCE + step), self.knobs.get(DENSITY));
        self.fire = fire_roll < chance;
        self.accent = self.fire && accent_roll < self.knobs.get(ACCENT);
        self.repeats = 0;
        self.since = 0;
        if self.fire && ratchet_roll < self.knobs.get(RATCHET) {
            if let Some(period) = self.meter.period() {
                let repeats = self.knobs.count(REPEATS).clamp(2, 8) as u32;
                self.slot = (period / repeats).max(2);
                self.repeats = repeats;
            }
        }
    }
}

impl Stepper for Prob {
    fn tick(&mut self, clock_rise: bool, clock_high: bool) -> [f32; MAX_OUTPUTS] {
        self.meter.tick(clock_rise);
        if clock_rise {
            self.step();
        }
        let high = if self.repeats > 0 {
            let slot = self.since / self.slot;
            slot < self.repeats && self.since % self.slot < self.slot / 2
        } else {
            clock_high && self.fire
        };
        self.since = self.since.saturating_add(1);
        [gate(high), gate(high && self.accent), 0.0, 0.0]
    }

    fn restart(&mut self) {
        self.next = 0;
        self.fire = false;
        self.accent = false;
        self.repeats = 0;
        self.since = 0;
    }
}

impl Rhythm for Prob {
    /// The sequencer counts clock edges and measures them in samples, so
    /// the rate needs no sizing.
    fn prepare(&mut self, _sample_rate: f32) {
        self.meter.clear();
        self.restart();
    }

    fn reset(&mut self) {
        self.restart();
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.knobs.set(index, value);
    }

    fn process(&mut self, clock: &[f32], reset: &[f32], outputs: &mut [&mut [f32]]) {
        let mut inputs = self.inputs;
        drive(self, &mut inputs, clock, reset, outputs);
        self.inputs = inputs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn density_leans_every_chance_to_its_ends() {
        assert!(lean(0.3, 0.0).abs() < f32::EPSILON);
        assert!((lean(0.3, 0.5) - 0.3).abs() < 1.0e-6);
        assert!((lean(0.3, 1.0) - 1.0).abs() < 1.0e-6);
        assert!((lean(0.0, 1.0) - 1.0).abs() < 1.0e-6);
        assert!(lean(1.0, 0.0).abs() < f32::EPSILON);
    }
}
