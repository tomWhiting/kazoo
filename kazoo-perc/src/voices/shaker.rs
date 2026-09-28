//! Shakers: maracas, cabasa, tambourine and sleigh bells, as particles.
//!
//! This is Perry Cook's `PhISEM` model (physically informed stochastic event
//! modelling). A shaker is a few dozen beads in a shell. Shaking puts
//! energy into the system, which then bleeds away. At every moment each
//! bead has a small chance of hitting the shell or another bead, and the
//! more energy the system holds, the louder each collision. A collision
//! is a tiny burst of noise that dies within a few milliseconds. The
//! bursts add up and ring the shell's resonance (the gourd of a maraca, the
//! frame of a tambourine), and in the jingled instruments they also strike
//! small metal resonators, each retuned a little at random on every hit,
//! because no two jingles are alike or strike the same way twice.
//!
//! - `grains` is how many beads there are: a few rattle individually, a
//!   hundred make a smooth hiss.
//! - `decay` is how long the shake's energy lasts.
//! - `tone` moves every resonance up or down.
//!
//! Each trigger is one shake: it throws the beads against the shell at
//! once (a first collision on the very sample) and adds energy to whatever
//! is still moving, so fast triggers build up a continuous shake. Silence with no trigger
//! is exact: without energy nothing collides.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Mode, Params, Svf, hit, ratio, sane_rate, t60_coeff};
use crate::{Voice, VoiceKind};

const MODEL: usize = 0;
const GRAINS: usize = 1;
const DECAY: usize = 2;
const TONE: usize = 3;

static PARAMS: [ParamSpec; 4] = [
    ParamSpec {
        name: "model",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["maracas", "cabasa", "tambourine", "sleighbells"],
        },
    },
    ParamSpec {
        name: "grains",
        min: 1.0,
        max: 128.0,
        default: 25.0,
        unit: "",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "decay",
        min: 0.05,
        max: 2.0,
        default: 0.25,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "tone",
        min: -12.0,
        max: 12.0,
        default: 0.0,
        unit: "st",
        curve: Curve::Linear,
    },
];

/// The shakers.
pub const KIND: VoiceKind = VoiceKind {
    id: "shaker",
    name: "Shaker",
    description: "Maracas, cabasa, tambourine or sleigh bells from colliding particles (Cook's \
                  PhISEM): every trigger is a shake.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Shaker::new())
}

/// How one instrument is built.
#[derive(Debug, Clone, Copy)]
struct Recipe {
    /// Collisions per second for each bead at full energy.
    rate: f32,
    /// How fast one collision's noise dies (seconds to -60 dB).
    click: f32,
    /// The shell's resonance: centre, Q and level.
    shell_hz: f32,
    shell_q: f32,
    shell_gain: f32,
    /// The jingles' resonances, in hertz (0 for none).
    jingles: [f32; 4],
    /// How long a jingle rings.
    jingle_ring: f32,
    /// How far each jingle is retuned at random on each hit.
    jitter: f32,
    /// How hard a collision strikes the jingles.
    jingle_gain: f32,
}

/// Maracas, cabasa, tambourine, sleigh bells: after Cook's measurements.
const RECIPES: [Recipe; 4] = [
    Recipe {
        rate: 21.5,
        click: 0.006,
        shell_hz: 3_200.0,
        shell_q: 4.0,
        shell_gain: 1.0,
        jingles: [0.0; 4],
        jingle_ring: 0.1,
        jitter: 0.0,
        jingle_gain: 0.0,
    },
    Recipe {
        rate: 40.0,
        click: 0.008,
        shell_hz: 3_000.0,
        shell_q: 1.0,
        shell_gain: 1.0,
        jingles: [0.0; 4],
        jingle_ring: 0.1,
        jitter: 0.0,
        jingle_gain: 0.0,
    },
    Recipe {
        rate: 21.5,
        click: 0.006,
        shell_hz: 2_300.0,
        shell_q: 5.0,
        shell_gain: 0.5,
        jingles: [5_600.0, 8_100.0, 6_900.0, 0.0],
        jingle_ring: 0.15,
        jitter: 0.05,
        jingle_gain: 0.25,
    },
    Recipe {
        rate: 21.5,
        click: 0.009,
        shell_hz: 2_500.0,
        shell_q: 2.0,
        shell_gain: 0.15,
        jingles: [2_500.0, 5_300.0, 6_500.0, 8_300.0],
        jingle_ring: 0.3,
        jitter: 0.03,
        jingle_gain: 0.2,
    },
];

/// The most energy repeated shakes can store.
const MAX_ENERGY: f32 = 2.0;

/// The shaker voice.
#[derive(Debug, Clone)]
pub struct Shaker {
    params: Params<4>,
    rate: f32,
    finish: Finish,
    active: bool,
    energy: Decay,
    /// The level of the collision noise now.
    sound: f32,
    noise: Noise,
    shell: Svf,
    jingles: [Mode; 4],
}

impl Default for Shaker {
    fn default() -> Self {
        Self::new()
    }
}

impl Shaker {
    /// A shaker at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut shaker = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            energy: Decay::default(),
            sound: 0.0,
            noise: Noise::new(0x5348_414B),
            shell: Svf::default(),
            jingles: [Mode::default(); 4],
        };
        shaker.prepare(48_000.0);
        shaker
    }

    fn recipe(&self) -> Recipe {
        RECIPES[self.params.step_index(MODEL).min(RECIPES.len() - 1)]
    }

    fn shake(&mut self, velocity: f32, accent: bool) {
        wake(self);
        if self.finish.continue_strike() {
            self.silence();
        }
        self.active = true;
        let added = self.energy.level() + strike_level(velocity, accent);
        self.energy.strike(added.min(MAX_ENERGY));
        // The shake itself throws the beads against the shell at once.
        self.collide(self.energy.level());
    }

    /// One collision in a system holding `energy`: a burst of noise, and a
    /// knock on every jingle, each retuned a little at random.
    fn collide(&mut self, energy: f32) {
        let recipe = self.recipe();
        let tone = ratio(self.params.get(TONE));
        let bump = energy * 1.2 / self.params.get(GRAINS).sqrt();
        self.sound += bump;
        for (mode, hz) in self.jingles.iter_mut().zip(recipe.jingles) {
            if hz > 0.0 {
                let nudge = recipe.jitter.mul_add(self.noise.sample(), 1.0);
                mode.tune(hz * tone * nudge, recipe.jingle_ring, self.rate);
                let side = if self.noise.sample() < 0.0 { -1.0 } else { 1.0 };
                mode.strike(bump * recipe.jingle_gain * side);
            }
        }
    }

    /// A random number from 0 up to 1.
    fn chance(&mut self) -> f32 {
        self.noise.sample().mul_add(0.5, 0.5)
    }
}

impl Circuit for Shaker {
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
        let recipe = self.recipe();
        let grains = self.params.get(GRAINS);
        let tone = ratio(self.params.get(TONE));
        self.energy.set_time(self.params.get(DECAY), self.rate);
        let energy = self.energy.tick();

        // Does a bead hit something this sample?
        let odds = grains * recipe.rate / self.rate;
        if energy > 0.0 && self.chance() < odds {
            self.collide(energy);
        }
        let rattle = self.noise.sample() * self.sound;
        self.sound *= t60_coeff(recipe.click, self.rate);

        self.shell
            .set(recipe.shell_hz * tone, recipe.shell_q, self.rate);
        let mut out = self.shell.process(rattle).band * recipe.shell_gain;
        let mut ringing = 0.0;
        for mode in &mut self.jingles {
            out += mode.tick();
            ringing += mode.energy();
        }
        if energy <= 0.0 && self.sound < 1.0e-6 && ringing < 1.0e-10 {
            self.active = false;
        }
        out * 2.0
    }

    fn silence(&mut self) {
        self.active = false;
        self.energy.clear();
        self.sound = 0.0;
        self.shell.clear();
        for mode in &mut self.jingles {
            mode.clear();
        }
    }
}

impl Voice for Shaker {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.params.prepare(self.rate);
        self.finish.prepare(self.rate);
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
            Hit::Strike(velocity) => self.shake(velocity, accent),
            Hit::Choke => self.finish.choke(),
            Hit::Ignore => {}
        }
    }

    fn process(&mut self, out: &mut [f32]) {
        run(self, out);
    }
}
