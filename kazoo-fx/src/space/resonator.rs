//! `resonator`: a bank of tuned resonators that the input sets ringing.
//!
//! In the spirit of Mutable Instruments' Rings, written from scratch. Feed
//! it drums, a voice, a scraped mic, and it rings out as strings, bars and
//! bells tuned to a note or a chord. Three models, on a stepped switch:
//!
//! - **Modal**: each of four voices is sixteen modes, each mode a decaying
//!   sine: a complex one-pole filter, `s ← r e^{jω} s + g x`, the most
//!   direct model of a single vibrating mode there is (Mathews and Smith's
//!   "phasor filter"). It stays perfectly stable and click-free while its
//!   frequency glides, which a plain biquad does not. The *structure* knob
//!   moves the mode frequencies from a string's harmonic series (`k f0`),
//!   through a stiff string's stretched partials (`k f0 √(1 + B k²)`, the
//!   piano's inharmonicity), to a free bar's (the Euler-Bernoulli beam's
//!   `(β_k / β_1)²`: 1, 2.76, 5.40, 8.93 ...), which rings like a marimba
//!   or a bell. *Position* is where the string or bar is struck: mode `k`
//!   is excited in proportion to `sin(π k p)`, so striking the middle
//!   silences the even modes, as it does on a real string. *Brightness*
//!   tilts both how hard the upper modes are excited and how much faster
//!   they die; *damping* sets the fundamental's decay time.
//! - **Comb**: each voice is a Karplus-Strong string: a delay line one
//!   period long closing through a one-pole lowpass (brightness) and two
//!   first-order allpasses (structure: stiffness, which stretches the
//!   partials sharp). The loop filters' phase delay at the fundamental is
//!   worked out and taken off the delay, so the string is in tune whatever
//!   the filters do, and the loop gain is set from the filters' magnitude
//!   there, so the decay time is the knob's. The input excites the string
//!   through a feedforward comb one pick-position long, the same notch
//!   pattern a real pluck at that point makes.
//! - **Sympathetic**: the same strings, but only the root is played by the
//!   input. The other three are tuned to the chord and hear only the root,
//!   as the sympathetic strings of a sitar or a piano with the pedal down
//!   do, and they ring on longer.
//!
//! *Pitch* is in semitones from middle C; *chord* tunes the four voices to
//! a chord (each voice a few cents off the others, so a unison beats like
//! real strings). Pitch and chord changes glide, and the models crossfade,
//! so nothing clicks. A resonator tuned to the note it is fed can ring very
//! loud; a stereo peak limiter and the family's soft ceiling keep the
//! output under +6 dBFS.

use std::f32::consts::{PI, TAU};

use crate::dsp::{DelayLine, Smoothed, flush};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

use super::kit::{
    CONTROL, Knobs, Limiter, MIN_READ, Ramp, clean, decay_gain, decay_radius, equal_power, frames,
    silence, unchanged, usable_rate,
};

const VOICES: usize = 4;
const MODES: usize = 16;
/// Middle C, the pitch knob's zero.
const MIDDLE_C: f32 = 261.625_58;
/// Each voice's detune, semitones, and level.
const DETUNE: [f32; VOICES] = [0.0, 0.03, -0.03, 0.015];
const VOICE_LEVEL: [f32; VOICES] = [1.0, 0.8, 0.8, 0.7];
/// Each model's output level, set so that sustained noise comes out about
/// as loud as it went in at the default settings (measured below the
/// limiter). A sustained sound makes a resonator ring up far more than a
/// single hit does, so without these the models would jump in level
/// against each other and against the other reverbs.
const MODAL_LEVEL: f32 = 0.038_5;
const COMB_LEVEL: f32 = 0.168;
const SYMPATHETIC_LEVEL: f32 = 0.067_5;
/// Each string's place in the stereo field, -1 left to 1 right.
const STRING_PAN: [f32; VOICES] = [0.0, -0.6, 0.6, -0.2];
/// The stiff string's inharmonicity at the middle of the structure knob.
const STIFFNESS: f32 = 0.004;
/// The chords, as four intervals in semitones.
const CHORDS: [[f32; VOICES]; 10] = [
    [0.0, 0.0, 0.0, 0.0],
    [0.0, 12.0, 0.0, 12.0],
    [0.0, 7.0, 12.0, 19.0],
    [0.0, 5.0, 7.0, 12.0],
    [0.0, 3.0, 7.0, 12.0],
    [0.0, 4.0, 7.0, 12.0],
    [0.0, 3.0, 7.0, 10.0],
    [0.0, 4.0, 7.0, 11.0],
    [0.0, 3.0, 10.0, 14.0],
    [0.0, 4.0, 7.0, 14.0],
];
/// The longest string: the lowest pitch, a semitone of glide below it.
const LOWEST_HZ: f32 = 30.0;

const MODEL: usize = 0;
const PITCH: usize = 1;
const CHORD: usize = 2;
const STRUCTURE: usize = 3;
const BRIGHTNESS: usize = 4;
const DAMPING: usize = 5;
const POSITION: usize = 6;
const MIX: usize = 7;

static PARAMS: [ParamSpec; 8] = [
    ParamSpec {
        name: "model",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["modal", "comb", "sympathetic"],
        },
    },
    ParamSpec {
        name: "pitch",
        min: -36.0,
        max: 24.0,
        default: -12.0,
        unit: "st",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "chord",
        min: 0.0,
        max: 9.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &[
                "unison", "octave", "fifths", "sus4", "minor", "major", "minor 7", "major 7",
                "minor 9", "add 9",
            ],
        },
    },
    ParamSpec {
        name: "structure",
        min: 0.0,
        max: 1.0,
        default: 0.25,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "brightness",
        min: 0.0,
        max: 1.0,
        default: 0.6,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "damping",
        min: 0.0,
        max: 1.0,
        default: 0.4,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "position",
        min: 0.0,
        max: 1.0,
        default: 0.3,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.8,
        unit: "",
        curve: Curve::Linear,
    },
];

const GLIDES: [f32; 8] = [0.0, 0.03, 0.0, 0.05, 0.05, 0.05, 0.05, 0.03];
/// How long a voice takes to glide to a new chord note, and a model
/// crossfade.
const CHORD_GLIDE: f32 = 0.03;
const MODEL_FADE: f32 = 0.05;

/// The resonator's entry in the catalogue.
pub static KIND: EffectKind = EffectKind {
    id: "resonator",
    name: "Resonator",
    description: "Tuned strings, bars and bells rung by the input: modal, comb-string and sympathetic-string models, tuned to a note or a chord.",
    params: &PARAMS,
    build: || Box::new(Resonator::new()),
};

/// The ratio of mode `k` (1 up) to the fundamental for `structure`.
fn partial(k: usize, structure: f32) -> f32 {
    let n = k as f32;
    let harmonic = n;
    let stiff = n * STIFFNESS.mul_add(n * n, 1.0).sqrt() / (1.0 + STIFFNESS).sqrt();
    // The free-free beam's roots; past the second, (k + 1/2) π is within a
    // few parts per million (3 ppm at the third, and closer after).
    let beta = match k {
        1 => 4.730_04,
        2 => 7.853_20,
        _ => (n + 0.5) * PI,
    };
    let bar = (beta / 4.730_04f32).powi(2);
    // Geometric blends, so the partials move evenly in pitch.
    if structure < 0.5 {
        let t = structure * 2.0;
        harmonic.powf(1.0 - t) * stiff.powf(t)
    } else {
        let t = (structure - 0.5) * 2.0;
        stiff.powf(1.0 - t) * bar.powf(t)
    }
}

/// The decay time the damping knob gives the fundamental: 10 s down to
/// 0.1 s.
fn decay_time(damping: f32) -> f32 {
    10.0 * 0.01f32.powf(damping)
}

/// A mode's input gain. The level a mode rings at is its input gain over
/// `1 - r`, and `1 - r` shrinks as the sample rate rises, so the gain is set
/// in proportion to `1 - r` with a factor worked out at 48 kHz: the same
/// input rings the mode just as loud at 44.1 kHz or 192 kHz.
fn mode_input(radius: f32, rt60: f32) -> f32 {
    let reference = (1.0 - decay_radius(rt60, 48_000.0)).max(1e-9);
    // At 48 kHz this is (1 - r²)^¼, which keeps a noise-driven mode's
    // energy close to its input's while letting a struck mode ring clearly.
    (1.0 - radius) * 0.25f32.exp2() * reference.powf(-0.75)
}

/// The most a string's loop may give back a period: just under one.
const LOOP_CEILING: f32 = 0.999_5;

/// The one-pole loop lowpass's gain `(1 - c) / |1 - c e^(-jw)|` at an angle
/// with sine `sin` and cosine `cos`.
fn lowpass_gain(c: f32, sin: f32, cos: f32) -> f32 {
    (1.0 - c) / c.mul_add(-cos, 1.0).hypot(c * sin)
}

/// The darkest loop lowpass coefficient, no darker than `wanted`, whose
/// gain at the fundamental is still at least `floor` (the gain falls as the
/// coefficient rises, so a bisection finds the edge).
fn darkest(wanted: f32, floor: f32, sin: f32, cos: f32) -> f32 {
    if lowpass_gain(wanted, sin, cos) >= floor {
        return wanted;
    }
    let (mut bright, mut dark) = (0.0f32, wanted);
    for _ in 0..20 {
        let middle = 0.5 * (bright + dark);
        if lowpass_gain(middle, sin, cos) >= floor {
            bright = middle;
        } else {
            dark = middle;
        }
    }
    bright
}

/// Pick position from the knob: never quite at the end, never past the
/// middle (the string is symmetric).
fn pick(position: f32) -> f32 {
    0.48f32.mul_add(position, 0.02)
}

/// One mode: a complex one-pole resonator.
#[derive(Debug, Clone, Copy, Default)]
struct Mode {
    re: f32,
    im: f32,
    pole_re: f32,
    pole_im: f32,
    input: f32,
    /// Its level in each side now, where it is heading, and the step a
    /// sample that gets it there by the next control period, so a moving
    /// knob never steps the level of a ringing mode.
    gain: [f32; 2],
    goal: [f32; 2],
    step: [f32; 2],
}

impl Mode {
    fn process(&mut self, x: f32) -> f32 {
        let re = self
            .pole_re
            .mul_add(self.re, (-self.pole_im).mul_add(self.im, self.input * x));
        let im = self.pole_re.mul_add(self.im, self.pole_im * self.re);
        self.re = re;
        self.im = im;
        flush(&mut self.re);
        flush(&mut self.im);
        self.im
    }
}

/// A Karplus-Strong string with position, brightness and stiffness.
#[derive(Debug, Clone, Default)]
struct Strand {
    line: DelayLine,
    exciter: DelayLine,
    delay: f32,
    delay_step: f32,
    pick: f32,
    lowpass: f32,
    smoothing: f32,
    stiffness: f32,
    allpass: [(f32, f32); 2],
    gain: f32,
    input: f32,
    last: f32,
}

impl Strand {
    fn clear(&mut self) {
        self.line.clear();
        self.exciter.clear();
        self.lowpass = 0.0;
        self.allpass = [(0.0, 0.0); 2];
        self.last = 0.0;
    }

    /// Retune for `hz` with decay `rt60`, brightness and stiffness, over
    /// the next control period.
    fn tune(
        &mut self,
        hz: f32,
        rt60: f32,
        brightness: f32,
        structure: f32,
        position: f32,
        rate: f32,
    ) {
        let w = TAU * hz / rate;
        // The loop filters are specified at 48 kHz and mapped to this rate
        // through their poles (a pole `p` at 48 kHz is `p^(48000 / fs)`), so
        // the string sounds the same at every rate.
        let per_48k = 48_000.0 / rate;
        let a = -(0.5 * structure).powf(per_48k);
        let (sin, cos) = w.sin_cos();
        // The loop lowpass may not take more at the fundamental than the
        // decay allows, or the string would die sooner than the knob says
        // (a high, dark string); past that it is opened up.
        let wanted = 0.6f32.mul_add(1.0 - brightness, 0.02).powf(per_48k);
        let floor = decay_gain(1.0 / hz, rt60) / LOOP_CEILING;
        let c = darkest(wanted, floor, sin, cos);
        // One-pole lowpass (1 - c) / (1 - c z⁻¹): phase delay and gain at w.
        let lp_delay = (c * sin).atan2(c.mul_add(-cos, 1.0)) / w;
        let lp_gain = lowpass_gain(c, sin, cos);
        // First-order allpass (a + z⁻¹) / (1 + a z⁻¹): phase delay at w.
        let phase = (-sin).atan2(a + cos) - (-a * sin).atan2(a.mul_add(cos, 1.0));
        let ap_delay = -phase / w;
        let period = rate / hz;
        let target = 2.0f32.mul_add(-ap_delay, period - lp_delay).max(MIN_READ);
        self.delay_step = (target - self.delay) / CONTROL as f32;
        if self.delay < 1.0 {
            self.delay = target;
            self.delay_step = 0.0;
        }
        self.smoothing = c;
        self.stiffness = a;
        self.gain = (decay_gain(1.0 / hz, rt60) / lp_gain.max(1e-3)).min(LOOP_CEILING);
        self.input = self.gain.mul_add(-self.gain, 1.0).max(0.0).powf(0.25);
        self.pick = (position * period).max(MIN_READ);
    }

    fn process(&mut self, excitation: f32) -> f32 {
        self.delay += self.delay_step;
        let plucked = excitation - self.exciter.read(self.pick);
        self.exciter.push(excitation);
        let back = self.line.read(self.delay);
        self.lowpass = self.smoothing.mul_add(self.lowpass - back, back);
        let mut x = self.lowpass;
        for (last_in, last_out) in &mut self.allpass {
            let y = self.stiffness.mul_add(x - *last_out, *last_in);
            *last_in = x;
            *last_out = y;
            flush(last_out);
            x = y;
        }
        let mut y = self.gain.mul_add(x, self.input * plucked);
        flush(&mut y);
        flush(&mut self.lowpass);
        self.line.push(y);
        self.last = y;
        y
    }
}

/// The resonator bank.
#[derive(Debug, Clone)]
pub struct Resonator {
    rate: f32,
    prepared: bool,
    knobs: Knobs<8>,
    voice_pitch: [Smoothed; VOICES],
    modes: [[Mode; MODES]; VOICES],
    strands: [Strand; VOICES],
    modal: Ramp,
    strings: Ramp,
    sympathy: Ramp,
    modal_clear: bool,
    strings_clear: bool,
    /// What the modes were last tuned for.
    modal_tuned: [f32; 8],
    limiter: Limiter,
    countdown: usize,
}

impl Default for Resonator {
    fn default() -> Self {
        Self::new()
    }
}

impl Resonator {
    /// A resonator at the default settings, unprepared.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rate: 0.0,
            prepared: false,
            knobs: Knobs::new(&PARAMS),
            voice_pitch: [Smoothed::new(0.0); VOICES],
            modes: [[Mode::default(); MODES]; VOICES],
            strands: Default::default(),
            modal: Ramp::new(1.0),
            strings: Ramp::new(0.0),
            sympathy: Ramp::new(0.0),
            modal_clear: false,
            strings_clear: true,
            modal_tuned: [f32::NAN; 8],
            limiter: Limiter::new(0.9, 48_000.0),
            countdown: 0,
        }
    }

    /// Where each voice's pitch should be, semitones from middle C.
    fn voice_targets(&self) -> [f32; VOICES] {
        let chord = CHORDS[(self.knobs.target(CHORD) as usize).min(CHORDS.len() - 1)];
        let pitch = self.knobs.get(PITCH);
        std::array::from_fn(|v| pitch + chord[v] + DETUNE[v])
    }

    /// The model switch's three fades.
    fn follow_model(&mut self) {
        let model = self.knobs.target(MODEL);
        let targets: [f32; 3] = if model < 0.5 {
            [1.0, 0.0, 0.0]
        } else if model < 1.5 {
            [0.0, 1.0, 0.0]
        } else {
            [0.0, 1.0, 1.0]
        };
        let fade = MODEL_FADE * self.rate;
        let ramps = [&mut self.modal, &mut self.strings, &mut self.sympathy];
        for (ramp, target) in ramps.into_iter().zip(targets) {
            if (ramp.target() - target).abs() > f32::EPSILON {
                ramp.set(target, fade);
            }
        }
    }

    fn update(&mut self) {
        self.follow_model();
        let targets = self.voice_targets();
        for (pitch, target) in self.voice_pitch.iter_mut().zip(targets) {
            pitch.set(target);
        }
        let structure = self.knobs.get(STRUCTURE);
        let brightness = self.knobs.get(BRIGHTNESS);
        let rt60 = decay_time(self.knobs.get(DAMPING));
        let position = pick(self.knobs.get(POSITION));
        let nyquist = 0.45 * self.rate;
        let voice_norm = 1.0 / VOICE_LEVEL.iter().map(|l| l * l).sum::<f32>().sqrt();
        let modal_wanted = [
            self.voice_pitch[0].value(),
            self.voice_pitch[1].value(),
            self.voice_pitch[2].value(),
            self.voice_pitch[3].value(),
            structure,
            brightness,
            rt60,
            position,
        ];
        // The modes cost the most to retune, so only when something moved.
        let modal_due = (self.modal.target() > 0.0 || self.modal.value() > 0.0)
            && !unchanged(&modal_wanted, &self.modal_tuned);
        if modal_due {
            self.modal_tuned = modal_wanted;
        }
        for voice in 0..VOICES {
            let hz = MIDDLE_C * (self.voice_pitch[voice].value() / 12.0).exp2();
            if modal_due {
                self.tune_modes(
                    voice,
                    hz,
                    [structure, brightness, rt60, position],
                    voice_norm,
                    nyquist,
                );
            }
            if self.strings.target() > 0.0 || self.strings.value() > 0.0 {
                // Sympathetic strings ring on longer than the played one.
                let sympathy = self.sympathy.value();
                let ring = if voice == 0 {
                    rt60
                } else {
                    rt60 * 0.5f32.mul_add(sympathy, 1.0)
                };
                self.strands[voice].tune(hz, ring, brightness, structure, position, self.rate);
            }
        }
        // Every mode's level heads for its goal over the next period.
        let period = CONTROL as f32;
        for mode in self.modes.iter_mut().flatten() {
            for side in 0..2 {
                mode.step[side] = (mode.goal[side] - mode.gain[side]) / period;
            }
        }
    }

    fn tune_modes(
        &mut self,
        voice: usize,
        hz: f32,
        [structure, brightness, rt60, position]: [f32; 4],
        voice_norm: f32,
        nyquist: f32,
    ) {
        let tilt = 0.9f32.mul_add(1.0 - brightness, 0.3);
        let slope = 1.0 - brightness;
        let mut weights = [0.0f32; MODES];
        let mut ratios = [0.0f32; MODES];
        // Modes fade out over the third of an octave below 20 kHz (or the
        // top of the band at a low rate), the same at every rate, and
        // smoothly, so a mode gliding across the edge does not click.
        let top = 20_000.0f32.min(nyquist);
        let bottom = top / 2f32.cbrt();
        for (k, (weight, ratio)) in weights.iter_mut().zip(&mut ratios).enumerate() {
            *ratio = partial(k + 1, structure);
            let hz = hz * *ratio;
            let fade = if hz <= bottom {
                1.0
            } else if hz >= top {
                0.0
            } else {
                let along = (hz / bottom).log(top / bottom);
                0.5f32.mul_add((PI * along).cos(), 0.5)
            };
            let struck = (PI * (k + 1) as f32 * position).sin().abs();
            *weight = fade * struck * ((k + 1) as f32).powf(-slope);
        }
        let total = weights.iter().map(|w| w * w).sum::<f32>().sqrt().max(1e-6);
        let level = VOICE_LEVEL[voice] * voice_norm / total;
        for (k, mode) in self.modes[voice].iter_mut().enumerate() {
            let weight = weights[k] * level;
            let ratio = ratios[k];
            let radius = decay_radius(rt60 * ratio.powf(-tilt), self.rate);
            let (sin, cos) = (TAU * hz * ratio / self.rate).sin_cos();
            mode.pole_re = radius * cos;
            mode.pole_im = radius * sin;
            mode.input = mode_input(radius, rt60 * ratio.powf(-tilt));
            // Odd modes lean left, even right.
            let lean = if k % 2 == 0 { -0.5 } else { 0.5 };
            mode.goal = [weight * (1.0 - lean), weight * (1.0 + lean)];
        }
    }

    fn frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.knobs.step();
        for pitch in &mut self.voice_pitch {
            pitch.step();
        }
        if self.countdown == 0 {
            self.update();
            self.countdown = CONTROL;
        }
        self.countdown -= 1;
        let (left, right) = (clean(left), clean(right));
        let x = 0.5 * (left + right);
        let modal = self.modal.next();
        let strings = self.strings.next();
        let sympathy = self.sympathy.next();
        let (mut wet_left, mut wet_right) = (0.0f32, 0.0f32);

        if modal > 0.0 {
            self.modal_clear = false;
            let level = modal * MODAL_LEVEL;
            for voice in &mut self.modes {
                for mode in voice.iter_mut() {
                    let y = mode.process(x);
                    mode.gain[0] += mode.step[0];
                    mode.gain[1] += mode.step[1];
                    wet_left = mode.gain[0].mul_add(y * level, wet_left);
                    wet_right = mode.gain[1].mul_add(y * level, wet_right);
                }
            }
        } else if !self.modal_clear {
            for voice in &mut self.modes {
                for mode in voice.iter_mut() {
                    mode.re = 0.0;
                    mode.im = 0.0;
                }
            }
            self.modal_clear = true;
        }

        if strings > 0.0 {
            self.strings_clear = false;
            let root = self.strands[0].last;
            let level = strings * sympathy.mul_add(SYMPATHETIC_LEVEL - COMB_LEVEL, COMB_LEVEL);
            for (voice, strand) in self.strands.iter_mut().enumerate() {
                let excitation = if voice == 0 {
                    x
                } else {
                    sympathy.mul_add(0.25f32.mul_add(root, -x), x)
                };
                let y = strand.process(excitation) * level * VOICE_LEVEL[voice];
                let pan = STRING_PAN[voice];
                wet_left = (1.0 - pan).mul_add(y, wet_left);
                wet_right = (1.0 + pan).mul_add(y, wet_right);
            }
        } else if !self.strings_clear {
            for strand in &mut self.strands {
                strand.clear();
            }
            self.strings_clear = true;
        }

        let (wet_left, wet_right) = self.limiter.process(wet_left, wet_right);
        let (dry, wet) = equal_power(self.knobs.get(MIX));
        (
            dry.mul_add(left, wet * wet_left),
            dry.mul_add(right, wet * wet_right),
        )
    }
}

impl Effect for Resonator {
    fn prepare(&mut self, sample_rate: f32) {
        let Some(rate) = usable_rate(sample_rate) else {
            self.prepared = false;
            return;
        };
        self.rate = rate;
        let longest = (rate / LOWEST_HZ) as usize + 8;
        for strand in &mut self.strands {
            strand.line = DelayLine::new(longest);
            strand.exciter = DelayLine::new(longest);
        }
        self.limiter = Limiter::new(0.9, rate);
        self.knobs.prepare(rate, &GLIDES);
        let targets = self.voice_targets();
        for (pitch, target) in self.voice_pitch.iter_mut().zip(targets) {
            pitch.set_time(CHORD_GLIDE, rate);
            pitch.snap(target);
        }
        self.follow_model();
        for ramp in [&mut self.modal, &mut self.strings, &mut self.sympathy] {
            ramp.snap(ramp.target());
        }
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for mode in self.modes.iter_mut().flatten() {
            mode.re = 0.0;
            mode.im = 0.0;
            mode.gain = [0.0; 2];
            mode.step = [0.0; 2];
        }
        for strand in &mut self.strands {
            strand.clear();
            strand.delay = 0.0;
        }
        self.limiter.reset();
        self.modal_tuned = [f32::NAN; 8];
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
    use super::super::testing::{self, RATE, build, impulse, peak, rms, run_mono, sine};

    /// The strongest frequency in a signal between `low` and `high` Hz, by
    /// a direct scan (tests only).
    fn loudest(signal: &[f32], low: f32, high: f32) -> f32 {
        let steps = (high / low).log(1.002f32) as i32;
        let mut best = (0.0f32, low);
        for step in 0..steps {
            let hz = low * 1.002f32.powi(step);
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (n, &x) in signal.iter().enumerate() {
                let angle = std::f32::consts::TAU * hz * n as f32 / RATE;
                re = x.mul_add(angle.cos(), re);
                im = x.mul_add(angle.sin(), im);
            }
            let size = re.hypot(im);
            if size > best.0 {
                best = (size, hz);
            }
        }
        best.1
    }

    #[test]
    fn the_resonator_keeps_the_contract() {
        testing::contract("resonator");
    }

    #[test]
    fn modes_follow_the_structure() {
        assert!((super::partial(3, 0.0) - 3.0).abs() < 1e-5);
        assert!(super::partial(8, 0.5) > 8.0);
        assert!((super::partial(2, 1.0) - 2.756).abs() < 0.01);
    }

    #[test]
    fn a_high_bar_is_the_same_at_every_rate() {
        testing::rates_agree_with("resonator", &[("pitch", 24.0), ("structure", 1.0)]);
    }

    #[test]
    fn every_model_rings_at_the_pitch_it_is_set_to() {
        for model in [0.0, 1.0, 2.0] {
            // A2, 110 Hz: 15 semitones below middle C.
            let mut resonator = build(
                "resonator",
                &[
                    ("model", model),
                    ("pitch", -15.0),
                    ("mix", 1.0),
                    ("damping", 0.2),
                    ("structure", 0.0),
                ],
            );
            let (left, right) = run_mono(resonator.as_mut(), &impulse(1.0));
            let mono: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l + r).collect();
            let tail = &mono[(0.2 * RATE) as usize..(0.7 * RATE) as usize];
            let found = loudest(tail, 90.0, 130.0);
            assert!(
                (found / 110.0 - 1.0).abs() < 0.01,
                "model {model}: rang at {found} Hz"
            );
        }
    }

    #[test]
    fn a_note_fed_at_its_own_pitch_is_held_by_the_limiter() {
        let mut resonator = build(
            "resonator",
            &[("pitch", -12.0), ("mix", 1.0), ("damping", 0.0)],
        );
        let (left, right) = run_mono(resonator.as_mut(), &sine(3.0, 130.81, 1.0));
        let settled = (0.5 * RATE) as usize;
        let top = peak(&left[settled..]).max(peak(&right[settled..]));
        assert!(top <= 1.0, "the limiter let {top} through");
        assert!(rms(&left) > 0.05);
    }

    #[test]
    fn switching_models_does_not_click() {
        let jumps = |switch: bool| {
            let mut resonator = build("resonator", &[("mix", 1.0), ("damping", 0.1)]);
            run_mono(resonator.as_mut(), &sine(0.5, 130.81, 0.3));
            let mut worst = 0.0f32;
            for model in [1.0, 2.0, 0.0] {
                if switch {
                    resonator.set_param(super::MODEL, model);
                }
                let (left, _) = run_mono(resonator.as_mut(), &sine(0.2, 130.81, 0.3));
                worst = left
                    .windows(2)
                    .fold(worst, |m, w| m.max((w[1] - w[0]).abs()));
            }
            worst
        };
        let (switched, still) = (jumps(true), jumps(false));
        assert!(
            switched <= 1.5f32.mul_add(still, 1e-3),
            "{switched} against {still}"
        );
    }

    #[test]
    fn a_brightness_sweep_does_not_zip() {
        // A ringing harmonic string whose partials all sit below 2.1 kHz,
        // swept from dark to bright, against the same string left at one
        // brightness: the sweep adds nothing above the partials.
        let above = |sweep: bool| {
            let mut resonator = build(
                "resonator",
                &[
                    ("mix", 1.0),
                    ("pitch", -12.0),
                    ("structure", 0.0),
                    ("damping", 0.2),
                    ("brightness", 0.5),
                ],
            );
            let mut click = impulse(1.0);
            click[0] = 0.5;
            let mut output = Vec::new();
            for (block, chunk) in click.chunks(32).enumerate() {
                if sweep {
                    resonator.set_param(super::BRIGHTNESS, block as f32 / 1_500.0);
                }
                let (left, _) = run_mono(resonator.as_mut(), chunk);
                output.extend(left);
            }
            // An eighth-order highpass at 3.5 kHz: 70 dB down at the top
            // partial.
            let mut sections = [0.509_795_6, 0.601_344_9, 0.899_976_3, 2.562_915_4].map(|q| {
                let mut section = super::super::matched::Biquad::default();
                section.set(super::super::matched::highpass(3_500.0, q, f64::from(RATE)));
                section
            });
            let high: Vec<f32> = output
                .iter()
                .map(|&x| {
                    sections
                        .iter_mut()
                        .fold(f64::from(x), |signal, section| section.process(signal))
                        as f32
                })
                .collect();
            20.0 * (rms(&high) / rms(&output)).log10()
        };
        let (swept, still) = (above(true), above(false));
        assert!(
            swept < still + 3.0,
            "zipper noise at {swept} dB against {still} dB"
        );
    }

    /// A string tuned directly, plucked, and its fundamental found to a
    /// tenth of a cent.
    fn strand_pitch(hz: f32, brightness: f32, structure: f32, rate: f32) -> f32 {
        let mut strand = super::Strand {
            line: crate::dsp::DelayLine::new((rate / super::LOWEST_HZ) as usize + 8),
            exciter: crate::dsp::DelayLine::new((rate / super::LOWEST_HZ) as usize + 8),
            ..super::Strand::default()
        };
        strand.tune(hz, 10.0, brightness, structure, 0.3, rate);
        let total = (2.0 * rate) as usize;
        let out: Vec<f32> = (0..total)
            .map(|n| strand.process(if n == 0 { 1.0 } else { 0.0 }))
            .collect();
        let window = &out[total / 4..];
        let strength = |f: f32| {
            let size = window.len() as f64;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (n, &x) in window.iter().enumerate() {
                let hann = 0.5f64.mul_add(-(std::f64::consts::TAU * n as f64 / size).cos(), 0.5);
                let angle = std::f64::consts::TAU * f64::from(f) * n as f64 / f64::from(rate);
                re = (f64::from(x) * hann).mul_add(angle.cos(), re);
                im = (f64::from(x) * hann).mul_add(angle.sin(), im);
            }
            re.hypot(im)
        };
        let mut best = (0.0f64, hz);
        for tenth in -300..=300 {
            let f = hz * (tenth as f32 / 12_000.0).exp2();
            let value = strength(f);
            if value > best.0 {
                best = (value, f);
            }
        }
        1_200.0 * (best.1 / hz).log2()
    }

    #[test]
    fn a_string_is_in_tune_to_three_cents_high_dark_and_stiff() {
        for rate in [44_100.0f32, 192_000.0] {
            for (hz, brightness, structure) in [
                (1_046.5f32, 0.0f32, 1.0f32),
                (130.81, 0.5, 0.5),
                (3_135.96, 0.0, 1.0),
            ] {
                let cents = strand_pitch(hz, brightness, structure, rate);
                assert!(cents.abs() < 3.0, "{hz} Hz at {rate}: {cents} cents out");
            }
        }
    }

    #[test]
    fn a_string_rings_for_the_decay_it_is_given() {
        for (hz, brightness) in [(130.81f32, 1.0f32), (1_046.5, 0.0), (3_135.96, 0.0)] {
            let mut strand = super::Strand {
                line: crate::dsp::DelayLine::new((RATE / super::LOWEST_HZ) as usize + 8),
                exciter: crate::dsp::DelayLine::new((RATE / super::LOWEST_HZ) as usize + 8),
                ..super::Strand::default()
            };
            strand.tune(hz, 1.0, brightness, 0.0, 0.3, RATE);
            let out: Vec<f32> = (0..(2.0 * RATE) as usize)
                .map(|n| strand.process(if n == 0 { 1.0 } else { 0.0 }))
                .collect();
            let edge = 1.12f32;
            let low = testing::lowpassed(&testing::lowpassed(&out, hz * edge), hz * edge);
            let fundamental = testing::highpassed(&testing::highpassed(&low, hz / edge), hz / edge);
            let measured = testing::rt60(&fundamental);
            assert!(
                (measured - 1.0).abs() < 0.2,
                "{hz} Hz, brightness {brightness}: {measured} s"
            );
        }
    }
}
