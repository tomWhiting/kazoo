//! Hi-hat: six detuned squares, filtered to a hiss of metal.
//!
//! The TR-808 (and the metal half of the 909) builds its hats from six
//! square-wave oscillators at unrelated frequencies. Summed, they share no
//! harmonic series, so what comes out is a dense clang with no pitch. That
//! is band-passed high and then high-passed higher still, leaving only the
//! shimmer, and shaped by a VCA with a short decay. The squares here are
//! band-limited so the top of the hat does not alias. The metal knob blends
//! the squares with white noise, from pure 808 metal toward the noisier
//! hats of later machines.
//!
//! One voice plays both closed and open hats: the decay knob is how open
//! the hat is (tens of milliseconds closed, up to two seconds open).
//!
//! **Choke.** As on the drum machines, where closed and open hat share one
//! circuit, every new hit cuts off the one before (restarting the circuit
//! and fading the old ring out in about 1.5 ms), so a closed hit after an
//! open one chokes it. A trigger with velocity 0 chokes without a new hit:
//! the ring fades out over about 14 ms. A host can patch another voice's
//! trigger (a pedal, a closed hat on a second hat voice) into that.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, MetalSource, Params, Svf, hit, ratio, sane_rate};
use crate::{Voice, VoiceKind};

const TUNE: usize = 0;
const DECAY: usize = 1;
const TONE: usize = 2;
const METAL: usize = 3;

static PARAMS: [ParamSpec; 4] = [
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
        min: 0.02,
        max: 2.0,
        default: 0.07,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "tone",
        min: 3_000.0,
        max: 14_000.0,
        default: 8_000.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "metal",
        min: 0.0,
        max: 1.0,
        default: 0.85,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The hi-hat.
pub const KIND: VoiceKind = VoiceKind {
    id: "hat",
    name: "Hi-hat",
    description: "Six detuned squares band-passed into 808 metal; decay opens it from closed \
                  to open, and every hit chokes the last.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Hat::new())
}

/// The hi-hat voice.
#[derive(Debug, Clone)]
pub struct Hat {
    params: Params<4>,
    rate: f32,
    finish: Finish,
    active: bool,
    metal: MetalSource,
    noise: Noise,
    band: Svf,
    high: Svf,
    amp: Decay,
}

impl Default for Hat {
    fn default() -> Self {
        Self::new()
    }
}

impl Hat {
    /// A hat at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut hat = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            metal: MetalSource::default(),
            noise: Noise::new(0x4841_5453),
            band: Svf::default(),
            high: Svf::default(),
            amp: Decay::default(),
        };
        hat.prepare(48_000.0);
        hat
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        self.amp.strike(strike_level(velocity, accent));
    }
}

impl Circuit for Hat {
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
        let pitch = ratio(self.params.get(TUNE));
        let tone = self.params.get(TONE);
        let metal = self.params.get(METAL);
        self.amp.set_time(self.params.get(DECAY), self.rate);
        self.band.set(tone, 1.2, self.rate);
        self.high.set(tone * 0.75, 0.7, self.rate);
        let squares = self.metal.next(pitch, self.rate, [1.0; 6]);
        let hiss = self.noise.sample();
        let source = metal.mul_add(squares.mul_add(1.5, -hiss), hiss);
        let shimmer = self.high.process(self.band.process(source).band).high;
        let out = shimmer * self.amp.tick() * 2.0;
        if self.amp.is_done() {
            self.active = false;
        }
        out
    }

    fn silence(&mut self) {
        self.active = false;
        self.metal.clear();
        self.band.clear();
        self.high.clear();
        self.amp.clear();
    }
}

impl Voice for Hat {
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
