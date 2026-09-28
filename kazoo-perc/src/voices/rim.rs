//! Rimshot and claves: short, hard, woody knocks.
//!
//! **Rimshot.** The TR-808 rim shot rings two bridged-T resonators, at
//! about 455 Hz and 1.67 kHz, for a few milliseconds, highpasses the sum
//! and drives it into a transistor stage hard enough to clip. That clipping
//! is most of its sound: a hollow, square-edged tock. Here the two modes
//! ring for the decay knob, pass a highpass at 300 Hz and are driven into
//! the saturator; snap drives it harder and adds the stick's click.
//!
//! **Clave.** Two hardwood sticks struck together ring almost as a single
//! pure mode near 2.5 kHz (the 808 clave is one resonator there). One mode
//! with a faint second partial at 2.76 times its pitch (the next mode of a
//! free bar), unclipped; snap adds a little click.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Mode, Params, Svf, hit, ratio, sane_rate, saturate};
use crate::{Voice, VoiceKind};

const MODEL: usize = 0;
const TUNE: usize = 1;
const DECAY: usize = 2;
const SNAP: usize = 3;

static PARAMS: [ParamSpec; 4] = [
    ParamSpec {
        name: "model",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["rimshot", "clave"],
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
        min: 0.005,
        max: 0.2,
        default: 0.03,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "snap",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The rimshot and clave.
pub const KIND: VoiceKind = VoiceKind {
    id: "rim",
    name: "Rimshot and clave",
    description: "808 rimshot, two resonators clipped into a hollow tock, or a pure ringing \
                  hardwood clave.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Rim::new())
}

/// The rimshot's two resonators, in hertz before tuning.
const RIM_HZ: [f32; 2] = [455.0, 1_667.0];

/// The clave's mode and its faint second partial, in hertz before tuning.
const CLAVE_HZ: [f32; 2] = [2_500.0, 2_500.0 * 2.76];

/// The rimshot and clave voice.
#[derive(Debug, Clone)]
pub struct Rim {
    params: Params<4>,
    rate: f32,
    finish: Finish,
    active: bool,
    clave: bool,
    modes: [Mode; 2],
    click: Decay,
    noise: Noise,
    highpass: Svf,
}

impl Default for Rim {
    fn default() -> Self {
        Self::new()
    }
}

impl Rim {
    /// A rimshot at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut rim = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            clave: false,
            modes: [Mode::default(); 2],
            click: Decay::default(),
            noise: Noise::new(0x5249_4D53),
            highpass: Svf::default(),
        };
        rim.prepare(48_000.0);
        rim
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        self.clave = self.params.step_index(MODEL) == 1;
        let level = strike_level(velocity, accent);
        let gains = if self.clave { [0.8, 0.08] } else { [0.6, 0.45] };
        for (mode, gain) in self.modes.iter_mut().zip(gains) {
            mode.strike(level * gain);
        }
        self.click.strike(level);
    }
}

impl Circuit for Rim {
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
        let tune = ratio(self.params.get(TUNE));
        let decay = self.params.get(DECAY);
        let snap = self.params.get(SNAP);
        let (hz, rings) = if self.clave {
            (CLAVE_HZ, [1.2, 0.4])
        } else {
            (RIM_HZ, [1.0, 0.7])
        };
        let mut ring = 0.0;
        for ((mode, hz), share) in self.modes.iter_mut().zip(hz).zip(rings) {
            mode.tune(hz * tune, decay * share, self.rate);
            ring += mode.tick();
        }
        let click = self.noise.sample() * self.click.tick() * snap;
        let out = if self.clave {
            click.mul_add(0.2, ring)
        } else {
            self.highpass.set(300.0 * tune, 0.7, self.rate);
            let tock = self.highpass.process(click.mul_add(0.4, ring)).high;
            saturate(tock * 3.0f32.mul_add(snap, 1.5)) * 0.8
        };
        if self.modes.iter().all(|mode| mode.energy() < 1.0e-10) && self.click.is_done() {
            self.active = false;
        }
        out
    }

    fn silence(&mut self) {
        self.active = false;
        for mode in &mut self.modes {
            mode.clear();
        }
        self.click.clear();
        self.highpass.clear();
    }
}

impl Voice for Rim {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.params.prepare(self.rate);
        self.finish.prepare(self.rate);
        self.click.set_time(0.003, self.rate);
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
