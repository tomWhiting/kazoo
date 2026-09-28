//! Bursts: every trigger becomes a quick run of triggers, a ratchet, a
//! flam, a roll or a bouncing ball.
//!
//! Each rising edge on the clock input starts a burst of `count` gates.
//! The first rises on the very sample of the edge; each following one
//! comes `spacing` later, and each gap is `bounce` times the one before:
//! below 1 the hits crowd together like a dropped ball coming to rest,
//! above 1 they spread out like a slowing roll. Each gate is high for half
//! its gap. With `sync` on clock, the first gap is instead the measured
//! clock period divided by the count, so an unbounced burst fills exactly
//! one clock step (until two edges have been seen the spacing knob is used).
//!
//! The level output is a control voltage holding the level of the latest
//! hit: 1 for the first, falling by `decay` for each one after (with decay
//! 0.25 the hits are 1, 0.75, 0.56, ...). Patched into a VCA or a voice's
//! level it makes the burst die away like a real roll.
//!
//! A new edge in the middle of a burst starts a fresh burst; a reset stops
//! it and drops the level to 0.

use kazoo_fx::{Curve, ParamSpec};

use super::{Inputs, Knobs, Meter, Stepper, drive, gate};
use crate::parts::{numbers, sane_rate};
use crate::{MAX_OUTPUTS, Output, Rhythm, RhythmKind, Signal};

const COUNT: usize = 0;
const SPACING: usize = 1;
const SYNC: usize = 2;
const BOUNCE: usize = 3;
const DECAY: usize = 4;

static PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "count",
        min: 1.0,
        max: 16.0,
        default: 4.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(1, 16),
        },
    },
    ParamSpec {
        name: "spacing",
        min: 0.005,
        max: 1.0,
        default: 0.06,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "sync",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["time", "clock"],
        },
    },
    ParamSpec {
        name: "bounce",
        min: 0.5,
        max: 1.5,
        default: 1.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "decay",
        min: 0.0,
        max: 1.0,
        default: 0.25,
        unit: "",
        curve: Curve::Linear,
    },
];

static OUTPUTS: [Output; 2] = [
    Output {
        name: "gate",
        signal: Signal::Gate,
    },
    Output {
        name: "level",
        signal: Signal::Cv,
    },
];

/// The burst generator.
pub const KIND: RhythmKind = RhythmKind {
    id: "burst",
    name: "Burst",
    description: "Turns each trigger into a ratchet, roll or bouncing ball of gates, with a \
                  falling level output.",
    params: &PARAMS,
    outputs: &OUTPUTS,
    build,
};

fn build() -> Box<dyn Rhythm> {
    Box::new(Burst::new())
}

/// The shortest gap between hits, in samples.
const MIN_GAP: f32 = 2.0;

/// The burst generator.
#[derive(Debug, Clone)]
pub struct Burst {
    knobs: Knobs<5>,
    inputs: Inputs,
    meter: Meter,
    rate: f32,
    hits_left: usize,
    fired: i32,
    /// The gap before the next hit, in samples.
    gap: f32,
    until_next: u32,
    gate_left: u32,
    level: f32,
}

impl Default for Burst {
    fn default() -> Self {
        Self::new()
    }
}

impl Burst {
    /// A burst generator at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut burst = Self {
            knobs: Knobs::new(&PARAMS),
            inputs: Inputs::default(),
            meter: Meter::default(),
            rate: 48_000.0,
            hits_left: 0,
            fired: 0,
            gap: MIN_GAP,
            until_next: 0,
            gate_left: 0,
            level: 0.0,
        };
        burst.prepare(48_000.0);
        burst
    }

    fn start(&mut self) {
        let count = self.knobs.count(COUNT).clamp(1, 16);
        let synced = match (self.knobs.count(SYNC), self.meter.period()) {
            (1, Some(period)) => Some(period as f32 / count as f32),
            _ => None,
        };
        let gap = synced.unwrap_or_else(|| self.knobs.get(SPACING) * self.rate);
        self.gap = gap.max(MIN_GAP);
        self.hits_left = count;
        self.fired = 0;
        self.until_next = 0;
    }

    fn fire(&mut self) {
        self.level = (1.0 - self.knobs.get(DECAY)).powi(self.fired);
        self.fired = self.fired.saturating_add(1);
        self.hits_left -= 1;
        self.gate_left = (self.gap / 2.0).round().max(1.0) as u32;
        self.until_next = self.gap.round() as u32;
        self.gap = (self.gap * self.knobs.get(BOUNCE)).max(MIN_GAP);
    }
}

impl Stepper for Burst {
    fn tick(&mut self, clock_rise: bool, _clock_high: bool) -> [f32; MAX_OUTPUTS] {
        self.meter.tick(clock_rise);
        if clock_rise {
            self.start();
        }
        if self.hits_left > 0 && self.until_next == 0 {
            self.fire();
        }
        let high = self.gate_left > 0;
        self.gate_left = self.gate_left.saturating_sub(1);
        self.until_next = self.until_next.saturating_sub(1);
        [gate(high), self.level, 0.0, 0.0]
    }

    fn restart(&mut self) {
        self.hits_left = 0;
        self.until_next = 0;
        self.gate_left = 0;
        self.level = 0.0;
    }
}

impl Rhythm for Burst {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
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
