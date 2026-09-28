//! Hand clap: several hands, not quite together, in a room.
//!
//! The TR-808 clap is white noise through a bandpass filter, opened by a
//! sawtooth-shaped envelope that fires a few times in quick succession and
//! then lets the last burst ring on, with a second, slower noise path for
//! the room. This follows that circuit. Each burst jumps to full level and
//! falls away within its gap; the gaps are the spread knob, each one
//! nudged a little at random so no two claps are identical (a real group
//! of hands never lands on a grid). The last burst decays over the decay
//! knob. The tail is the room: noise through a lower, wider bandpass that
//! swells in over a few milliseconds and dies over one and a half times
//! the decay.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Params, Svf, hit, numbers, sane_rate};
use crate::{Voice, VoiceKind};

const TONE: usize = 0;
const SPREAD: usize = 1;
const BURSTS: usize = 2;
const DECAY: usize = 3;
const TAIL: usize = 4;

static PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "tone",
        min: 500.0,
        max: 4_000.0,
        default: 1_100.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "spread",
        min: 0.003,
        max: 0.03,
        default: 0.011,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "bursts",
        min: 2.0,
        max: 6.0,
        default: 4.0,
        unit: "",
        curve: Curve::Stepped {
            labels: numbers(2, 6),
        },
    },
    ParamSpec {
        name: "decay",
        min: 0.05,
        max: 2.0,
        default: 0.3,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "tail",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The hand clap.
pub const KIND: VoiceKind = VoiceKind {
    id: "clap",
    name: "Hand clap",
    description: "808-style bursts of bandpassed noise from loosely timed hands, with a room \
                  tail.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Clap::new())
}

/// The hand clap voice.
#[derive(Debug, Clone)]
pub struct Clap {
    params: Params<5>,
    rate: f32,
    finish: Finish,
    active: bool,
    level: f32,
    /// Bursts still to fire after the current one.
    bursts_left: usize,
    /// Samples until the next burst.
    until_next: f32,
    burst: Decay,
    room: Decay,
    swell: Decay,
    noise: Noise,
    hands: Svf,
    walls: Svf,
}

impl Default for Clap {
    fn default() -> Self {
        Self::new()
    }
}

impl Clap {
    /// A clap at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut clap = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            level: 0.0,
            bursts_left: 0,
            until_next: 0.0,
            burst: Decay::default(),
            room: Decay::default(),
            swell: Decay::default(),
            noise: Noise::new(0x434C_4150),
            hands: Svf::default(),
            walls: Svf::default(),
        };
        clap.prepare(48_000.0);
        clap
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        self.level = strike_level(velocity, accent);
        self.bursts_left = self.params.step_index(BURSTS).max(1);
        self.until_next = 0.0;
        self.room.strike(self.level * self.params.get(TAIL));
        self.swell.strike(1.0);
    }

    /// Fire the next burst when its time has come.
    fn schedule(&mut self) {
        if self.bursts_left == 0 {
            return;
        }
        self.until_next -= 1.0;
        if self.until_next > 0.0 {
            return;
        }
        self.bursts_left -= 1;
        let spread = self.params.get(SPREAD);
        let time = if self.bursts_left == 0 {
            self.params.get(DECAY)
        } else {
            spread * 0.9
        };
        self.burst.set_time(time, self.rate);
        self.burst.strike(self.level);
        let nudge = 0.3f32.mul_add(self.noise.sample(), 1.0);
        self.until_next = spread * nudge * self.rate;
    }
}

impl Circuit for Clap {
    fn is_active(&self) -> bool {
        self.active
    }

    fn finish(&mut self) -> &mut Finish {
        &mut self.finish
    }

    fn step_params(&mut self) {
        self.params.step();
    }

    fn snap_params(&mut self) {
        self.params.snap();
    }

    fn render(&mut self) -> f32 {
        self.schedule();
        let tone = self.params.get(TONE);
        self.hands.set(tone, 2.5, self.rate);
        self.walls.set(tone * 0.8, 1.2, self.rate);
        self.room.set_time(self.params.get(DECAY) * 1.5, self.rate);
        let raw = self.noise.sample();
        let hands = self.hands.process(raw).band * self.burst.tick();
        let swell = 1.0 - self.swell.tick();
        let walls = self.walls.process(raw).band * self.room.tick() * swell;
        if self.bursts_left == 0 && self.burst.is_done() && self.room.is_done() {
            self.active = false;
        }
        (hands + walls * 0.6) * 1.3
    }

    fn silence(&mut self) {
        self.active = false;
        self.bursts_left = 0;
        self.burst.clear();
        self.room.clear();
        self.swell.clear();
        self.hands.clear();
        self.walls.clear();
    }
}

impl Voice for Clap {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.params.prepare(self.rate);
        self.finish.prepare(self.rate);
        self.swell.set_time(0.02, self.rate);
        self.silence();
    }

    fn reset(&mut self) {
        self.finish.clear();
        self.silence();
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.params.set(index, value);
    }

    fn trigger(&mut self, velocity: f32, accent: bool) {
        match hit(velocity) {
            Hit::Strike(velocity) => self.strike(velocity, accent),
            Hit::Choke => self.finish.choke(),
            Hit::Ignore => {}
        }
    }

    fn process(&mut self, out: &mut [f32]) {
        run(self, out);
    }
}
