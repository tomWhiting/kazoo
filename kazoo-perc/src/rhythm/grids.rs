//! A topographic drum map, in the spirit of Mutable Instruments' Grids.
//!
//! The map is a square of nine hand-written drum patterns, each sixteen
//! steps of kick, snare and hat. Every step of every part holds a weight
//! from 0 to 255: how essential that hit is to the groove. The downbeat
//! kick and backbeat snare weigh 255; ghost notes and fills weigh less.
//! Moving `x` and `y` blends the weights of the four nearest patterns, so
//! the groove morphs smoothly from one style to the next instead of
//! switching.
//!
//! Across (`x`) the patterns get busier. Down (`y`) the feel changes:
//!
//! | y \ x    | 0                | 0.5             | 1                    |
//! |----------|------------------|-----------------|----------------------|
//! | 0        | house            | disco           | driving techno       |
//! | 0.5      | boom bap         | funk            | breakbeat            |
//! | 1        | one drop         | bossa nova      | afro-cuban           |
//!
//! Each part's density knob sets a threshold: a step plays when its weight
//! clears it, so turning density up adds the less essential hits in order
//! of importance (the essential ones first, the ghost notes last) and
//! density 0 plays nothing. Chaos shakes every weight up or down at random
//! on every step, so the pattern breathes and fills appear. The accent
//! output goes high on kick or snare hits whose weight (before chaos) is
//! 200 or more: the strong beats.
//!
//! Each clock edge is a sixteenth; all four gates follow the clock's width.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Inputs, Knobs, Stepper, drive, gate, uniform};
use crate::{MAX_OUTPUTS, Output, Rhythm, RhythmKind, Signal};

const X: usize = 0;
const Y: usize = 1;
const KICK: usize = 2;
const SNARE: usize = 3;
const HAT: usize = 4;
const CHAOS: usize = 5;

/// Steps in each pattern.
pub const STEPS: usize = 16;

/// A pattern's three parts: kick, snare, hat.
type Pattern = [[u8; STEPS]; 3];

/// The map, row by row (`y` = 0, 0.5, 1), left to right (`x` = 0, 0.5, 1).
const MAP: [[Pattern; 3]; 3] = [
    [
        // House: four on the floor, claps on two and four, open offbeat hats.
        [
            [255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 40],
            [0, 0, 0, 0, 230, 0, 0, 0, 0, 0, 0, 0, 230, 0, 60, 0],
            [60, 0, 230, 0, 60, 0, 230, 0, 60, 0, 230, 0, 60, 0, 230, 90],
        ],
        // Disco: the same floor, sixteenth hats with the offbeat leaning.
        [
            [255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0],
            [0, 0, 0, 0, 255, 0, 0, 70, 0, 0, 0, 0, 255, 0, 0, 0],
            [
                180, 90, 230, 90, 180, 90, 230, 90, 180, 90, 230, 90, 180, 90, 230, 120,
            ],
        ],
        // Driving techno: ghost kicks between, rolling hats, snare fills.
        [
            [255, 0, 60, 0, 255, 0, 0, 110, 255, 0, 60, 0, 255, 0, 140, 0],
            [0, 0, 0, 0, 200, 0, 0, 0, 0, 0, 90, 0, 200, 0, 0, 120],
            [
                150, 120, 230, 120, 150, 120, 230, 120, 150, 120, 230, 120, 150, 160, 230, 200,
            ],
        ],
    ],
    [
        // Boom bap: a lazy kick on one and the and of three, heavy snares.
        [
            [255, 0, 0, 0, 0, 0, 0, 120, 0, 0, 230, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 40],
            [
                220, 0, 180, 0, 220, 0, 180, 0, 220, 0, 180, 0, 220, 0, 180, 60,
            ],
        ],
        // Funk: syncopated kicks, ghosted snares around the backbeat.
        [
            [255, 0, 110, 0, 0, 0, 0, 0, 200, 0, 230, 0, 0, 0, 90, 0],
            [0, 0, 0, 90, 255, 0, 70, 0, 0, 110, 0, 90, 255, 0, 0, 130],
            [
                220, 120, 200, 120, 220, 120, 200, 120, 220, 120, 200, 120, 220, 120, 200, 160,
            ],
        ],
        // Breakbeat: a broken kick and a snare that answers it.
        [
            [255, 0, 200, 0, 0, 0, 0, 0, 0, 0, 230, 160, 0, 0, 0, 0],
            [0, 0, 0, 0, 255, 0, 0, 130, 0, 120, 0, 0, 255, 0, 0, 140],
            [
                230, 110, 230, 110, 230, 110, 230, 110, 230, 110, 230, 110, 230, 110, 230, 110,
            ],
        ],
    ],
    [
        // One drop: nothing on one; kick and rim together on three.
        [
            [0, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, 70],
            [90, 0, 230, 0, 90, 0, 230, 0, 90, 0, 230, 0, 90, 0, 230, 120],
        ],
        // Bossa nova: the surdo's dotted pulse and the bossa clave.
        [
            [
                255, 0, 0, 160, 255, 0, 0, 160, 255, 0, 0, 160, 255, 0, 0, 160,
            ],
            [230, 0, 0, 230, 0, 0, 230, 0, 0, 0, 230, 0, 0, 230, 0, 0],
            [
                200, 90, 140, 90, 200, 90, 140, 90, 200, 90, 140, 90, 200, 90, 140, 90,
            ],
        ],
        // Afro-cuban: a tumbao-like kick, busy snare and the bell pattern.
        [
            [255, 0, 0, 140, 0, 0, 200, 0, 0, 0, 230, 0, 0, 0, 110, 0],
            [0, 0, 120, 0, 230, 0, 0, 110, 0, 130, 0, 0, 230, 0, 100, 0],
            [
                230, 0, 200, 0, 230, 200, 0, 230, 0, 230, 0, 200, 230, 0, 200, 0,
            ],
        ],
    ],
];

static PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "x",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "y",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "kick",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "snare",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "hat",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "chaos",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Linear,
    },
];

static OUTPUTS: [Output; 4] = [
    Output {
        name: "kick",
        signal: Signal::Gate,
    },
    Output {
        name: "snare",
        signal: Signal::Gate,
    },
    Output {
        name: "hat",
        signal: Signal::Gate,
    },
    Output {
        name: "accent",
        signal: Signal::Gate,
    },
];

/// The drum map.
pub const KIND: RhythmKind = RhythmKind {
    id: "grids",
    name: "Drum map",
    description: "Nine hand-written grooves on an x/y map, blended smoothly, with a density \
                  per part, chaos and an accent output.",
    params: &PARAMS,
    outputs: &OUTPUTS,
    build,
};

fn build() -> Box<dyn Rhythm> {
    Box::new(Grids::new())
}

/// The weight an accented kick or snare must reach.
const ACCENT_WEIGHT: f32 = 200.0;

/// How far chaos can move a weight, either way.
const CHAOS_REACH: f32 = 110.0;

/// The weight of `part` at `step`, blended across the map at (`x`, `y`).
#[must_use]
pub fn weight(x: f32, y: f32, part: usize, step: usize) -> f32 {
    let spot = |position: f32| {
        let scaled = position.clamp(0.0, 1.0) * 2.0;
        let cell = (scaled.floor() as usize).min(1);
        (cell, scaled - cell as f32)
    };
    let (column, across) = spot(x);
    let (row, down) = spot(y);
    let part = part.min(2);
    let step = step % STEPS;
    let at = |r: usize, c: usize| f32::from(MAP[r][c][part][step]);
    let top = (at(row, column + 1) - at(row, column)).mul_add(across, at(row, column));
    let bottom =
        (at(row + 1, column + 1) - at(row + 1, column)).mul_add(across, at(row + 1, column));
    (bottom - top).mul_add(down, top)
}

/// The drum map generator.
#[derive(Debug, Clone)]
pub struct Grids {
    knobs: Knobs<6>,
    inputs: Inputs,
    dice: Noise,
    next: usize,
    hits: [bool; 3],
    accent: bool,
}

impl Default for Grids {
    fn default() -> Self {
        Self::new()
    }
}

impl Grids {
    /// A drum map at the default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            knobs: Knobs::new(&PARAMS),
            inputs: Inputs::default(),
            dice: Noise::new(0x4752_4944),
            next: 0,
            hits: [false; 3],
            accent: false,
        }
    }

    fn step(&mut self) {
        let step = self.next;
        self.next = (step + 1) % STEPS;
        let x = self.knobs.get(X);
        let y = self.knobs.get(Y);
        let chaos = self.knobs.get(CHAOS);
        self.accent = false;
        for (part, density_knob) in [KICK, SNARE, HAT].into_iter().enumerate() {
            let density = self.knobs.get(density_knob);
            let written = weight(x, y, part, step);
            let shaken = uniform(&mut self.dice).mul_add(2.0, -1.0) * chaos * CHAOS_REACH;
            let threshold = 255.0 * (1.0 - density);
            let hit = density > 0.0 && written + shaken > threshold && written + shaken > 0.0;
            self.hits[part] = hit;
            if hit && part < 2 && written >= ACCENT_WEIGHT {
                self.accent = true;
            }
        }
    }
}

impl Stepper for Grids {
    fn tick(&mut self, clock_rise: bool, clock_high: bool) -> [f32; MAX_OUTPUTS] {
        if clock_rise {
            self.step();
        }
        [
            gate(clock_high && self.hits[0]),
            gate(clock_high && self.hits[1]),
            gate(clock_high && self.hits[2]),
            gate(clock_high && self.accent),
        ]
    }

    fn restart(&mut self) {
        self.next = 0;
        self.hits = [false; 3];
        self.accent = false;
    }
}

impl Rhythm for Grids {
    /// The map counts clock edges only, so the rate needs no sizing.
    fn prepare(&mut self, _sample_rate: f32) {
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
    fn the_corners_are_the_written_patterns() {
        for (patterns, y) in MAP.iter().zip([0.0, 0.5, 1.0]) {
            for (pattern, x) in patterns.iter().zip([0.0, 0.5, 1.0]) {
                for (part, weights) in pattern.iter().enumerate() {
                    for (step, want) in weights.iter().enumerate() {
                        let got = weight(x, y, part, step);
                        assert!((got - f32::from(*want)).abs() < 1.0e-3);
                    }
                }
            }
        }
    }

    #[test]
    fn between_the_corners_the_weights_blend() {
        // Halfway from house to disco on the snare's step 7: 0 and 70.
        assert!((weight(0.25, 0.0, 1, 7) - 35.0).abs() < 1.0e-3);
    }
}
