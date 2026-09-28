//! Noise percussion: a filtered noise burst that sweeps. Risers, zaps,
//! whooshes, snaps and synthetic hand percussion.
//!
//! White noise through a resonant state-variable filter whose cutoff
//! travels from `from` to `to` over `length`, evenly in pitch (so a sweep
//! over four octaves spends as long in each), under one of three
//! envelopes:
//!
//! - **hit**: full level at once, falling 60 dB over the length. With a
//!   fast downward sweep and high resonance, a laser zap; slow and low,
//!   a whoosh.
//! - **swell**: rising over the length (as the square of time, so it
//!   builds late) and then cut off in 3 ms: the riser before a drop.
//! - **gate**: a 2 ms fade in, full level for the length, a 10 ms fade out.
//!
//! The filter can be lowpass, bandpass or highpass. At high resonance the
//! filter sings at its cutoff, so the sweep becomes a pitch glide; the
//! resonance is capped short of self-oscillation. The noise feeding the
//! lowpass and highpass is turned down as resonance rises so their peak
//! does not jump out.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Params, Svf, hit, sane_rate};
use crate::{Voice, VoiceKind};

const FILTER: usize = 0;
const SHAPE: usize = 1;
const LENGTH: usize = 2;
const FROM: usize = 3;
const TO: usize = 4;
const RESONANCE: usize = 5;

static PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "filter",
        min: 0.0,
        max: 2.0,
        default: 1.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["lowpass", "bandpass", "highpass"],
        },
    },
    ParamSpec {
        name: "shape",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["hit", "swell", "gate"],
        },
    },
    ParamSpec {
        name: "length",
        min: 0.005,
        max: 8.0,
        default: 0.25,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "from",
        min: 20.0,
        max: 18_000.0,
        default: 8_000.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "to",
        min: 20.0,
        max: 18_000.0,
        default: 300.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "resonance",
        min: 0.0,
        max: 0.97,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The noise percussion.
pub const KIND: VoiceKind = VoiceKind {
    id: "noiseperc",
    name: "Noise sweep",
    description: "A noise burst through a sweeping resonant filter: hits, zaps, whooshes and \
                  risers.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(NoisePerc::new())
}

/// The noise percussion voice.
#[derive(Debug, Clone)]
pub struct NoisePerc {
    params: Params<6>,
    rate: f32,
    finish: Finish,
    active: bool,
    level: f32,
    shape: usize,
    /// Samples since the strike.
    elapsed: u32,
    /// The hit envelope, or the fade-out of swell and gate.
    fall: Decay,
    releasing: bool,
    noise: Noise,
    filter: Svf,
}

impl Default for NoisePerc {
    fn default() -> Self {
        Self::new()
    }
}

impl NoisePerc {
    /// A noise hit at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut voice = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            level: 0.0,
            shape: 0,
            elapsed: 0,
            fall: Decay::default(),
            releasing: false,
            noise: Noise::new(0x4E4F_4953),
            filter: Svf::default(),
        };
        voice.prepare(48_000.0);
        voice
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        self.level = strike_level(velocity, accent);
        self.shape = self.params.step_index(SHAPE);
        self.elapsed = 0;
        self.releasing = false;
        self.fall.strike(1.0);
    }

    /// Seconds since the strike.
    fn seconds(&self) -> f32 {
        self.elapsed as f32 / self.rate
    }

    /// The envelope now, from 0 up to 1.
    fn envelope(&mut self, length: f32) -> f32 {
        let time = self.seconds();
        if self.shape == 0 {
            self.fall.set_time(length, self.rate);
            return self.fall.tick();
        }
        if !self.releasing && time >= length {
            self.releasing = true;
            let release = if self.shape == 1 { 0.003 } else { 0.01 };
            self.fall.set_time(release, self.rate);
        }
        let hold = if self.shape == 1 {
            let progress = (time / length).min(1.0);
            progress * progress
        } else {
            (time / 0.002).min(1.0)
        };
        if self.releasing {
            hold * self.fall.tick()
        } else {
            hold
        }
    }
}

impl Circuit for NoisePerc {
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
        let length = self.params.get(LENGTH);
        let from = self.params.get(FROM);
        let to = self.params.get(TO);
        let progress = (self.seconds() / length).min(1.0);
        let cutoff = from * (to / from).powf(progress);
        let resonance = self.params.get(RESONANCE);
        let q = (resonance * resonance).mul_add(39.5, 0.5);
        self.filter.set(cutoff, q, self.rate);
        let envelope = self.envelope(length);
        self.elapsed = self.elapsed.saturating_add(1);

        let filtered = self.filter.process(self.noise.sample());
        let tamed = 1.0 / 0.15f32.mul_add(q, 1.0);
        let out = match self.params.step_index(FILTER) {
            0 => filtered.low * tamed,
            1 => filtered.band * 1.5,
            _ => filtered.high * tamed,
        };
        if self.fall.is_done() {
            self.active = false;
        }
        out * envelope * self.level
    }

    fn silence(&mut self) {
        self.active = false;
        self.fall.clear();
        self.releasing = false;
        self.filter.clear();
    }
}

impl Voice for NoisePerc {
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
