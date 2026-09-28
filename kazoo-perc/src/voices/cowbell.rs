//! Cowbell: the TR-808's two squares through a bandpass.
//!
//! The 808 cowbell is two square-wave oscillators at 540 Hz and 800 Hz
//! (the same two found among its hi-hat metal), summed and passed through a
//! bandpass filter, then a VCA whose envelope falls fast at first and then
//! slowly: a bright clank that leaves a hollow ring. The squares here are
//! band-limited. The two-stage envelope is two decays added: a fast one
//! (60 ms, 70% of the level) and the decay knob. Tone moves the bandpass:
//! low for the famous dull 808 clonk, high for a thinner, more metallic
//! agogo-like bell.

use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Params, Svf, hit, ratio, sane_rate, square};
use crate::{Voice, VoiceKind};

const TUNE: usize = 0;
const DECAY: usize = 1;
const TONE: usize = 2;

static PARAMS: [ParamSpec; 3] = [
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
        name: "tone",
        min: 800.0,
        max: 5_000.0,
        default: 2_000.0,
        unit: "Hz",
        curve: Curve::Log,
    },
];

/// The cowbell.
pub const KIND: VoiceKind = VoiceKind {
    id: "cowbell",
    name: "Cowbell",
    description: "The 808's two detuned squares through a bandpass with a clank-then-ring \
                  envelope.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Cowbell::new())
}

/// The two oscillators, in hertz before tuning.
const BELL_HZ: [f32; 2] = [540.0, 800.0];

/// The cowbell voice.
#[derive(Debug, Clone)]
pub struct Cowbell {
    params: Params<3>,
    rate: f32,
    finish: Finish,
    active: bool,
    phases: [f32; 2],
    band: Svf,
    clank: Decay,
    ring: Decay,
}

impl Default for Cowbell {
    fn default() -> Self {
        Self::new()
    }
}

impl Cowbell {
    /// A cowbell at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut cowbell = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            phases: [0.0; 2],
            band: Svf::default(),
            clank: Decay::default(),
            ring: Decay::default(),
        };
        cowbell.prepare(48_000.0);
        cowbell
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        let level = strike_level(velocity, accent);
        self.clank.strike(level * 0.7);
        self.ring.strike(level * 0.3);
    }
}

impl Circuit for Cowbell {
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
        let mut squares = 0.0;
        for (phase, hz) in self.phases.iter_mut().zip(BELL_HZ) {
            squares += square(phase, (hz * tune / self.rate).min(0.45));
        }
        self.band.set(self.params.get(TONE), 2.5, self.rate);
        self.ring.set_time(self.params.get(DECAY), self.rate);
        let bell = self.band.process(squares * 0.5).band;
        let envelope = self.clank.tick() + self.ring.tick();
        if self.clank.is_done() && self.ring.is_done() {
            self.active = false;
        }
        bell * envelope * 1.6
    }

    fn silence(&mut self) {
        self.active = false;
        self.phases = [0.0; 2];
        self.band.clear();
        self.clank.clear();
        self.ring.clear();
    }
}

impl Voice for Cowbell {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.params.prepare(self.rate);
        self.finish.prepare(self.rate);
        self.clank.set_time(0.06, self.rate);
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
