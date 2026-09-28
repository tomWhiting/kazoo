//! Ride and crash cymbals from the same six-square metal as the hats.
//!
//! The TR-808 cymbal splits its metal source into three bands, each with
//! its own VCA: the low band dies quickly, the middle one later and the
//! top one rings longest, which is what makes a cymbal's tail sound
//! brighter as it fades. This keeps that structure (bands at 0.4, 0.7 and
//! 1 times the tone knob, dying at 0.3, 0.6 and 1 times the decay) and adds
//! what a real cymbal does that the 808 did not:
//!
//! - **Ride.** A stick lands on a thick, stiff plate: a short bright tick,
//!   and the bell's two strong inharmonic partials ringing over the wash.
//!   The decay is shaped in two stages, a quick drop from the stick
//!   followed by the long ring.
//! - **Crash.** A thin plate hit on the edge takes a few milliseconds to
//!   bloom into full wash as the energy spreads, then falls fast before its
//!   long tail. More noise in the source roughens it the way a crash
//!   roars.
//!
//! The wash knob adds noise to the metal source, from clean 808 clang to a
//! roaring sheet.

use kazoo_fx::dsp::Noise;
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, MetalSource, Mode, Params, Svf, hit, ratio, sane_rate};
use crate::{Voice, VoiceKind};

const MODEL: usize = 0;
const TUNE: usize = 1;
const DECAY: usize = 2;
const TONE: usize = 3;
const WASH: usize = 4;

static PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "model",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["ride", "crash"],
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
        min: 0.3,
        max: 8.0,
        default: 2.5,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "tone",
        min: 2_000.0,
        max: 12_000.0,
        default: 6_500.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "wash",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The cymbal.
pub const KIND: VoiceKind = VoiceKind {
    id: "cymbal",
    name: "Cymbal",
    description: "Ride or crash: 808 three-band metal with a stick tick and bell partials, or \
                  an edge-struck bloom into roaring wash.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Cymbal::new())
}

/// The bell partials of a ride, in hertz before tuning.
const BELL_HZ: [f32; 2] = [742.0, 1_811.0];

/// How long each band rings, as a share of the decay knob.
const BAND_DECAY: [f32; 3] = [0.3, 0.6, 1.0];

/// Where each band sits, as a share of the tone knob.
const BAND_TONE: [f32; 3] = [0.4, 0.7, 1.0];

/// The cymbal voice.
#[derive(Debug, Clone)]
pub struct Cymbal {
    params: Params<5>,
    rate: f32,
    finish: Finish,
    active: bool,
    crash: bool,
    metal: MetalSource,
    noise: Noise,
    bands: [Svf; 3],
    ring: [Decay; 3],
    /// The fast first stage of the two-stage decay.
    drop: Decay,
    /// 1 at the strike, falling to 0: the crash's bloom and the ride's tick.
    onset: Decay,
    bell: [Mode; 2],
    tick: Svf,
}

impl Default for Cymbal {
    fn default() -> Self {
        Self::new()
    }
}

impl Cymbal {
    /// A cymbal at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut cymbal = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            crash: false,
            metal: MetalSource::default(),
            noise: Noise::new(0x4359_4D42),
            bands: [Svf::default(); 3],
            ring: [Decay::default(); 3],
            drop: Decay::default(),
            onset: Decay::default(),
            bell: [Mode::default(); 2],
            tick: Svf::default(),
        };
        cymbal.prepare(48_000.0);
        cymbal
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        self.crash = self.params.step_index(MODEL) == 1;
        let level = strike_level(velocity, accent);
        let decay = self.params.get(DECAY);
        self.set_times();
        for ring in &mut self.ring {
            ring.strike(level);
        }
        self.drop.strike(1.0);
        let onset_time = if self.crash { 0.012 } else { 0.004 };
        self.onset.set_time(onset_time, self.rate);
        self.onset.strike(1.0);
        if !self.crash {
            let tune = ratio(self.params.get(TUNE));
            for (mode, hz) in self.bell.iter_mut().zip(BELL_HZ) {
                mode.tune(hz * tune, decay * 0.5, self.rate);
                mode.strike(level * 0.12);
            }
        }
    }

    /// Follow the decay knob: each band's ring and the first-stage drop.
    fn set_times(&mut self) {
        let decay = self.params.get(DECAY);
        for (ring, share) in self.ring.iter_mut().zip(BAND_DECAY) {
            ring.set_time(decay * share, self.rate);
        }
        let drop_share = if self.crash { 0.12 } else { 0.06 };
        self.drop.set_time(decay * drop_share, self.rate);
    }
}

impl Circuit for Cymbal {
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
        self.set_times();
        let pitch = ratio(self.params.get(TUNE));
        let tone = self.params.get(TONE);
        let wash = self.params.get(WASH);
        let hiss = self.noise.sample();
        let squares = self.metal.next(pitch, self.rate, [1.0; 6]) * 1.5;
        let roar = if self.crash {
            wash.mul_add(0.5, 0.2)
        } else {
            wash * 0.5
        };
        let source = roar.mul_add(hiss - squares, squares);

        let mut wash_out = 0.0;
        for ((band, ring), share) in self.bands.iter_mut().zip(&mut self.ring).zip(BAND_TONE) {
            band.set(tone * share, 1.4, self.rate);
            wash_out = band.process(source).band.mul_add(ring.tick(), wash_out);
        }
        let onset = self.onset.tick();
        let drop = self.drop.tick();
        let shaped = if self.crash {
            // Blooms in, then a fast fall onto the long tail.
            wash_out * (1.0 - onset) * 0.6f32.mul_add(drop, 0.4)
        } else {
            wash_out * 0.5f32.mul_add(drop, 0.5)
        };
        let extra = if self.crash {
            0.0
        } else {
            self.tick.set(tone, 0.8, self.rate);
            let tick = self.tick.process(hiss).high * onset * 0.8;
            tick + self.bell[0].tick() + self.bell[1].tick()
        };
        if self.ring.iter().all(Decay::is_done) {
            self.active = false;
        }
        shaped.mul_add(1.6, extra)
    }

    fn silence(&mut self) {
        self.active = false;
        self.metal.clear();
        for band in &mut self.bands {
            band.clear();
        }
        for ring in &mut self.ring {
            ring.clear();
        }
        for mode in &mut self.bell {
            mode.clear();
        }
        self.drop.clear();
        self.onset.clear();
        self.tick.clear();
    }
}

impl Voice for Cymbal {
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
