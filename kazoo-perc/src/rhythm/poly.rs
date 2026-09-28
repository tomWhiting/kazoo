//! Polyrhythm: two counters dividing one clock, N against M.
//!
//! Counter A fires on every `a`-th clock edge and counter B on every
//! `b`-th, so over `a × b` edges A fires `b` times and B fires `a` times:
//! 3 and 2 give the hemiola (two against three), 4 and 3 three against
//! four. `shift` starts B that many edges late, turning the same ratio
//! into a different interlock. The `both` output fires where the two
//! coincide (the downbeat they share) and `either` wherever one of them
//! fires, the composite rhythm you hear when both play.
//!
//! The first clock edge after a reset is edge 0, where A fires (and so
//! does B, unshifted). All four gates follow the clock's width.

use kazoo_fx::{Curve, ParamSpec};

use super::{Inputs, Knobs, Stepper, drive, gate};
use crate::parts::numbers;
use crate::{MAX_OUTPUTS, Output, Rhythm, RhythmKind, Signal};

const A: usize = 0;
const B: usize = 1;
const SHIFT: usize = 2;

static PARAMS: [ParamSpec; 3] = [
    ParamSpec {
        name: "a",
        min: 1.0,
        max: 32.0,
        default: 3.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(1, 32),
        },
    },
    ParamSpec {
        name: "b",
        min: 1.0,
        max: 32.0,
        default: 4.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(1, 32),
        },
    },
    ParamSpec {
        name: "shift",
        min: 0.0,
        max: 31.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(0, 31),
        },
    },
];

static OUTPUTS: [Output; 4] = [
    Output {
        name: "a",
        signal: Signal::Gate,
    },
    Output {
        name: "b",
        signal: Signal::Gate,
    },
    Output {
        name: "both",
        signal: Signal::Gate,
    },
    Output {
        name: "either",
        signal: Signal::Gate,
    },
];

/// The polyrhythm generator.
pub const KIND: RhythmKind = RhythmKind {
    id: "poly",
    name: "Polyrhythm",
    description: "Two clock dividers, N against M, with outputs for each, their coincidences \
                  and the composite.",
    params: &PARAMS,
    outputs: &OUTPUTS,
    build,
};

fn build() -> Box<dyn Rhythm> {
    Box::new(Poly::new())
}

/// The polyrhythm generator.
#[derive(Debug, Clone)]
pub struct Poly {
    knobs: Knobs<3>,
    inputs: Inputs,
    /// Clock edges since the reset.
    count: u64,
    fired: [bool; 2],
}

impl Default for Poly {
    fn default() -> Self {
        Self::new()
    }
}

impl Poly {
    /// A generator at the default settings.
    #[must_use]
    pub fn new() -> Self {
        Self {
            knobs: Knobs::new(&PARAMS),
            inputs: Inputs::default(),
            count: 0,
            fired: [false; 2],
        }
    }

    fn step(&mut self) {
        let a = self.knobs.count(A).max(1) as u64;
        let b = self.knobs.count(B).max(1) as u64;
        let shift = self.knobs.count(SHIFT) as u64 % b;
        self.fired = [self.count % a == 0, (self.count % b + b - shift) % b == 0];
        self.count = self.count.wrapping_add(1);
    }
}

impl Stepper for Poly {
    fn tick(&mut self, clock_rise: bool, clock_high: bool) -> [f32; MAX_OUTPUTS] {
        if clock_rise {
            self.step();
        }
        let [a, b] = self.fired;
        [
            gate(clock_high && a),
            gate(clock_high && b),
            gate(clock_high && a && b),
            gate(clock_high && (a || b)),
        ]
    }

    fn restart(&mut self) {
        self.count = 0;
        self.fired = [false; 2];
    }
}

impl Rhythm for Poly {
    /// The counters count clock edges only, so the rate needs no sizing.
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
