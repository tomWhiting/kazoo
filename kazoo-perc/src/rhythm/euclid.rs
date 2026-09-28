//! Euclidean rhythms: a number of hits spread as evenly as they can be
//! over a number of steps.
//!
//! Godfried Toussaint showed that Bjorklund's algorithm (written to space
//! out neutron-accelerator pulses) produces, for small numbers, most of the
//! world's traditional rhythms: 3 in 8 is the tresillo, 5 in 8 the
//! cinquillo, 5 in 16 the bossa nova's skeleton. This is Bjorklund's
//! algorithm exactly, so the patterns match Toussaint's tables (5 in 16 is
//! `x..x..x..x..x...`).
//!
//! - `steps` is the length of the cycle, `pulses` how many hits it holds
//!   (never more than the steps).
//! - `rotate` moves the whole pattern later by that many steps, wrapping
//!   round: 5 in 16 rotated by 1 is `.x..x..x..x..x..`.
//! - `accents` spreads that many accents over the hits by the same
//!   algorithm, counting hits from the first step of the cycle; the accent
//!   output is high on those hits only.
//!
//! Each clock edge is one step; both gates follow the clock's own width.

use kazoo_fx::{Curve, ParamSpec};

use super::{Inputs, Knobs, Stepper, drive, gate};
use crate::parts::numbers;
use crate::{MAX_OUTPUTS, Output, Rhythm, RhythmKind, Signal};

const STEPS: usize = 0;
const PULSES: usize = 1;
const ROTATE: usize = 2;
const ACCENTS: usize = 3;

/// The longest cycle.
pub const MAX_STEPS: usize = 32;

static PARAMS: [ParamSpec; 4] = [
    ParamSpec {
        name: "steps",
        min: 1.0,
        max: 32.0,
        default: 16.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(1, 32),
        },
    },
    ParamSpec {
        name: "pulses",
        min: 0.0,
        max: 32.0,
        default: 5.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(0, 32),
        },
    },
    ParamSpec {
        name: "rotate",
        min: 0.0,
        max: 31.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(0, 31),
        },
    },
    ParamSpec {
        name: "accents",
        min: 0.0,
        max: 32.0,
        default: 2.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(0, 32),
        },
    },
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

/// The Euclidean rhythm generator.
pub const KIND: RhythmKind = RhythmKind {
    id: "euclid",
    name: "Euclidean rhythm",
    description: "Bjorklund's even spread of pulses over steps, with rotation and an evenly \
                  spread accent output.",
    params: &PARAMS,
    outputs: &OUTPUTS,
    build,
};

fn build() -> Box<dyn Rhythm> {
    Box::new(Euclid::new())
}

/// The lowest `steps` bits set.
const fn mask(steps: usize) -> u64 {
    if steps >= 64 {
        u64::MAX
    } else {
        (1u64 << steps) - 1
    }
}

/// A run of steps: bit `i` of `bits` is step `i`, `len` steps long.
type Group = (u64, u32);

const fn join(first: Group, second: Group) -> Group {
    (first.0 | (second.0 << first.1), first.1 + second.1)
}

/// Bjorklund's algorithm: `pulses` hits spread over `steps` (up to 64).
/// Bit `i` is set when step `i` is a hit; the first step always is when
/// there is any hit at all.
#[must_use]
pub fn bjorklund(steps: usize, pulses: usize) -> u64 {
    let steps = steps.min(64);
    if steps == 0 || pulses == 0 {
        return 0;
    }
    if pulses >= steps {
        return mask(steps);
    }
    // Start with a group per hit and a group per rest, then keep dealing
    // the rests onto the hits until at most one group is left over.
    let mut front = [(0u64, 0u32); 64];
    let mut back = [(0u64, 0u32); 64];
    let mut front_len = pulses;
    let mut back_len = steps - pulses;
    for group in front.iter_mut().take(front_len) {
        *group = (1, 1);
    }
    for group in back.iter_mut().take(back_len) {
        *group = (0, 1);
    }
    while back_len > 1 {
        let paired = front_len.min(back_len);
        let mut rest = [(0u64, 0u32); 64];
        let (source, source_len) = if front_len > paired {
            (&front, front_len)
        } else {
            (&back, back_len)
        };
        let rest_len = source_len - paired;
        rest[..rest_len].copy_from_slice(&source[paired..source_len]);
        for (group, extra) in front.iter_mut().zip(back.iter()).take(paired) {
            *group = join(*group, *extra);
        }
        front_len = paired;
        back[..rest_len].copy_from_slice(&rest[..rest_len]);
        back_len = rest_len;
    }
    let mut pattern = (0u64, 0u32);
    for group in front[..front_len].iter().chain(&back[..back_len]) {
        pattern = join(pattern, *group);
    }
    pattern.0
}

/// `pattern` of `steps` steps moved `by` steps later, wrapping round.
#[must_use]
pub const fn rotate(pattern: u64, steps: usize, by: usize) -> u64 {
    if steps == 0 || steps > 64 {
        return pattern;
    }
    let by = by % steps;
    if by == 0 {
        return pattern;
    }
    ((pattern << by) | (pattern >> (steps - by))) & mask(steps)
}

/// The Euclidean generator.
#[derive(Debug, Clone)]
pub struct Euclid {
    knobs: Knobs<4>,
    inputs: Inputs,
    hits: u64,
    accents: u64,
    steps: usize,
    next: usize,
    hit_now: bool,
    accent_now: bool,
}

impl Default for Euclid {
    fn default() -> Self {
        Self::new()
    }
}

impl Euclid {
    /// A generator at the default settings.
    #[must_use]
    pub fn new() -> Self {
        let mut euclid = Self {
            knobs: Knobs::new(&PARAMS),
            inputs: Inputs::default(),
            hits: 0,
            accents: 0,
            steps: 1,
            next: 0,
            hit_now: false,
            accent_now: false,
        };
        euclid.rebuild();
        euclid
    }

    /// Recompute the hit and accent patterns from the knobs.
    fn rebuild(&mut self) {
        let steps = self.knobs.count(STEPS).clamp(1, MAX_STEPS);
        let pulses = self.knobs.count(PULSES).min(steps);
        self.steps = steps;
        self.hits = rotate(bjorklund(steps, pulses), steps, self.knobs.count(ROTATE));
        let accented = bjorklund(pulses, self.knobs.count(ACCENTS).min(pulses));
        let mut accents = 0;
        let mut ordinal = 0;
        for step in 0..steps {
            if self.hits & (1 << step) != 0 {
                if accented & (1 << ordinal) != 0 {
                    accents |= 1 << step;
                }
                ordinal += 1;
            }
        }
        self.accents = accents;
    }

    /// The hit pattern: bit `i` is step `i`.
    #[must_use]
    pub const fn pattern(&self) -> u64 {
        self.hits
    }

    /// The accent pattern: bit `i` is step `i`.
    #[must_use]
    pub const fn accent_pattern(&self) -> u64 {
        self.accents
    }
}

impl Stepper for Euclid {
    fn tick(&mut self, clock_rise: bool, clock_high: bool) -> [f32; MAX_OUTPUTS] {
        if clock_rise {
            let step = self.next % self.steps;
            self.next = (step + 1) % self.steps;
            self.hit_now = self.hits & (1 << step) != 0;
            self.accent_now = self.accents & (1 << step) != 0;
        }
        [
            gate(clock_high && self.hit_now),
            gate(clock_high && self.accent_now),
            0.0,
            0.0,
        ]
    }

    fn restart(&mut self) {
        self.next = 0;
        self.hit_now = false;
        self.accent_now = false;
    }
}

impl Rhythm for Euclid {
    /// Euclid counts clock edges only, so the rate needs no sizing.
    fn prepare(&mut self, _sample_rate: f32) {
        self.restart();
    }

    fn reset(&mut self) {
        self.restart();
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if self.knobs.set(index, value) {
            self.rebuild();
        }
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

    fn show(pattern: u64, steps: usize) -> String {
        (0..steps)
            .map(|step| if pattern & (1 << step) != 0 { 'x' } else { '.' })
            .collect()
    }

    #[test]
    fn toussaints_tables() {
        let cases = [
            (16, 5, "x..x..x..x..x..."),
            (8, 3, "x..x..x."),
            (8, 5, "x.xx.xx."),
            (13, 5, "x..x.x..x.x.."),
            (12, 7, "x.xx.x.xx.x."),
            (9, 4, "x.x.x.x.."),
            (7, 3, "x.x.x.."),
            (4, 3, "xxx."),
            (16, 4, "x...x...x...x..."),
            (24, 11, "x..x.x.x.x.x..x.x.x.x.x."),
            (7, 7, "xxxxxxx"),
            (5, 0, "....."),
        ];
        for (steps, pulses, want) in cases {
            assert_eq!(
                show(bjorklund(steps, pulses), steps),
                want,
                "E({pulses},{steps})"
            );
        }
    }

    #[test]
    fn rotation_moves_the_pattern_later() {
        let pattern = bjorklund(16, 5);
        assert_eq!(show(rotate(pattern, 16, 1), 16), ".x..x..x..x..x..");
        assert_eq!(show(rotate(pattern, 16, 3), 16), "...x..x..x..x..x");
        assert_eq!(rotate(pattern, 16, 16), pattern);
    }

    #[test]
    fn accents_fall_evenly_on_the_hits() {
        let mut euclid = Euclid::new();
        // 5 in 16 with 2 accents: hits 0 and 2 of the five.
        assert_eq!(show(euclid.accent_pattern(), 16), "x.....x.........");
        euclid.set_param(ACCENTS, 0.0);
        assert_eq!(euclid.accent_pattern(), 0);
        euclid.set_param(PULSES, 40.0);
        assert_eq!(euclid.pattern(), mask(16));
    }
}
