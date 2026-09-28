//! Kick drum, in the two ways the drum machines made one.
//!
//! **808.** The TR-808 kick is a bridged-T filter held just short of
//! oscillation and pinged by the trigger pulse: a resonator, not an
//! oscillator. It is modelled here as one damped sinusoidal mode whose
//! ringing time is the decay knob (the bridged-T's Q). While the trigger
//! pulse is high it shifts the filter's tuning, so the first few
//! milliseconds sit higher and fall to the resting pitch; that is the
//! sweep, an exponential fall. The sweep knob sets how far above the
//! resting pitch it starts (up to two octaves) and, as it rises, how long
//! the fall takes (30 ms to 120 ms to fall silent), from a tight thud to a
//! long electronic boom. A little of
//! the trigger pulse itself leaks through to the output as a soft tick:
//! the click knob.
//!
//! **909.** The TR-909 kick is an oscillator instead: a triangle shaped into
//! a near-sine by a diode shaper (so it carries a trace of odd harmonics),
//! swept down from well above its resting pitch (up to three octaves, over
//! 50 ms to 170 ms as the sweep knob rises) by a fast pitch envelope,
//! through a VCA with its own decay. Its attack is a separate circuit: a
//! short pulse and a burst of lowpassed noise, the beater's slap. Here the
//! click knob sets that attack.
//!
//! Both finish in an output stage that saturates smoothly as it is pushed:
//! the drive knob, which fattens and squares off the body.

use kazoo_fx::dsp::{Noise, OnePole};
use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Mode, Params, hit, sane_rate, saturate};
use crate::{Voice, VoiceKind};

const MODEL: usize = 0;
const TUNE: usize = 1;
const DECAY: usize = 2;
const SWEEP: usize = 3;
const CLICK: usize = 4;
const DRIVE: usize = 5;

static PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "model",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["808", "909"],
        },
    },
    ParamSpec {
        name: "tune",
        min: 30.0,
        max: 120.0,
        default: 50.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "decay",
        min: 0.05,
        max: 3.0,
        default: 0.6,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "sweep",
        min: 0.0,
        max: 1.0,
        default: 0.4,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "click",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "drive",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The kick drum.
pub const KIND: VoiceKind = VoiceKind {
    id: "kick",
    name: "Kick drum",
    description: "808 bridged-T boom or 909 swept oscillator with a beater slap, tunable, \
                  with pitch sweep and a saturating output stage.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Kick::new())
}

/// Level of the body at full velocity, before the output stage.
const BODY: f32 = 0.8;

/// The kick drum voice.
#[derive(Debug, Clone)]
pub struct Kick {
    params: Params<6>,
    rate: f32,
    finish: Finish,
    active: bool,
    model: usize,
    level: f32,
    accent: bool,
    /// The 808 body.
    mode: Mode,
    /// The 909 body: oscillator phase and amplitude.
    phase: f32,
    amp: Decay,
    /// The pitch envelope, 1 at the strike falling to 0.
    pitch: Decay,
    /// The 808 pulse leak or the 909 click pulse.
    pulse: Decay,
    /// The 909 beater noise.
    slap: Decay,
    noise: Noise,
    slap_filter: OnePole,
}

impl Default for Kick {
    fn default() -> Self {
        Self::new()
    }
}

impl Kick {
    /// A kick at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut kick = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            model: 0,
            level: 0.0,
            accent: false,
            mode: Mode::default(),
            phase: 0.0,
            amp: Decay::default(),
            pitch: Decay::default(),
            pulse: Decay::default(),
            slap: Decay::default(),
            noise: Noise::new(0x4B49_434B),
            slap_filter: OnePole::default(),
        };
        kick.prepare(48_000.0);
        kick
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        self.finish.restrike();
        self.silence();
        self.active = true;
        self.accent = accent;
        self.model = self.params.step_index(MODEL);
        self.level = strike_level(velocity, accent) * BODY;
        self.pitch.strike(1.0);
        self.pulse.strike(1.0);
        if self.model == 0 {
            self.mode.strike(self.level);
        } else {
            self.phase = 0.0;
            self.amp.strike(self.level);
            self.slap.strike(1.0);
        }
    }

    /// The pitch now, in hertz, and how many octaves the sweep starts
    /// above it.
    fn pitch_now(&mut self) -> f32 {
        let tune = self.params.get(TUNE);
        let depth = self.params.get(SWEEP) * if self.accent { 1.3 } else { 1.0 };
        let octaves = match self.model {
            0 => 2.0 * depth,
            _ => 3.0 * depth,
        };
        tune * (octaves * self.pitch.tick()).exp2()
    }

    fn render_808(&mut self, hz: f32) -> f32 {
        let decay = self.params.get(DECAY);
        self.mode.tune(hz, decay, self.rate);
        let body = self.mode.tick();
        let tick = self.pulse.tick() * self.params.get(CLICK) * 0.35 * self.level;
        if self.mode.energy() < 1.0e-10 && self.pulse.is_done() {
            self.active = false;
        }
        body + tick
    }

    fn render_909(&mut self, hz: f32) -> f32 {
        // A triangle starting at zero and rising, shaped toward a sine.
        let tri = if self.phase < 0.25 {
            4.0 * self.phase
        } else if self.phase < 0.75 {
            4.0f32.mul_add(-self.phase, 2.0)
        } else {
            4.0f32.mul_add(self.phase, -4.0)
        };
        let shaped = tri * (-0.5f32).mul_add(tri * tri, 1.5);
        self.phase = (self.phase + hz / self.rate).fract();
        let body = shaped * self.amp.tick();
        let click = self.params.get(CLICK) * self.level;
        let pulse = self.pulse.tick() * 0.6;
        let slap = self.slap_filter.lowpass(self.noise.sample()) * self.slap.tick() * 0.9;
        if self.amp.is_done() && self.slap.is_done() && self.pulse.is_done() {
            self.active = false;
        }
        click.mul_add(pulse + slap, body)
    }
}

impl Circuit for Kick {
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
        let sweep = self.params.get(SWEEP);
        let sweep_time = if self.model == 0 {
            0.09f32.mul_add(sweep, 0.03)
        } else {
            0.12f32.mul_add(sweep, 0.05)
        };
        self.pitch.set_time(sweep_time, self.rate);
        self.amp.set_time(decay, self.rate);
        let hz = self.pitch_now();
        let dry = if self.model == 0 {
            self.render_808(hz)
        } else {
            self.render_909(hz)
        };
        let drive = self.params.get(DRIVE);
        saturate(dry * 5.0f32.mul_add(drive, 1.0)) * 0.25f32.mul_add(-drive, 1.0)
    }

    fn silence(&mut self) {
        self.active = false;
        self.mode.clear();
        self.amp.clear();
        self.pitch.clear();
        self.pulse.clear();
        self.slap.clear();
        self.slap_filter.reset();
    }
}

impl Voice for Kick {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = sane_rate(sample_rate);
        self.params.prepare(self.rate);
        self.finish.prepare(self.rate);
        self.pulse.set_time(0.004, self.rate);
        self.slap.set_time(0.025, self.rate);
        self.slap_filter.set_cutoff(6_000.0, self.rate);
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
