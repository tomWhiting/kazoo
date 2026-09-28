//! Struck bars, bells and bowls: banks of tuned, damped modes.
//!
//! Anything rigid that is struck rings in modes: fixed frequencies, each
//! dying at its own rate. What makes a marimba sound like a marimba and not
//! a bell is where those modes sit, how long each lasts and how strongly
//! each is excited. Each material here is a measured or tuned set of up to
//! eight modes (frequency ratios to the fundamental, ringing times as a
//! share of the decay knob, and loudness):
//!
//! - **marimba**: rosewood bars undercut so the first overtones sit two
//!   octaves and about three octaves and a third up (1 : 4 : 10); a tube
//!   under each bar reinforces the fundamental, and the overtones die fast.
//! - **vibraphone**: aluminium bars tuned 1 : 4 : 10, ringing much longer.
//! - **xylophone**: harder wood tuned 1 : 3, bright and short.
//! - **wood**: a block or slit drum; untuned, close, quickly damped modes.
//! - **glass**: a bowl or wine glass; widely spaced modes that ring on.
//! - **bell**: a church bell's partials: hum, prime, minor-third tierce,
//!   quint, nominal and the upper partials. `tune` is the prime.
//! - **steelpan**: a hammered note area tuned to near-harmonic modes.
//! - **chime**: an untuned free bar or tube, whose modes follow the ideal
//!   beam (1 : 2.76 : 5.40 : 8.93 ...).
//!
//! **Strike** is where the mallet lands along the bar, from the end (0) to
//! the centre (0.5). Each mode is excited in proportion to the shape of an
//! ideal free-free beam's matching mode at that point, so striking at the
//! centre silences the modes that have a node there, just as on a real
//! bar. (For bells and glass it reads as moving from the lip toward the
//! shoulder.) **Hardness** is the mallet: yarn cannot excite high modes (a
//! lowpass on the strike from 200 Hz to 16 kHz), a hard mallet also adds a
//! short knock of contact noise.
//!
//! Strikes add to what is already ringing, as on the real instrument.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Mode, Params, hit, sane_rate};
use crate::{Voice, VoiceKind};

const MATERIAL: usize = 0;
const TUNE: usize = 1;
const DECAY: usize = 2;
const STRIKE: usize = 3;
const HARDNESS: usize = 4;

static PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "material",
        min: 0.0,
        max: 7.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &[
                "marimba",
                "vibraphone",
                "xylophone",
                "wood",
                "glass",
                "bell",
                "steelpan",
                "chime",
            ],
        },
    },
    ParamSpec {
        name: "tune",
        min: 40.0,
        max: 4_000.0,
        default: 440.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "decay",
        min: 0.05,
        max: 10.0,
        default: 1.2,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "strike",
        min: 0.0,
        max: 0.5,
        default: 0.4,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "hardness",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The modal percussion.
pub const KIND: VoiceKind = VoiceKind {
    id: "modal",
    name: "Bars and bells",
    description: "Modal resonator bank: marimba, vibraphone, xylophone, wood, glass, bell, \
                  steelpan or chime, with strike position and mallet hardness.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Modal::new())
}

/// The most modes a material has.
const MODES: usize = 8;

/// One material's modes. A gain of 0 marks a mode it does not have.
#[derive(Debug, Clone, Copy)]
struct Material {
    ratios: [f32; MODES],
    rings: [f32; MODES],
    gains: [f32; MODES],
}

const MATERIALS: [Material; 8] = [
    // Marimba.
    Material {
        ratios: [1.0, 3.99, 9.83, 17.1, 0.0, 0.0, 0.0, 0.0],
        rings: [1.0, 0.35, 0.15, 0.08, 0.0, 0.0, 0.0, 0.0],
        gains: [1.0, 0.35, 0.18, 0.06, 0.0, 0.0, 0.0, 0.0],
    },
    // Vibraphone.
    Material {
        ratios: [1.0, 3.98, 9.96, 17.2, 0.0, 0.0, 0.0, 0.0],
        rings: [1.0, 0.6, 0.35, 0.2, 0.0, 0.0, 0.0, 0.0],
        gains: [1.0, 0.45, 0.2, 0.08, 0.0, 0.0, 0.0, 0.0],
    },
    // Xylophone.
    Material {
        ratios: [1.0, 3.0, 6.1, 9.9, 14.8, 0.0, 0.0, 0.0],
        rings: [1.0, 0.5, 0.3, 0.2, 0.12, 0.0, 0.0, 0.0],
        gains: [1.0, 0.6, 0.3, 0.15, 0.08, 0.0, 0.0, 0.0],
    },
    // Wood.
    Material {
        ratios: [1.0, 2.57, 4.03, 5.6, 7.3, 0.0, 0.0, 0.0],
        rings: [1.0, 0.6, 0.4, 0.3, 0.2, 0.0, 0.0, 0.0],
        gains: [1.0, 0.6, 0.45, 0.3, 0.2, 0.0, 0.0, 0.0],
    },
    // Glass.
    Material {
        ratios: [1.0, 2.32, 4.25, 6.63, 9.38, 0.0, 0.0, 0.0],
        rings: [1.0, 0.8, 0.6, 0.45, 0.3, 0.0, 0.0, 0.0],
        gains: [1.0, 0.5, 0.35, 0.2, 0.12, 0.0, 0.0, 0.0],
    },
    // Bell: hum, prime, tierce, quint, nominal, deciem, undeciem, duodeciem.
    Material {
        ratios: [0.5, 1.0, 1.19, 1.5, 2.0, 2.52, 2.66, 3.01],
        rings: [2.0, 1.2, 1.0, 0.8, 0.7, 0.5, 0.45, 0.4],
        gains: [0.5, 1.0, 0.7, 0.4, 0.8, 0.3, 0.3, 0.25],
    },
    // Steelpan.
    Material {
        ratios: [1.0, 2.0, 3.0, 4.02, 5.05, 0.0, 0.0, 0.0],
        rings: [1.0, 0.7, 0.5, 0.35, 0.25, 0.0, 0.0, 0.0],
        gains: [1.0, 0.7, 0.4, 0.25, 0.15, 0.0, 0.0, 0.0],
    },
    // Chime: the ideal free-free beam.
    Material {
        ratios: [1.0, 2.756, 5.404, 8.933, 13.345, 18.638, 24.81, 31.87],
        rings: [1.0, 0.8, 0.6, 0.45, 0.35, 0.25, 0.2, 0.15],
        gains: [0.6, 1.0, 0.9, 0.7, 0.5, 0.35, 0.25, 0.15],
    },
];

/// The free-free beam's eigenvalues `β L` for its first eight modes.
const BETA: [f64; MODES] = [
    4.730_041, 7.853_205, 10.995_608, 14.137_165, 17.278_760, 20.420_352, 23.561_945, 26.703_538,
];

/// Retune the modes every this many samples.
const RETUNE_EVERY: u32 = 16;

/// The shape of a free-free beam's mode with eigenvalue `beta` at `x`
/// along it (0 and 1 are the ends), scaled to ±1 at the ends.
#[must_use]
pub fn beam_shape(beta: f64, x: f64) -> f64 {
    let sigma = (beta.cosh() - beta.cos()) / (beta.sinh() - beta.sin());
    let bx = beta * x;
    sigma.mul_add(-(bx.sinh() + bx.sin()), bx.cosh() + bx.cos()) / 2.0
}

/// The bars-and-bells voice.
#[derive(Debug, Clone)]
pub struct Modal {
    params: Params<5>,
    rate: f32,
    finish: Finish,
    active: bool,
    material: usize,
    modes: [Mode; MODES],
    knock: Decay,
    knock_level: f32,
    noise: Noise,
    countdown: u32,
}

impl Default for Modal {
    fn default() -> Self {
        Self::new()
    }
}

impl Modal {
    /// A marimba bar at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut modal = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            material: 0,
            modes: [Mode::default(); MODES],
            knock: Decay::default(),
            knock_level: 0.0,
            noise: Noise::new(0x4D4F_4441),
            countdown: 0,
        };
        modal.prepare(48_000.0);
        modal
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        if self.finish.continue_strike() {
            self.silence();
        }
        let material = self.params.step_index(MATERIAL).min(MATERIALS.len() - 1);
        if material != self.material {
            // A different instrument: what rang on the old one stops,
            // faded out by the finish.
            self.finish.restrike();
            for mode in &mut self.modes {
                mode.clear();
            }
            self.material = material;
        }
        let recipe = MATERIALS[material];
        let level = strike_level(velocity, accent);
        let hardness = (self.params.get(HARDNESS) + if accent { 0.15 } else { 0.0 }).min(1.0);
        let corner = 200.0 * 80.0f32.powf(hardness);
        let tune = self.params.get(TUNE);
        let x = f64::from(self.params.get(STRIKE));
        let mut gains = [0.0f32; MODES];
        let mut total = 0.0;
        for (index, gain) in gains.iter_mut().enumerate() {
            let hz = tune * recipe.ratios[index];
            let mallet = 1.0 / (hz / corner).mul_add(hz / corner, 1.0);
            let shape = beam_shape(BETA[index], x) as f32;
            *gain = recipe.gains[index] * shape * mallet;
            total += gain.abs();
        }
        let scale = level * 0.8 / total.max(1.0e-3);
        for (mode, gain) in self.modes.iter_mut().zip(gains) {
            mode.strike(gain * scale);
        }
        self.knock_level = level * hardness * hardness * 0.3;
        self.knock.strike(1.0);
        self.countdown = 0;
        self.active = true;
    }

    fn retune(&mut self) {
        let recipe = MATERIALS[self.material];
        let tune = self.params.get(TUNE);
        let decay = self.params.get(DECAY);
        for ((mode, ratio), (ring, gain)) in self
            .modes
            .iter_mut()
            .zip(recipe.ratios)
            .zip(recipe.rings.into_iter().zip(recipe.gains))
        {
            if gain > 0.0 {
                mode.tune(tune * ratio, decay * ring, self.rate);
            } else {
                mode.tune(0.0, 0.0, self.rate);
            }
        }
    }
}

impl Circuit for Modal {
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
        if self.countdown == 0 {
            self.retune();
            self.countdown = RETUNE_EVERY;
            let energy: f32 = self.modes.iter().map(Mode::energy).sum();
            if energy < 1.0e-10 && self.knock.is_done() {
                self.silence();
                return 0.0;
            }
        }
        self.countdown -= 1;
        let knock = self.noise.sample() * self.knock.tick() * self.knock_level;
        let ring: f32 = self.modes.iter_mut().map(Mode::tick).sum();
        ring + knock
    }

    fn silence(&mut self) {
        self.active = false;
        for mode in &mut self.modes {
            mode.clear();
        }
        self.knock.clear();
        self.countdown = 0;
    }
}

impl Voice for Modal {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.params.prepare(self.rate);
        self.finish.prepare(self.rate);
        self.knock.set_time(0.002, self.rate);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beam_modes_have_their_nodes_where_bars_do() {
        // The first mode's nodes sit 22.4% of the way in from each end.
        assert!(beam_shape(BETA[0], 0.224).abs() < 0.01);
        // The second mode has a node at the centre; the first does not.
        assert!(beam_shape(BETA[1], 0.5).abs() < 1.0e-6);
        assert!(beam_shape(BETA[0], 0.5).abs() > 0.5);
        // Every mode is ±1 at the ends.
        for beta in BETA {
            assert!((beam_shape(beta, 0.0).abs() - 1.0).abs() < 1.0e-6);
        }
    }
}
