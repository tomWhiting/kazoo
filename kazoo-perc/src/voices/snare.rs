//! Snare drum: a tuned shell and a rattle of wires.
//!
//! The body is two damped modes, as in the TR-808 snare's pair of bridged-T
//! resonators. Their tuning follows a real drum head: the second mode sits
//! 1.59 times above the first, the ratio of a circular membrane's first
//! two modes, and dies a little sooner. The stick deflects the head as it
//! lands, so both modes start about 12% sharp and settle within 20 ms.
//!
//! The snare wires are white noise through a highpass (the wires do not
//! move air below about 800 Hz) and a lowpass set by the tone knob, on
//! their own decay (snap). Real wires are driven by the bottom head, so the
//! noise is modulated a little by the body's own motion: it buzzes with the
//! drum instead of sitting beside it. Snappy sets how much wire there is.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Mode, Params, Svf, hit, sane_rate};
use crate::{Voice, VoiceKind};

const TUNE: usize = 0;
const DECAY: usize = 1;
const TONE: usize = 2;
const SNAPPY: usize = 3;
const SNAP: usize = 4;

static PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "tune",
        min: 100.0,
        max: 400.0,
        default: 185.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "decay",
        min: 0.05,
        max: 1.0,
        default: 0.18,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "tone",
        min: 1_000.0,
        max: 12_000.0,
        default: 5_000.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "snappy",
        min: 0.0,
        max: 1.0,
        default: 0.6,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "snap",
        min: 0.05,
        max: 1.0,
        default: 0.22,
        unit: "s",
        curve: Curve::Log,
    },
];

/// The snare drum.
pub const KIND: VoiceKind = VoiceKind {
    id: "snare",
    name: "Snare drum",
    description: "Two tuned head modes with a stick-deflection pitch drop, and noise wires \
                  that buzz with the body.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Snare::new())
}

/// The ratio of a circular membrane's second mode to its first.
const SECOND_MODE: f32 = 1.59;

/// The snare drum voice.
#[derive(Debug, Clone)]
pub struct Snare {
    params: Params<5>,
    rate: f32,
    finish: Finish,
    active: bool,
    level: f32,
    wire_level: f32,
    modes: [Mode; 2],
    bend: Decay,
    wires: Decay,
    noise: Noise,
    highpass: Svf,
    lowpass: Svf,
}

impl Default for Snare {
    fn default() -> Self {
        Self::new()
    }
}

impl Snare {
    /// A snare at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut snare = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            level: 0.0,
            wire_level: 0.0,
            modes: [Mode::default(); 2],
            bend: Decay::default(),
            wires: Decay::default(),
            noise: Noise::new(0x534E_4152),
            highpass: Svf::default(),
            lowpass: Svf::default(),
        };
        snare.prepare(48_000.0);
        snare
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        self.level = strike_level(velocity, accent);
        self.wire_level = if accent { 1.2 } else { 1.0 };
        self.modes[0].strike(self.level * 0.6);
        self.modes[1].strike(self.level * 0.35);
        self.bend.strike(1.0);
        self.wires.strike(self.level);
    }
}

impl Circuit for Snare {
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
        let hz = self.params.get(TUNE) * 0.12f32.mul_add(self.bend.tick(), 1.0);
        self.modes[0].tune(hz, decay, self.rate);
        self.modes[1].tune(hz * SECOND_MODE, decay * 0.7, self.rate);
        let body = self.modes[0].tick() + self.modes[1].tick();

        self.wires.set_time(self.params.get(SNAP), self.rate);
        self.highpass.set(800.0, 0.7, self.rate);
        self.lowpass.set(self.params.get(TONE), 0.7, self.rate);
        let hiss = self.highpass.process(self.noise.sample()).high;
        let hiss = self.lowpass.process(hiss).low;
        let buzz = 0.35f32.mul_add((body.abs() * 2.0).min(1.0), 0.65);
        let wires = hiss * buzz * self.wires.tick() * self.params.get(SNAPPY) * self.wire_level;

        let energy = self.modes[0].energy() + self.modes[1].energy();
        if energy < 1.0e-10 && self.wires.is_done() {
            self.active = false;
        }
        body + wires * 0.9
    }

    fn silence(&mut self) {
        self.active = false;
        for mode in &mut self.modes {
            mode.clear();
        }
        self.bend.clear();
        self.wires.clear();
        self.highpass.clear();
        self.lowpass.clear();
    }
}

impl Voice for Snare {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.params.prepare(self.rate);
        self.finish.prepare(self.rate);
        self.bend.set_time(0.02, self.rate);
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
