//! `plate`: the Dattorro plate reverb.
//!
//! The model is the one Jon Dattorro published in *Effect Design, Part 1:
//! Reverberator and Other Filters* (J. Audio Eng. Soc., 1997), itself a
//! description of the Lexicon-style plate programmes of the 1980s. Every
//! delay length and tap below is his, given at his 29 761 Hz rate and
//! scaled to ours (and by the size knob).
//!
//! The signal path:
//!
//! 1. Pre-delay, then a one-pole "bandwidth" lowpass and a bass cut.
//! 2. Four Schroeder allpasses in series (the input diffusers), which smear
//!    a click into a dense wash before it reaches the tank.
//! 3. The tank: two halves joined in a figure eight, each feeding the
//!    other. Each half is a modulated allpass (its delay swept by a slow
//!    sine, the two halves in quadrature, which keeps the tail from ringing
//!    metallic), a delay, a one-pole damping lowpass, the decay gain, a
//!    second allpass, a second delay and the decay gain again.
//! 4. The output: each side sums seven taps read from inside the tank's
//!    delays and allpasses, with Dattorro's signs, so left and right are
//!    built from different parts of the same tank and come out wide and
//!    uncorrelated.
//!
//! The high cut sets both the input bandwidth filter and the tank's
//! damping, so a lower setting darkens the tail more the longer it rings, as
//! a real plate's treble dies first. Both are first-order lowpasses matched
//! to the analogue response (see `matched`), so the treble decays the same
//! at 44.1 kHz and 192 kHz. The low cut is a 12 dB-per-octave
//! Butterworth highpass on the way in (plates were always bass-cut: a
//! booming plate muddies everything).
//!
//! The decay knob is a true RT60 in the mid band (as quoted for real rooms
//! and plates) with the high cut open. The tank is one loop through all
//! eight of its delays with four decay gains on the way round, so each gain
//! is set to `10^(-3 T_loop / (4 RT60))`, recomputed as the size moves
//! (which changes `T_loop`). The damping then takes a little more on every
//! pass above the high cut: that is its job, and with the high cut pulled
//! right down to 1 kHz it shortens the mid band too (a 2.8 s decay rings
//! about 2 s there).
//!
//! The size knob stretches every delay and tap together (the modulation's
//! sweep stays the same size). A delay line that is stretched while it
//! holds sound bends that sound's pitch, so the stretch moves no faster
//! than 2% of real time: a size change bends the tail by at most a third
//! of a semitone, like a tape machine's wow, and never clicks or zips. The
//! pre-delay moves under the same limit.

use std::sync::Arc;

use crate::dsp::{Phasor, flush};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

use super::kit::{
    CONTROL, Knobs, MAX_SLEW, Slew, TapAllpass, ceiling, clean, decay_gain, equal_power, frames,
    silence, sine, usable_rate, widen,
};
use super::line::{Kernel, Line, MIN_SINC_READ};
use super::matched::{self, Biquad, FirstOrder};

/// Dattorro's sample rate: every length below is in samples at this rate.
const DATTORRO_RATE: f32 = 29_761.0;
/// The input diffusers' lengths and coefficients.
const DIFFUSER_LENGTHS: [f32; 4] = [142.0, 107.0, 379.0, 277.0];
const DIFFUSER_GAINS: [f32; 4] = [0.75, 0.75, 0.625, 0.625];
/// Each tank half: modulated allpass, delay, allpass, delay.
const TANK: [[f32; 4]; 2] = [
    [672.0, 4_453.0, 1_800.0, 3_720.0],
    [908.0, 4_217.0, 2_656.0, 3_163.0],
];
/// The modulated allpasses' coefficient ("decay diffusion 1", negated as
/// in the paper's figure).
const DECAY_DIFFUSION_1: f32 = -0.70;
/// The peak sweep of the modulated allpasses with the `mod` knob full up
/// (Dattorro's own 16 samples sits at about 0.7). The knob works as a
/// square law, so its lower half gives fine control of the subtle depths
/// a clean source needs.
const EXCURSION: f32 = 32.0;
/// The whole loop, all eight tank delays.
const TANK_LOOP: f32 = 672.0 + 4_453.0 + 1_800.0 + 3_720.0 + 908.0 + 4_217.0 + 2_656.0 + 3_163.0;
/// The output taps' gain: Dattorro's 0.6, trimmed so that sustained noise
/// comes out of the plate about as loud as it went in at the defaults.
const OUTPUT_LEVEL: f32 = 0.47;
/// The largest size, which sets the buffer lengths.
const MAX_SIZE: f32 = 2.0;
/// The longest pre-delay, in seconds.
const MAX_PREDELAY: f32 = 0.25;

/// One output tap: which tank element, how far in, and its sign.
#[derive(Debug, Clone, Copy)]
enum Element {
    /// The first delay of a half.
    Delay1(usize),
    /// The second allpass of a half.
    Allpass2(usize),
    /// The second delay of a half.
    Delay2(usize),
}

const LEFT_TAPS: [(Element, f32, f32); 7] = [
    (Element::Delay1(1), 266.0, 1.0),
    (Element::Delay1(1), 2_974.0, 1.0),
    (Element::Allpass2(1), 1_913.0, -1.0),
    (Element::Delay2(1), 1_996.0, 1.0),
    (Element::Delay1(0), 1_990.0, -1.0),
    (Element::Allpass2(0), 187.0, -1.0),
    (Element::Delay2(0), 1_066.0, -1.0),
];

const RIGHT_TAPS: [(Element, f32, f32); 7] = [
    (Element::Delay1(0), 353.0, 1.0),
    (Element::Delay1(0), 3_627.0, 1.0),
    (Element::Allpass2(0), 1_228.0, -1.0),
    (Element::Delay2(0), 2_673.0, 1.0),
    (Element::Delay1(1), 2_111.0, -1.0),
    (Element::Allpass2(1), 335.0, -1.0),
    (Element::Delay2(1), 121.0, -1.0),
];

const PREDELAY: usize = 0;
const DECAY: usize = 1;
const SIZE: usize = 2;
const MOD: usize = 3;
const RATE: usize = 4;
const LOW_CUT: usize = 5;
const HIGH_CUT: usize = 6;
const WIDTH: usize = 7;
const MIX: usize = 8;

static PARAMS: [ParamSpec; 9] = [
    ParamSpec {
        name: "predelay",
        min: 0.0,
        max: MAX_PREDELAY,
        default: 0.02,
        unit: "s",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "decay",
        min: 0.2,
        max: 20.0,
        default: 2.8,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "size",
        min: 0.25,
        max: MAX_SIZE,
        default: 1.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mod",
        min: 0.0,
        max: 1.0,
        default: 0.25,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "rate",
        min: 0.05,
        max: 5.0,
        default: 0.8,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "lowcut",
        min: 20.0,
        max: 1_000.0,
        default: 80.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "highcut",
        min: 1_000.0,
        max: 20_000.0,
        default: 9_000.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "width",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
];

/// Glide times, seconds, knob by knob.
/// Pre-delay and size move under a speed limit of their own instead.
const GLIDES: [f32; 9] = [0.0, 0.05, 0.0, 0.05, 0.05, 0.03, 0.03, 0.03, 0.03];
/// The longest element the size stretches, which sets its speed limit.
const LONGEST: f32 = 4_453.0;

/// The plate's entry in the catalogue.
pub static KIND: EffectKind = EffectKind {
    id: "plate",
    name: "Plate reverb",
    description: "Dattorro's figure-eight plate: dense, bright and smooth, with a slow shimmer in the tail.",
    params: &PARAMS,
    build: || Box::new(Plate::new()),
};

/// One half of the figure-eight tank.
#[derive(Debug, Clone, Default)]
struct Half {
    allpass1: TapAllpass,
    delay1: Line,
    damping: FirstOrder,
    allpass2: TapAllpass,
    delay2: Line,
}

impl Half {
    fn new(lengths: [f32; 4], most: f32, kernel: &Arc<Kernel>) -> Self {
        let room = |length: f32, extra: f32| (length.mul_add(most, extra)) as usize + 8;
        let sweep = EXCURSION * most / MAX_SIZE;
        Self {
            allpass1: TapAllpass::new(room(lengths[0], sweep), kernel),
            delay1: Line::new(room(lengths[1], 0.0), kernel),
            damping: FirstOrder::default(),
            allpass2: TapAllpass::new(room(lengths[2], 0.0), kernel),
            delay2: Line::new(room(lengths[3], 0.0), kernel),
        }
    }

    fn clear(&mut self) {
        self.allpass1.clear();
        self.delay1.clear();
        self.damping.reset();
        self.allpass2.clear();
        self.delay2.clear();
    }

    /// Run one sample round this half; returns what it hands to the other.
    fn process(&mut self, input: f32, lengths: [f32; 4], sweep: f32, tank: Tank) -> f32 {
        let mut a = self
            .allpass1
            .process(input, lengths[0] + sweep, DECAY_DIFFUSION_1);
        flush(&mut a);
        let b = self.delay1.read(lengths[1]);
        self.delay1.push(a);
        let b = self.damping.process(f64::from(b)) as f32 * tank.decay;
        let mut c = self.allpass2.process(b, lengths[2], tank.diffusion);
        flush(&mut c);
        let mut d = self.delay2.read(lengths[3]) * tank.decay;
        self.delay2.push(c);
        flush(&mut d);
        d
    }
}

/// The tank's derived coefficients.
#[derive(Debug, Clone, Copy)]
struct Tank {
    decay: f32,
    diffusion: f32,
}

/// The Dattorro plate.
#[derive(Debug, Clone)]
pub struct Plate {
    rate: f32,
    prepared: bool,
    knobs: Knobs<9>,
    predelay: Line,
    predelay_time: Slew,
    size: Slew,
    bandwidth: FirstOrder,
    low_cut: Biquad,
    diffusers: [TapAllpass; 4],
    halves: [Half; 2],
    handoff: [f32; 2],
    lfo: Phasor,
    tank: Tank,
    countdown: usize,
}

impl Default for Plate {
    fn default() -> Self {
        Self::new()
    }
}

impl Plate {
    /// A plate at the default settings, unprepared.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 0.0,
            prepared: false,
            knobs: Knobs::new(&PARAMS),
            predelay: Line::default(),
            predelay_time: Slew::new(MIN_SINC_READ, MAX_SLEW),
            size: Slew::new(1.0, 0.0),
            bandwidth: FirstOrder::default(),
            low_cut: Biquad::default(),
            diffusers: Default::default(),
            halves: Default::default(),
            handoff: [0.0; 2],
            lfo: Phasor::default(),
            tank: Tank {
                decay: 0.5,
                diffusion: 0.5,
            },
            countdown: 0,
        }
    }

    /// Samples per Dattorro sample at this size.
    fn scale(&self, size: f32) -> f32 {
        size * self.rate / DATTORRO_RATE
    }

    /// Recompute the tank's gains and the filters from the gliding knobs.
    fn update(&mut self) {
        let scale = self.scale(self.size.value());
        let loop_seconds = TANK_LOOP * scale / self.rate;
        let decay = decay_gain(0.25 * loop_seconds, self.knobs.get(DECAY));
        self.tank = Tank {
            decay,
            diffusion: (decay + 0.15).clamp(0.25, 0.5),
        };
        let high =
            matched::lowpass_first_order(f64::from(self.knobs.get(HIGH_CUT)), f64::from(self.rate));
        self.bandwidth.set(high);
        for half in &mut self.halves {
            half.damping.set(high);
        }
        self.low_cut.set(matched::highpass(
            f64::from(self.knobs.get(LOW_CUT)),
            std::f64::consts::FRAC_1_SQRT_2,
            f64::from(self.rate),
        ));
    }

    /// Sum one side's taps.
    fn taps(&self, taps: &[(Element, f32, f32); 7], scale: f32) -> f32 {
        taps.iter().fold(0.0, |sum, &(element, at, sign)| {
            let at = at * scale;
            let value = match element {
                Element::Delay1(h) => self.halves[h].delay1.read(at),
                Element::Allpass2(h) => self.halves[h].allpass2.tap(at),
                Element::Delay2(h) => self.halves[h].delay2.read(at),
            };
            sign.mul_add(value, sum)
        })
    }

    /// One stereo frame.
    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.knobs.step();
        if self.countdown == 0 {
            self.update();
            self.countdown = CONTROL;
        }
        self.countdown -= 1;
        self.size.set(self.knobs.get(SIZE));
        let size = self.size.next();
        let scale = self.scale(size);
        self.predelay_time
            .set((self.knobs.get(PREDELAY) * self.rate).max(MIN_SINC_READ));
        let predelay = self.predelay_time.next();

        let (left, right) = (clean(left), clean(right));
        self.predelay.push(0.5 * (left + right));
        let delayed = self.predelay.read(predelay);
        let mut x = self.bandwidth.process(f64::from(delayed)) as f32;
        x = self.low_cut.process(f64::from(x)) as f32;
        for (diffuser, (&length, &gain)) in self
            .diffusers
            .iter_mut()
            .zip(DIFFUSER_LENGTHS.iter().zip(&DIFFUSER_GAINS))
        {
            x = diffuser.process(x, length * scale, gain);
        }

        let phase = self.lfo.next(self.knobs.get(RATE), self.rate);
        let amount = self.knobs.get(MOD);
        let depth = EXCURSION * amount * amount * self.rate / DATTORRO_RATE;
        let sweeps = [depth * sine(phase), depth * sine(phase + 0.25)];
        let tank = self.tank;
        let [into_left, into_right] = [x + self.handoff[1], x + self.handoff[0]];
        let lengths = |half: usize| TANK[half].map(|length| length * scale);
        let out_left = self.halves[0].process(into_left, lengths(0), sweeps[0], tank);
        let out_right = self.halves[1].process(into_right, lengths(1), sweeps[1], tank);
        self.handoff = [out_left, out_right];

        let wet_left = OUTPUT_LEVEL * self.taps(&LEFT_TAPS, scale);
        let wet_right = OUTPUT_LEVEL * self.taps(&RIGHT_TAPS, scale);
        let (wet_left, wet_right) = widen(wet_left, wet_right, self.knobs.get(WIDTH));
        let (dry, wet) = equal_power(self.knobs.get(MIX));
        (
            dry.mul_add(left, wet * ceiling(wet_left)),
            dry.mul_add(right, wet * ceiling(wet_right)),
        )
    }
}

impl Effect for Plate {
    fn prepare(&mut self, sample_rate: f32) {
        let Some(rate) = usable_rate(sample_rate) else {
            self.prepared = false;
            return;
        };
        self.rate = rate;
        let kernel = Kernel::new(rate);
        let most = MAX_SIZE * rate / DATTORRO_RATE;
        self.predelay = Line::new((MAX_PREDELAY * rate) as usize + 8, &kernel);
        self.diffusers =
            DIFFUSER_LENGTHS.map(|length| TapAllpass::new((length * most) as usize + 8, &kernel));
        self.halves = [
            Half::new(TANK[0], most, &kernel),
            Half::new(TANK[1], most, &kernel),
        ];
        self.knobs.prepare(rate, &GLIDES);
        // Size moves the longest line by at most MAX_SLEW samples a sample.
        self.size
            .set_speed(MAX_SLEW * DATTORRO_RATE / (LONGEST * rate));
        self.size.snap(self.knobs.get(SIZE));
        self.predelay_time
            .snap((self.knobs.get(PREDELAY) * rate).max(MIN_SINC_READ));
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        self.predelay.clear();
        self.bandwidth.reset();
        self.low_cut.reset();
        for diffuser in &mut self.diffusers {
            diffuser.clear();
        }
        for half in &mut self.halves {
            half.clear();
        }
        self.handoff = [0.0; 2];
        self.lfo.set(0.0);
        self.countdown = 0;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        self.knobs.set(index, value);
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], mut output: [&mut [f32]; 2]) {
        if !self.prepared {
            silence(&mut output);
            return;
        }
        let count = frames(input, &mut output);
        let [out_left, out_right] = output;
        for n in 0..count {
            let (l, r) = self.frame(input[0][n], input[1][n]);
            out_left[n] = l;
            out_right[n] = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::{
        self, RATE as TEST_RATE, build, impulse, lowpassed, rms, rt60, run_mono,
    };

    #[test]
    fn the_plate_keeps_the_contract() {
        testing::contract("plate");
    }

    #[test]
    fn the_plate_is_the_same_at_every_rate_with_the_treble_open() {
        testing::rates_agree_with("plate", &[("highcut", 20_000.0)]);
    }

    #[test]
    fn the_treble_decays_the_same_at_every_rate() {
        let treble = |rate: f32| {
            let mut plate = testing::build_at(
                "plate",
                rate,
                &[("mix", 1.0), ("predelay", 0.0), ("highcut", 9_000.0)],
            );
            let mut click = vec![0.0f32; (4.5 * rate) as usize];
            click[0] = 1.0;
            let (left, right) = run_mono(plate.as_mut(), &click);
            let mono: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l + r).collect();
            let mut high = testing::butterworth(8_000.0, &[0.541_196_1, 1.306_563], true, rate);
            let band = testing::filtered(&mono, &mut high);
            // The measurement helper counts time at the test rate; scale it.
            testing::rt60(&band) * TEST_RATE / rate
        };
        let reference = treble(48_000.0);
        for rate in [44_100.0, 192_000.0] {
            let measured = treble(rate);
            assert!(
                (measured / reference - 1.0).abs() < 0.05,
                "{rate} Hz: {measured} s against {reference} s"
            );
        }
    }

    #[test]
    fn the_decay_knob_is_a_true_rt60() {
        for decay in [1.0, 2.5, 6.0] {
            let mut plate = build(
                "plate",
                &[
                    ("decay", decay),
                    ("mix", 1.0),
                    ("highcut", 20_000.0),
                    ("lowcut", 20.0),
                    ("predelay", 0.0),
                ],
            );
            let (left, right) = run_mono(plate.as_mut(), &impulse(decay.mul_add(1.5, 1.0)));
            let mono: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l + r).collect();
            // Measured in the mid band, as reverberation times are quoted.
            let measured = rt60(&lowpassed(&mono, 2_000.0));
            assert!(
                (measured / decay - 1.0).abs() < 0.12,
                "decay {decay} s measured {measured} s"
            );
        }
    }

    #[test]
    fn the_tail_is_stereo_and_dies() {
        let mut plate = build("plate", &[("mix", 1.0)]);
        let (left, right) = run_mono(plate.as_mut(), &impulse(12.0));
        let early = (0.1 * TEST_RATE) as usize..(0.6 * TEST_RATE) as usize;
        let difference: Vec<f32> = left[early.clone()]
            .iter()
            .zip(&right[early.clone()])
            .map(|(l, r)| l - r)
            .collect();
        assert!(rms(&difference) > 0.3 * rms(&left[early]), "not wide");
        let end = left.len() - (TEST_RATE as usize);
        assert!(rms(&left[end..]) < 1e-4);
    }

    #[test]
    fn the_default_modulation_leaves_a_high_tone_clean() {
        // A sustained 5 kHz tone: at the default depth, nothing 40 to 400 Hz
        // either side of it comes within 60 dB (a heavier sweep smears the
        // highs into audible warble on a clean source).
        for rate in [48_000.0f32, 192_000.0] {
            let mut plate = testing::build_at("plate", rate, &[("mix", 1.0)]);
            let tone: Vec<f32> = (0..(3.0 * rate) as usize)
                .map(|n| 0.3 * (std::f32::consts::TAU * 5_000.0 * n as f32 / rate).sin())
                .collect();
            let (left, _) = run_mono(plate.as_mut(), &tone);
            let window = &left[left.len() - rate as usize..];
            let at = |hz: f32| {
                let (mut re, mut im) = (0.0f64, 0.0f64);
                let size = window.len() as f64;
                for (n, &x) in window.iter().enumerate() {
                    let hann =
                        0.5f64.mul_add(-(std::f64::consts::TAU * n as f64 / size).cos(), 0.5);
                    let angle = std::f64::consts::TAU * f64::from(hz) * n as f64 / f64::from(rate);
                    re = (f64::from(x) * hann).mul_add(angle.cos(), re);
                    im = (f64::from(x) * hann).mul_add(angle.sin(), im);
                }
                re.hypot(im)
            };
            let carrier = at(5_000.0);
            for step in 0..90 {
                let offset = (step * 4 + 40) as f32;
                for side in [-1.0f32, 1.0] {
                    let level = 20.0 * (at(side.mul_add(offset, 5_000.0)) / carrier).log10();
                    assert!(level < -60.0, "{rate} Hz: {level} dBc at {offset} Hz off");
                }
            }
        }
    }

    #[test]
    fn size_and_predelay_never_bend_the_tail_past_the_speed_limit() {
        use crate::Effect;
        let mut plate = super::Plate::new();
        plate.set_param(super::SIZE, 0.25);
        plate.set_param(super::PREDELAY, 0.0);
        plate.prepare(TEST_RATE);
        plate.set_param(super::SIZE, 2.0);
        plate.set_param(super::PREDELAY, 0.25);
        let context = crate::Context { bpm: 120.0 };
        let input = [0.1f32];
        let (mut left, mut right) = ([0.0f32], [0.0f32]);
        let longest = |plate: &super::Plate| super::LONGEST * plate.scale(plate.size.value());
        let mut last = (longest(&plate), plate.predelay_time.value());
        for _ in 0..(20.0 * TEST_RATE) as usize {
            plate.process(&context, [&input, &input], [&mut left, &mut right]);
            let now = (longest(&plate), plate.predelay_time.value());
            assert!(
                (now.0 - last.0).abs() <= super::MAX_SLEW * 1.05,
                "size moved {}",
                now.0 - last.0
            );
            assert!((now.1 - last.1).abs() <= super::MAX_SLEW * 1.05);
            last = now;
        }
        assert!(plate.size.settled(), "the size never arrived");
    }

    #[test]
    fn sweeping_the_size_does_not_click() {
        // The largest step between samples while the size moves, against
        // the same tail left alone: moving the size adds no click.
        let jump = |moved: bool| {
            let mut plate = build("plate", &[("mix", 1.0)]);
            run_mono(plate.as_mut(), &testing::sine(0.5, 220.0, 0.5));
            if moved {
                plate.set_param(super::SIZE, 0.3);
            }
            let (left, _) = run_mono(plate.as_mut(), &testing::silence(0.5));
            left.windows(2)
                .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()))
        };
        let (moved, still) = (jump(true), jump(false));
        assert!(
            moved <= 1.5f32.mul_add(still, 1e-3),
            "{moved} against {still}"
        );
    }
}
