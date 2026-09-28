//! Tom-tom: a tuned drum whose pitch falls as it dies.
//!
//! The drum-machine toms are a resonator (808) or a swept oscillator (909)
//! with a little noise for the skin. This is a damped mode for the head's
//! fundamental plus its second mode, 1.59 times higher and quieter, which
//! dies sooner. A real tom's pitch drops over the note because a hard
//! strike stretches the head and raises its tension, and the tension falls
//! back as the vibration dies. The bend knob models that directly: the
//! pitch rises by up to three quarters of an octave in proportion to how
//! loudly the head is still moving, so it slides down with the decay (and
//! more on an accent). The noise knob adds a burst of lowpassed noise, the
//! stick on the skin.
//!
//! The range knob picks the drum (low, mid or high, as the drum machines
//! labelled them); tune moves it by up to an octave either way.

use kazoo_fx::dsp::{Noise, OnePole};
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Mode, Params, hit, ratio, sane_rate};
use crate::{Voice, VoiceKind};

const RANGE: usize = 0;
const TUNE: usize = 1;
const DECAY: usize = 2;
const BEND: usize = 3;
const NOISE: usize = 4;

static PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "range",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["low", "mid", "high"],
        },
    },
    ParamSpec {
        name: "tune",
        min: -12.0,
        max: 12.0,
        default: 0.0,
        unit: "st",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "decay",
        min: 0.1,
        max: 2.0,
        default: 0.45,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "bend",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "noise",
        min: 0.0,
        max: 1.0,
        default: 0.2,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The tom-tom.
pub const KIND: VoiceKind = VoiceKind {
    id: "tom",
    name: "Tom-tom",
    description: "Low, mid or high tom: a tuned head whose pitch falls with its tension as it \
                  dies, and a touch of stick noise.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Tom::new())
}

/// The resting pitch of each range, in hertz.
pub const RANGE_HZ: [f32; 3] = [90.0, 130.0, 190.0];

/// The tom-tom voice.
#[derive(Debug, Clone)]
pub struct Tom {
    params: Params<5>,
    rate: f32,
    finish: Finish,
    active: bool,
    level: f32,
    bend_scale: f32,
    modes: [Mode; 2],
    /// How loudly the head still moves, 1 at the strike.
    motion: Decay,
    skin: Decay,
    noise: Noise,
    skin_filter: OnePole,
}

impl Default for Tom {
    fn default() -> Self {
        Self::new()
    }
}

impl Tom {
    /// A tom at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut tom = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            level: 0.0,
            bend_scale: 1.0,
            modes: [Mode::default(); 2],
            motion: Decay::default(),
            skin: Decay::default(),
            noise: Noise::new(0x544F_4D53),
            skin_filter: OnePole::default(),
        };
        tom.prepare(48_000.0);
        tom
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        self.level = strike_level(velocity, accent);
        self.bend_scale = if accent { 1.3 } else { 1.0 } * velocity;
        self.modes[0].strike(self.level * 0.8);
        self.modes[1].strike(self.level * 0.15);
        self.motion.strike(1.0);
        self.skin.strike(self.level);
    }

    /// The resting pitch, in hertz.
    fn resting_hz(&self) -> f32 {
        let range = self.params.step_index(RANGE).min(RANGE_HZ.len() - 1);
        RANGE_HZ[range] * ratio(self.params.get(TUNE))
    }
}

impl Circuit for Tom {
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
        let decay = self.params.get(DECAY);
        self.motion.set_time(decay * 0.6, self.rate);
        let octaves = 0.75 * self.params.get(BEND) * self.bend_scale * self.motion.tick();
        let hz = self.resting_hz() * octaves.exp2();
        self.modes[0].tune(hz, decay, self.rate);
        self.modes[1].tune(hz * 1.59, decay * 0.5, self.rate);
        let body = self.modes[0].tick() + self.modes[1].tick();
        let skin = self.skin_filter.lowpass(self.noise.sample()) * self.skin.tick();
        if self.modes[0].energy() < 1.0e-10 && self.skin.is_done() {
            self.active = false;
        }
        skin.mul_add(self.params.get(NOISE) * 1.5, body)
    }

    fn silence(&mut self) {
        self.active = false;
        for mode in &mut self.modes {
            mode.clear();
        }
        self.motion.clear();
        self.skin.clear();
        self.skin_filter.reset();
    }
}

impl Voice for Tom {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.params.prepare(self.rate);
        self.finish.prepare(self.rate);
        self.skin.set_time(0.04, self.rate);
        self.skin_filter.set_cutoff(4_000.0, self.rate);
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
