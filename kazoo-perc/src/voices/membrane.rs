//! A physically modelled drum head: a circular membrane, mode by mode.
//!
//! A stretched circular membrane vibrates in modes, each a pattern of
//! nodal circles and diameters. Mode (m, n) has m nodal diameters and n
//! nodal circles, and its frequency is proportional to `j(m, n)`, the n-th
//! zero of the Bessel function `J_m`; that is why a drum's overtones are not
//! harmonic (1, 1.59, 2.14, 2.30, 2.65, ...). This voice rings the 24
//! lowest modes (m from 0 to 5, n from 1 to 4) as damped sinusoids.
//!
//! - **Tension** sets the fundamental, mode (0, 1), in hertz; a membrane's
//!   pitch goes as the square root of its tension.
//! - **Strike** is where the stick lands, from the centre (0) toward the
//!   rim. Each mode is excited in proportion to its shape at that point,
//!   `J_m(j(m, n) r)`: a centre strike excites only the circular modes and
//!   sounds a round thud, while striking toward the rim wakes the others
//!   for a ringing, complex tone. The sound is picked up at a fixed point
//!   off-centre, weighted the same way. A strike gives the head velocity,
//!   so each mode's amplitude also falls as one over its frequency.
//! - **Damping** is how long the head rings (from about four seconds down
//!   to a dead thump), with the upper modes always dying sooner, as air
//!   and the head's own losses take more from them.
//! - **Material** runs from a soft calf skin (upper modes heavily damped)
//!   to a stiff, synthetic or metal-like head: less damping of the upper
//!   modes and a little bending stiffness, which stretches them sharp.
//! - **Hardness** is the stick: a soft mallet stays in contact longer and
//!   so cannot excite high modes (a lowpass on the strike, from 200 Hz to
//!   16 kHz).
//! - **Bend** is tension modulation: a hard strike stretches the head, so
//!   every mode starts sharp (up to 40%) and falls back as the head's
//!   motion dies, the pitch glide of real toms and timpani.
//!
//! Strikes add to the head's motion as real ones do, so rolls build up.

use kazoo_fx::{Curve, ParamSpec};

use super::{Circuit, run, strike_level, wake};
use crate::parts::{Decay, Finish, Hit, Mode, Params, hit, sane_rate};
use crate::{Voice, VoiceKind};

const TENSION: usize = 0;
const STRIKE: usize = 1;
const DAMPING: usize = 2;
const MATERIAL: usize = 3;
const HARDNESS: usize = 4;
const BEND: usize = 5;

static PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "tension",
        min: 40.0,
        max: 1_000.0,
        default: 110.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "strike",
        min: 0.0,
        max: 0.95,
        default: 0.35,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "damping",
        min: 0.0,
        max: 1.0,
        default: 0.35,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "material",
        min: 0.0,
        max: 1.0,
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
    ParamSpec {
        name: "bend",
        min: 0.0,
        max: 1.0,
        default: 0.25,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The drum head.
pub const KIND: VoiceKind = VoiceKind {
    id: "membrane",
    name: "Drum head",
    description: "A circular membrane rung mode by mode: tension, where it is struck, damping, \
                  head material, stick hardness and tension bend.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Voice> {
    Box::new(Membrane::new())
}

/// How many modes ring.
const MODES: usize = 24;

/// Each mode as (m, j(m, n)): its nodal diameters and the Bessel zero that
/// sets its frequency.
const SHAPES: [(u32, f64); MODES] = [
    (0, 2.404_826),
    (0, 5.520_078),
    (0, 8.653_728),
    (0, 11.791_534),
    (1, 3.831_706),
    (1, 7.015_587),
    (1, 10.173_468),
    (1, 13.323_692),
    (2, 5.135_622),
    (2, 8.417_244),
    (2, 11.619_841),
    (2, 14.795_952),
    (3, 6.380_162),
    (3, 9.761_023),
    (3, 13.015_201),
    (3, 16.223_466),
    (4, 7.588_342),
    (4, 11.064_709),
    (4, 14.372_537),
    (4, 17.615_966),
    (5, 8.771_484),
    (5, 12.338_604),
    (5, 15.700_174),
    (5, 18.980_134),
];

/// Where the sound is picked up: radius and angle from the strike.
const PICKUP_RADIUS: f64 = 0.55;
const PICKUP_ANGLE: f64 = 0.7;

/// Retune the modes every this many samples.
const RETUNE_EVERY: u32 = 16;

/// The Bessel function of the first kind, `J_m(x)`, summed from its power
/// series in double precision (ample for the `x` up to 19 used here).
#[must_use]
pub fn bessel_j(m: u32, x: f64) -> f64 {
    let half = x / 2.0;
    let mut term = 1.0;
    for k in 1..=m {
        term *= half / f64::from(k);
    }
    let mut sum = term;
    let step = -half * half;
    for k in 1..96u32 {
        term *= step / (f64::from(k) * f64::from(k + m));
        sum += term;
        if term.abs() < 1.0e-18 {
            break;
        }
    }
    sum
}

/// The drum head voice.
#[derive(Debug, Clone)]
pub struct Membrane {
    params: Params<6>,
    rate: f32,
    finish: Finish,
    active: bool,
    modes: [Mode; MODES],
    /// How strongly the pickup hears each mode.
    pickup: [f32; MODES],
    /// How loudly the head still moves, for the tension bend.
    motion: Decay,
    countdown: u32,
}

impl Default for Membrane {
    fn default() -> Self {
        Self::new()
    }
}

impl Membrane {
    /// A drum head at the default settings, prepared for 48 kHz.
    #[must_use]
    pub fn new() -> Self {
        let mut pickup = [0.0; MODES];
        for (weight, (m, zero)) in pickup.iter_mut().zip(SHAPES) {
            let angle = (f64::from(m) * PICKUP_ANGLE).cos();
            *weight = (bessel_j(m, zero * PICKUP_RADIUS) * angle) as f32;
        }
        let mut membrane = Self {
            params: Params::new(&PARAMS),
            rate: 48_000.0,
            finish: Finish::new(),
            active: false,
            modes: [Mode::default(); MODES],
            pickup,
            motion: Decay::default(),
            countdown: 0,
        };
        membrane.prepare(48_000.0);
        membrane
    }

    /// Each mode's frequency as a ratio of the fundamental, stretched by
    /// the head's stiffness.
    fn ratio(&self, zero: f64) -> f32 {
        let plain = (zero / SHAPES[0].1) as f32;
        let material = self.params.get(MATERIAL);
        let stiffness = material * material * 0.01;
        plain * (stiffness * plain).mul_add(plain, 1.0).sqrt() / (1.0 + stiffness).sqrt()
    }

    /// The fundamental's ringing time, from the damping knob.
    fn ring(&self) -> f32 {
        // 4 s undamped down to 60 ms fully damped, evenly in ratio.
        let damping = self.params.get(DAMPING);
        0.06 * (4.0f32 / 0.06).powf(1.0 - damping)
    }

    fn strike(&mut self, velocity: f32, accent: bool) {
        wake(self);
        if self.finish.continue_strike() {
            self.silence();
        }
        let level = strike_level(velocity, accent);
        let where_struck = f64::from(self.params.get(STRIKE));
        let hardness = self.params.get(HARDNESS) + if accent { 0.15 } else { 0.0 };
        let corner = 200.0 * 80.0f32.powf(hardness.min(1.0));
        let fundamental = self.params.get(TENSION);
        let mut gains = [0.0f32; MODES];
        let mut total = 0.0;
        for ((gain, (m, zero)), pickup) in gains.iter_mut().zip(SHAPES).zip(self.pickup) {
            let ratio = self.ratio(zero);
            let shape = bessel_j(m, zero * where_struck) as f32;
            let hz = fundamental * ratio;
            let mallet = 1.0 / (hz / corner).mul_add(hz / corner, 1.0);
            *gain = shape * pickup * mallet / ratio;
            total += gain.abs();
        }
        let scale = level * 0.9 / total.max(1.0e-3);
        for (mode, gain) in self.modes.iter_mut().zip(gains) {
            mode.strike(gain * scale);
        }
        let motion = (self.motion.level() + level).min(1.5);
        self.motion.strike(motion);
        self.countdown = 0;
        self.active = true;
    }

    fn retune(&mut self) {
        let ring = self.ring();
        let material = self.params.get(MATERIAL);
        let loss = 1.5f32.mul_add(1.0 - material, 0.15);
        let bend = 0.4 * self.params.get(BEND) * self.motion.level();
        let fundamental = self.params.get(TENSION) * (1.0 + bend);
        self.motion.set_time(ring * 0.7, self.rate);
        let ratios: [f32; MODES] = std::array::from_fn(|index| self.ratio(SHAPES[index].1));
        for (mode, ratio) in self.modes.iter_mut().zip(ratios) {
            let t60 = ring / loss.mul_add(ratio - 1.0, 1.0);
            mode.tune(fundamental * ratio, t60, self.rate);
        }
    }
}

impl Circuit for Membrane {
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
            if energy < 1.0e-10 {
                self.silence();
                return 0.0;
            }
        }
        self.countdown -= 1;
        self.motion.tick();
        self.modes.iter_mut().map(Mode::tick).sum()
    }

    fn silence(&mut self) {
        self.active = false;
        for mode in &mut self.modes {
            mode.clear();
        }
        self.motion.clear();
        self.countdown = 0;
    }
}

impl Voice for Membrane {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bessel_zeros_are_zeros() {
        for (m, zero) in SHAPES {
            assert!(bessel_j(m, zero).abs() < 1.0e-5, "J_{m}({zero})");
        }
        assert!((bessel_j(0, 0.0) - 1.0).abs() < 1.0e-12);
        assert!(bessel_j(3, 0.0).abs() < 1.0e-12);
    }
}
