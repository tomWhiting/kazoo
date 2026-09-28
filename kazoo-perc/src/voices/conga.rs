//! Congas and bongos: a small, tight, hand-struck membrane.
//!
//! A hand drum's open tone is dominated by the head's fundamental, with the
//! next two membrane modes (1.59 and 2.14 times higher) colouring it; they
//! ring as damped modes here. The palm pushes into the head as it lands,
//! so the pitch starts about 8% high and settles within 15 ms, the little
//! upward "bloop" of a conga.
//!
//! - **Slap** moves from the open tone toward a slap: the fingers crack the
//!   edge, so the upper modes come up and a short burst of highpassed skin
//!   noise cuts through.
//! - **Mute** is the other hand pressing the head: it damps the ring and
//!   stretches the head slightly, raising the pitch a touch.
//!
//! The size knob picks the drum, from the small bongo to the tumba.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Mode, Params, Svf, hit, ratio, sane_rate};
use crate::{Voice, VoiceKind};

const SIZE: usize = 0;
const TUNE: usize = 1;
const DECAY: usize = 2;
const SLAP: usize = 3;
const MUTE: usize = 4;

static PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "size",
        min: 0.0,
        max: 4.0,
        default: 3.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["bongo high", "bongo low", "quinto", "conga", "tumba"],
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
        min: 0.05,
        max: 1.5,
        default: 0.35,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "slap",
        min: 0.0,
        max: 1.0,
        default: 0.2,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mute",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Linear,
    },
];

/// Congas and bongos.
pub const KIND: VoiceKind = VoiceKind {
    id: "conga",
    name: "Conga and bongo",
    description: "Hand-struck membrane from bongo to tumba: open tone with a palm bloop, a \
                  slap that cracks the edge, and a muting hand.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Conga::new())
}

/// The open-tone pitch of each drum, in hertz.
pub const SIZE_HZ: [f32; 5] = [480.0, 330.0, 290.0, 230.0, 180.0];

/// The membrane's first three mode ratios.
const RATIOS: [f32; 3] = [1.0, 1.59, 2.14];

/// How long each mode rings, as a share of the decay.
const RINGS: [f32; 3] = [1.0, 0.6, 0.4];

/// The conga voice.
#[derive(Debug, Clone)]
pub struct Conga {
    params: Params<5>,
    rate: f32,
    finish: Finish,
    active: bool,
    modes: [Mode; 3],
    palm: Decay,
    crack: Decay,
    noise: Noise,
    skin: Svf,
}

impl Default for Conga {
    fn default() -> Self {
        Self::new()
    }
}

impl Conga {
    /// A conga at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut conga = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            modes: [Mode::default(); 3],
            palm: Decay::default(),
            crack: Decay::default(),
            noise: Noise::new(0x434F_4E47),
            skin: Svf::default(),
        };
        conga.prepare(48_000.0);
        conga
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        let level = strike_level(velocity, accent);
        let slap = self.params.get(SLAP);
        let gains = [0.8, slap.mul_add(0.4, 0.2), slap.mul_add(0.35, 0.1)];
        for (mode, gain) in self.modes.iter_mut().zip(gains) {
            mode.strike(level * gain);
        }
        self.palm.strike(1.0);
        self.crack.strike(level * slap);
    }

    /// The open-tone pitch, in hertz.
    fn open_hz(&self) -> f32 {
        let size = self.params.step_index(SIZE).min(SIZE_HZ.len() - 1);
        SIZE_HZ[size] * ratio(self.params.get(TUNE))
    }
}

impl Circuit for Conga {
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
        let mute = self.params.get(MUTE);
        let decay = self.params.get(DECAY) * 0.8f32.mul_add(-mute, 1.0);
        let press = 0.08f32.mul_add(self.palm.tick(), 0.06f32.mul_add(mute, 1.0));
        let hz = self.open_hz() * press;
        let mut body = 0.0;
        for ((mode, ratio), ring) in self.modes.iter_mut().zip(RATIOS).zip(RINGS) {
            mode.tune(hz * ratio, decay * ring, self.rate);
            body += mode.tick();
        }
        self.skin.set(2_500.0, 0.7, self.rate);
        let crack = self.skin.process(self.noise.sample()).high * self.crack.tick();
        if self.modes[0].energy() < 1.0e-10 && self.crack.is_done() {
            self.active = false;
        }
        body + crack
    }

    fn silence(&mut self) {
        self.active = false;
        for mode in &mut self.modes {
            mode.clear();
        }
        self.palm.clear();
        self.crack.clear();
        self.skin.clear();
    }
}

impl Voice for Conga {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.params.prepare(self.rate);
        self.finish.prepare(self.rate);
        self.palm.set_time(0.015, self.rate);
        self.crack.set_time(0.012, self.rate);
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
