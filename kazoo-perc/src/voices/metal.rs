//! Tuned metal: gongs, bells and plates by frequency modulation.
//!
//! John Chowning found that a sine wave modulating another at an
//! inharmonic ratio (1 : 1.4, say) gives the clangorous, unrelated partials
//! of struck metal, and that letting the modulation depth fall faster than
//! the loudness gives a bell's bright strike and mellow hum. This is that
//! instrument: a carrier at `tune`, a modulator at `ratio` times it, and a
//! modulation index that starts at `index` and falls over 40% of the
//! decay while the amplitude falls over all of it.
//!
//! - **Bloom** is how a large gong or tam-tam behaves: struck, it first
//!   sounds dull and low, then the energy cascades into higher partials and
//!   the sound blooms before it fades. Bloom delays the index, so it swells
//!   in over up to a second and a half.
//! - **Ring** blends in a ring modulator: the carrier multiplied by a
//!   second sine at 2.76 times its pitch (the second mode of a free bar),
//!   which adds sum and difference tones, the harsh edge of a struck plate.
//!
//! The index is held under the point where the sidebands would pass
//! 0.45 of the sample rate (by Carson's rule, the sidebands of note reach
//! about `(index + 1)` times the modulator frequency past the carrier), so
//! high, bright settings stay clean rather than alias.

use std::f32::consts::TAU;

use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Params, hit, sane_rate};
use crate::{Voice, VoiceKind};

const TUNE: usize = 0;
const RATIO: usize = 1;
const INDEX: usize = 2;
const DECAY: usize = 3;
const BLOOM: usize = 4;
const RING: usize = 5;

static PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "tune",
        min: 30.0,
        max: 2_000.0,
        default: 110.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "ratio",
        min: 0.5,
        max: 8.0,
        default: 1.41,
        unit: "",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "index",
        min: 0.0,
        max: 10.0,
        default: 4.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "decay",
        min: 0.1,
        max: 12.0,
        default: 3.0,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "bloom",
        min: 0.0,
        max: 1.0,
        default: 0.2,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "ring",
        min: 0.0,
        max: 1.0,
        default: 0.25,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The tuned metal.
pub const KIND: VoiceKind = VoiceKind {
    id: "metal",
    name: "Tuned metal",
    description: "Chowning FM bells and gongs with an index that falls faster than the ring, \
                  a tam-tam bloom and a ring-modulated edge.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Metal::new())
}

/// The ring modulator's partner, as a ratio of the carrier.
const RING_RATIO: f32 = 2.76;

/// The tuned metal voice.
#[derive(Debug, Clone)]
pub struct Metal {
    params: Params<6>,
    rate: f32,
    finish: Finish,
    active: bool,
    carrier: f32,
    modulator: f32,
    partner: f32,
    amp: Decay,
    brightness: Decay,
    /// 1 at the strike, falling to 0: what bloom holds the index back by.
    held: Decay,
}

impl Default for Metal {
    fn default() -> Self {
        Self::new()
    }
}

impl Metal {
    /// A gong at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut metal = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            carrier: 0.0,
            modulator: 0.0,
            partner: 0.0,
            amp: Decay::default(),
            brightness: Decay::default(),
            held: Decay::default(),
        };
        metal.prepare(48_000.0);
        metal
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        let level = strike_level(velocity, accent);
        self.amp.strike(level * 0.8);
        self.brightness.strike(if accent { 1.25 } else { 1.0 });
        self.held.strike(1.0);
    }
}

impl Circuit for Metal {
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
        let tune = self.params.get(TUNE);
        let ratio = self.params.get(RATIO);
        let decay = self.params.get(DECAY);
        let bloom = self.params.get(BLOOM);
        self.amp.set_time(decay, self.rate);
        self.brightness.set_time(decay * 0.4, self.rate);
        self.held.set_time(bloom.mul_add(1.5, 0.001), self.rate);

        let modulator_hz = tune * ratio;
        let ceiling = (0.45f32.mul_add(self.rate, -tune) / modulator_hz - 1.0).max(0.0);
        let swell = bloom.mul_add(-self.held.tick(), 1.0);
        let index = (self.params.get(INDEX) * self.brightness.tick() * swell).min(ceiling);

        let fm = index
            .mul_add((TAU * self.modulator).sin(), TAU * self.carrier)
            .sin();
        let ringed = (TAU * self.carrier).sin() * (TAU * self.partner).sin();
        let ring = self.params.get(RING);
        let voice = ring.mul_add(ringed - fm, fm);

        self.carrier = (self.carrier + tune / self.rate).fract();
        self.modulator = (self.modulator + modulator_hz / self.rate).fract();
        self.partner = (self.partner + tune * RING_RATIO / self.rate).fract();
        let out = voice * self.amp.tick();
        if self.amp.is_done() {
            self.active = false;
        }
        out
    }

    fn silence(&mut self) {
        self.active = false;
        self.carrier = 0.0;
        self.modulator = 0.0;
        self.partner = 0.0;
        self.amp.clear();
        self.brightness.clear();
        self.held.clear();
    }
}

impl Voice for Metal {
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
            Hit::Strike(velocity) => self.strike(velocity, accent),
            Hit::Choke => self.finish.choke(),
            Hit::Ignore => {}
        }
    }

    fn process(&mut self, out: &mut [f32]) {
        run(self, out);
    }
}
