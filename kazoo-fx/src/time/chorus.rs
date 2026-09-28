//! Chorus, modelled on the Roland Juno-60's, with a richer ensemble mode.
//!
//! # The model
//!
//! The Juno-60 chorus is a pair of short bucket-brigade delays (MN3009s)
//! swept by one triangle LFO, the right delay swept upside down, each mixed
//! one to one with the dry signal. Its two buttons set the sweep:
//!
//! - **I**: 0.513 Hz, the delay swinging between 1.66 and 5.35 ms.
//! - **II**: 0.863 Hz over the same span, a deeper-sounding wobble.
//! - **I+II**: 9.75 Hz over a narrow 3.3 to 3.7 ms: a fast shimmer.
//!
//! These are the commonly cited measurements of the real unit. The wet side
//! goes through an 8 kHz reconstruction lowpass, as the bucket-brigade's
//! output stage does, and stays linear: the Juno's chorus is clean.
//!
//! - **Ensemble** is a richer string-machine chorus: three voices a side,
//!   spaced a third of a cycle apart, swept by a slow sine and a faster
//!   vibrato together, as in the Solina's and the Dimension's ensembles.
//!
//! `rate` and `depth` scale whichever mode is on (1 and full depth are the
//! real unit), `width` narrows the stereo image, and at a `mix` of a half
//! the dry and wet are each at unity, as on the Juno. Switching mode glides
//! every setting, so it never clicks.
//!
//! Sources: the Juno-60 service notes and published measurements of its
//! chorus LFO and delay range.

use super::parts::{
    Biquad, Sinc, accept, clean, defaults, frames, guard, silence_from, sine, triangle,
};
use crate::dsp::{DelayLine, Phasor, Smoothed, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const MODE: usize = 0;
const RATE: usize = 1;
const DEPTH: usize = 2;
const WIDTH: usize = 3;
const MIX: usize = 4;

/// One chorus setting: where the delay sits and how it sweeps.
#[derive(Debug, Clone, Copy)]
struct Setting {
    /// Centre delay, in seconds.
    centre: f32,
    /// Slow sweep either side of the centre, in seconds.
    swing: f32,
    /// Slow sweep rate, in hertz.
    hz: f32,
    /// 0 for a triangle sweep, 1 for a sine.
    round: f32,
    /// The level of the second and third voices.
    ensemble: f32,
    /// Fast vibrato either side, in seconds.
    vibrato: f32,
}

const SETTINGS: [Setting; 4] = [
    Setting {
        centre: 0.003_505,
        swing: 0.001_845,
        hz: 0.513,
        round: 0.0,
        ensemble: 0.0,
        vibrato: 0.0,
    },
    Setting {
        centre: 0.003_505,
        swing: 0.001_845,
        hz: 0.863,
        round: 0.0,
        ensemble: 0.0,
        vibrato: 0.0,
    },
    Setting {
        centre: 0.003_5,
        swing: 0.000_2,
        hz: 9.75,
        round: 0.0,
        ensemble: 0.0,
        vibrato: 0.0,
    },
    Setting {
        centre: 0.007,
        swing: 0.002_2,
        hz: 0.62,
        round: 1.0,
        ensemble: 1.0,
        vibrato: 0.000_3,
    },
];

const VOICES: usize = 3;
/// The ensemble's vibrato rate.
const VIBRATO_HZ: f32 = 6.1;
const MAX_DELAY: f32 = 0.02;
const TONE_HZ: f32 = 8_000.0;

const PARAMS: [ParamSpec; 5] = [
    ParamSpec {
        name: "mode",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["I", "II", "I+II", "ensemble"],
        },
    },
    ParamSpec {
        name: "rate",
        min: 0.25,
        max: 4.0,
        default: 1.0,
        unit: "",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "depth",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        unit: "",
        curve: Curve::Linear,
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
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The chorus.
pub static KIND: EffectKind = EffectKind {
    id: "chorus",
    name: "Chorus",
    description: "The Juno-60's bucket-brigade chorus, modes I, II and I+II, plus a lush \
                  three-voice ensemble.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Chorus::new())
}

/// The gliding form of a [`Setting`].
#[derive(Debug, Clone, Copy)]
struct Sweep {
    centre: Smoothed,
    swing: Smoothed,
    hz: Smoothed,
    round: Smoothed,
    ensemble: Smoothed,
    vibrato: Smoothed,
}

impl Sweep {
    const fn new(setting: Setting) -> Self {
        Self {
            centre: Smoothed::new(setting.centre),
            swing: Smoothed::new(setting.swing),
            hz: Smoothed::new(setting.hz),
            round: Smoothed::new(setting.round),
            ensemble: Smoothed::new(setting.ensemble),
            vibrato: Smoothed::new(setting.vibrato),
        }
    }

    const fn all(&mut self) -> [&mut Smoothed; 6] {
        [
            &mut self.centre,
            &mut self.swing,
            &mut self.hz,
            &mut self.round,
            &mut self.ensemble,
            &mut self.vibrato,
        ]
    }

    const fn aim(&mut self, setting: Setting) {
        self.centre.set(setting.centre);
        self.swing.set(setting.swing);
        self.hz.set(setting.hz);
        self.round.set(setting.round);
        self.ensemble.set(setting.ensemble);
        self.vibrato.set(setting.vibrato);
    }
}

/// The chorus.
#[derive(Debug)]
pub struct Chorus {
    rate: f32,
    prepared: bool,
    glide: Sweep,
    speed: Smoothed,
    depth: Smoothed,
    width: Smoothed,
    mix: Smoothed,
    slow: Phasor,
    fast: Phasor,
    lines: [DelayLine; 2],
    sinc: Sinc,
    tone: [Biquad; 2],
}

impl Default for Chorus {
    fn default() -> Self {
        Self::new()
    }
}

impl Chorus {
    /// A chorus at its defaults, unprepared.
    #[must_use]
    pub fn new() -> Self {
        let values = defaults(&PARAMS);
        Self {
            rate: 48_000.0,
            prepared: false,
            glide: Sweep::new(setting(values[MODE])),
            speed: Smoothed::new(values[RATE]),
            depth: Smoothed::new(values[DEPTH]),
            width: Smoothed::new(values[WIDTH]),
            mix: Smoothed::new(values[MIX]),
            slow: Phasor::default(),
            fast: Phasor::default(),
            lines: [DelayLine::default(), DelayLine::default()],
            sinc: Sinc::default(),
            tone: [Biquad::new(); 2],
        }
    }

    fn smoothers(&mut self) -> impl Iterator<Item = &mut Smoothed> {
        let knobs = [
            &mut self.speed,
            &mut self.depth,
            &mut self.width,
            &mut self.mix,
        ];
        self.glide.all().into_iter().chain(knobs)
    }

    fn render(&mut self, input: [&[f32]; 2], output: &mut [&mut [f32]; 2], n: usize) {
        let rate = self.rate;
        for i in 0..n {
            let centre = self.glide.centre.step();
            let depth = self.depth.step();
            let swing = self.glide.swing.step() * depth;
            let speed = self.speed.step();
            let round = self.glide.round.step();
            let extra = self.glide.ensemble.step();
            let vibrato = self.glide.vibrato.step() * depth;
            let width = self.width.step();
            let mix = self.mix.step();
            let slow = self.slow.next(self.glide.hz.step() * speed, rate);
            let fast = self.fast.next(VIBRATO_HZ * speed, rate);
            let gains = [1.0, extra, extra];
            let norm = 1.0 / 2.0f32.mul_add(extra * extra, 1.0).sqrt();
            let x = [clean(input[0][i]), clean(input[1][i])];
            let mut wet = [0.0; 2];
            for (c, (line, tone)) in self.lines.iter_mut().zip(&mut self.tone).enumerate() {
                line.push(x[c]);
                let mut sum = 0.0;
                for (k, gain) in gains.iter().enumerate() {
                    let offset = 0.5f32.mul_add(c as f32, k as f32 / VOICES as f32);
                    let place = (slow + offset).fract();
                    let shape = (sine(place) - triangle(place)).mul_add(round, triangle(place));
                    let wobble = sine(offset.mul_add(0.75, fast).fract());
                    let seconds = vibrato.mul_add(wobble, swing.mul_add(shape, centre));
                    let at = f64::from(seconds).mul_add(f64::from(rate), 1.0);
                    sum = gain.mul_add(self.sinc.read(line, at), sum);
                }
                wet[c] = tone.process(sum * norm);
            }
            let mid = 0.5 * (wet[0] + wet[1]);
            let side = 0.5 * (wet[0] - wet[1]) * width;
            let dry_gain = (2.0 * (1.0 - mix)).min(1.0);
            let wet_gain = (2.0 * mix).min(1.0);
            output[0][i] = guard(dry_gain.mul_add(x[0], wet_gain * (mid + side)));
            output[1][i] = guard(dry_gain.mul_add(x[1], wet_gain * (mid - side)));
        }
    }
}

/// The setting of mode step `step`.
fn setting(step: f32) -> Setting {
    SETTINGS[(step.round().max(0.0) as usize).min(SETTINGS.len() - 1)]
}

impl Effect for Chorus {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        self.rate = rate;
        self.sinc = Sinc::new();
        for (line, tone) in self.lines.iter_mut().zip(&mut self.tone) {
            line.resize((MAX_DELAY * rate) as usize);
            tone.set_lowpass(TONE_HZ, 0.707, rate);
        }
        for smoother in self.smoothers() {
            smoother.set_time(0.02, rate);
        }
        for smoother in self.glide.all() {
            smoother.set_time(0.05, rate);
        }
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for (line, tone) in self.lines.iter_mut().zip(&mut self.tone) {
            line.clear();
            tone.reset();
        }
        for smoother in self.smoothers() {
            smoother.snap(smoother.target());
        }
        self.slow.set(0.0);
        self.fast.set(0.0);
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(value) = accept(&PARAMS, index, value) else {
            return;
        };
        match index {
            MODE => self.glide.aim(setting(value)),
            RATE => self.speed.set(value),
            DEPTH => self.depth.set(value),
            WIDTH => self.width.set(value),
            MIX => self.mix.set(value),
            _ => {}
        }
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        let mut output = output;
        let n = if self.prepared {
            frames(&input, &output)
        } else {
            0
        };
        self.render(input, &mut output, n);
        silence_from(&mut output, n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::testkit::{RATE as HOST, context, render, rms, sine as tone};

    fn spread(mode: f32) -> f32 {
        let mut chorus = crate::time::testkit::prepared(&KIND);
        chorus.set_param(MODE, mode);
        chorus.reset();
        let input = tone(HOST as usize * 2, 440.0, 0.5);
        let (left, right) = render(chorus.as_mut(), context(120.0), &input, &input, 256);
        let difference: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
        rms(&difference[HOST as usize..])
    }

    #[test]
    fn every_mode_spreads_a_mono_source() {
        for mode in 0..4 {
            assert!(spread(mode as f32) > 0.01, "mode {mode}");
        }
    }

    #[test]
    fn narrow_width_is_mono() {
        let mut chorus = crate::time::testkit::prepared(&KIND);
        chorus.set_param(WIDTH, 0.0);
        chorus.reset();
        let input = tone(HOST as usize, 440.0, 0.5);
        let (left, right) = render(chorus.as_mut(), context(120.0), &input, &input, 256);
        let difference: Vec<f32> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
        assert!(rms(&difference) < 1e-4);
    }
}
