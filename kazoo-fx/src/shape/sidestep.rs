//! Input-clocked synthesis: no note names, quantisation or pitch detector.
//!
//! The four bilinear poles, two allpasses, adaptive comparator and dividers
//! follow Esqueda, Lähdeoja and Välimäki, "Algorithms for guitar-driven
//! synthesis: application to an augmented guitar", SMC 2018, pp. 444–451:
//! <https://doi.org/10.5281/zenodo.1422641> (equations 3–7).
//! Period-driven filter tracking is inspired by the GR-300 description:
//! <https://gr300.com/GR-300.htm>. Divider context: the Boss OC-2 at
//! <https://en.wikipedia.org/wiki/Octave_effect>, and The Valve Wizard's
//! own U-Boat design, which compares itself with the OC-2,
//! <https://www.valvewizard.co.uk/uboat.html>. Boss's SY-1 and SY-300
//! method is not public; this effect makes no claim to reproduce it.

use std::f64::consts::FRAC_PI_2;

use super::{blep::EdgeWave, svf::Svf};
use crate::drive::kit::{DEFAULT_RATE, Knobs, flush64, one_pole};
use crate::dsp::{OnePole, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const N: usize = 21;

/// The longest gap between rising edges, in seconds, that is taken as a
/// period: 10 Hz, half the lowest `low` corner, so no input the high-pass
/// passes is longer.
const LONGEST_PERIOD: f64 = 0.1;

const fn knob(
    name: &'static str,
    min: f32,
    max: f32,
    default: f32,
    unit: &'static str,
    curve: Curve,
) -> ParamSpec {
    ParamSpec {
        name,
        min,
        max,
        default,
        unit,
        curve,
    }
}

static PARAMS: [ParamSpec; N] = [
    knob("low", 20.0, 200.0, 30.0, "Hz", Curve::Linear),
    knob("high", 200.0, 4000.0, 1000.0, "Hz", Curve::Log),
    knob("square", 0.0, 100.0, 0.0, "%", Curve::Linear),
    knob("sub1", 0.0, 100.0, 100.0, "%", Curve::Linear),
    knob("sub2", 0.0, 100.0, 0.0, "%", Curve::Linear),
    knob("ring", 0.0, 100.0, 0.0, "%", Curve::Linear),
    knob("half", 0.0, 100.0, 0.0, "%", Curve::Linear),
    knob("fold", 0.0, 100.0, 0.0, "%", Curve::Linear),
    knob("drive", 1.0, 10.0, 3.0, "", Curve::Linear),
    knob("cutoff", 40.0, 12_000.0, 1200.0, "Hz", Curve::Log),
    knob("reso", 0.0, 100.0, 30.0, "%", Curve::Linear),
    knob("track", 0.0, 100.0, 50.0, "%", Curve::Linear),
    knob("fenv", -100.0, 100.0, 50.0, "%", Curve::Linear),
    knob("attack", 0.5, 2000.0, 2.0, "ms", Curve::Log),
    knob("decay", 5.0, 5000.0, 300.0, "ms", Curve::Log),
    knob("sustain", 0.0, 100.0, 60.0, "%", Curve::Linear),
    knob("release", 5.0, 5000.0, 200.0, "ms", Curve::Log),
    knob(
        "env",
        0.0,
        1.0,
        1.0,
        "",
        Curve::Stepped {
            labels: &["follow", "adsr"],
        },
    ),
    knob("sense", -70.0, -20.0, -45.0, "dB", Curve::Linear),
    knob("dry", 0.0, 100.0, 0.0, "%", Curve::Linear),
    knob("level", -24.0, 12.0, 0.0, "dB", Curve::Linear),
];

pub(super) const KIND: EffectKind = EffectKind {
    id: "sidestep",
    name: "Sidestep",
    description: "Turn the input's own cycles into a synth: squares, subs, ring and fold.",
    params: &PARAMS,
    build: || Box::new(Sidestep::new()),
};

/// The bilinear one-pole low-pass at `cutoff`: `(b, a)` for
/// `y = b (x + x₋₁) − a y₋₁`, prewarped so it is 3 dB down at `cutoff`
/// itself, and held under 0.45 × `rate` so the warp stays finite. The same
/// `a` is the allpass coefficient (Esqueda et al., eqs. 3 to 5): a
/// first-order allpass with it has twice this low-pass's phase lag at every
/// frequency, so two allpasses have the phase of four low-passes.
pub(super) fn bilinear_lowpass(cutoff: f32, rate: f64) -> (f64, f64) {
    let corner = f64::from(cutoff).clamp(1.0, 0.45 * rate);
    let w = 2.0 * (std::f64::consts::PI * corner / rate).tan();
    (w / (2.0 + w), (w - 2.0) / (w + 2.0))
}

#[derive(Debug, Clone, Copy, Default)]
struct Pole {
    input: f64,
    output: f64,
}

impl Pole {
    fn lowpass(&mut self, input: f64, b: f64, a: f64) -> f64 {
        self.output = a.mul_add(-self.output, b * (input + self.input));
        self.input = input;
        flush64(&mut self.input);
        flush64(&mut self.output);
        self.output
    }

    fn allpass(&mut self, input: f64, a: f64) -> f64 {
        self.output = a.mul_add(input - self.output, self.input);
        self.input = input;
        flush64(&mut self.input);
        flush64(&mut self.output);
        self.output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug, Default)]
struct Comparators {
    positive: bool,
    negative: bool,
}

#[derive(Debug, Default)]
struct Dividers {
    sub1: bool,
    sub2: bool,
}

#[derive(Debug)]
pub(super) struct Sidestep {
    knobs: Knobs<N>,
    rate: f64,
    highpass: OnePole,
    poles: [Pole; 4],
    allpasses: [Pole; 2],
    front: [f32; 2],
    a: f64,
    b: f64,
    peak: f64,
    peak_input: f64,
    charge: f64,
    discharge: f64,
    previous_s: f64,
    previous_negative: f64,
    comparators: Comparators,
    latch: bool,
    dividers: Dividers,
    edges: [EdgeWave; 3],
    edge_age: f64,
    have_edge: bool,
    gate: bool,
    period: f64,
    clock: f64,
    clock_coeff: f64,
    envelope: f64,
    attack_coeff: f64,
    release_coeff: f64,
    valley: f64,
    valley_coeff: f64,
    onset_age: f64,
    adsr: f64,
    stage: Stage,
    envelope_params: [f32; 3],
    attack_step: f64,
    decay_factor: f64,
    release_factor: f64,
    previous_x: f64,
    previous_d: f64,
    previous_env: f64,
    filter: Svf,
    level_db: f32,
    level: f64,
    band_limited: bool,
}

impl Sidestep {
    pub(super) fn new() -> Self {
        let mut this = Self {
            knobs: Knobs::new(&PARAMS),
            rate: f64::from(DEFAULT_RATE),
            highpass: OnePole::default(),
            poles: [Pole::default(); 4],
            allpasses: [Pole::default(); 2],
            front: [0.0; 2],
            a: 0.0,
            b: 0.0,
            peak: 0.0,
            peak_input: 0.0,
            charge: 0.0,
            discharge: 0.0,
            previous_s: 0.0,
            previous_negative: 0.0,
            comparators: Comparators::default(),
            latch: false,
            dividers: Dividers::default(),
            edges: [EdgeWave::new(); 3],
            edge_age: 0.0,
            have_edge: false,
            gate: false,
            period: 48_000.0 / 220.0,
            clock: 220.0,
            clock_coeff: 0.0,
            envelope: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            valley: -160.0,
            valley_coeff: 0.0,
            onset_age: 48_000.0,
            adsr: 0.0,
            stage: Stage::Idle,
            envelope_params: [0.0; 3],
            attack_step: 0.0,
            decay_factor: 0.0,
            release_factor: 0.0,
            previous_x: 0.0,
            previous_d: 0.0,
            previous_env: 0.0,
            filter: Svf::new(),
            level_db: f32::NAN,
            level: 1.0,
            band_limited: true,
        };
        this.prepare(DEFAULT_RATE);
        this
    }

    fn tune(&mut self, p: &[f32; N]) {
        if p[..2] != self.front {
            self.front = [p[0], p[1]];
            self.highpass.set_cutoff(p[0], self.rate as f32);
            (self.b, self.a) = bilinear_lowpass(p[1], self.rate);
        }
        let times = [p[13], p[14], p[16]];
        if times.map(f32::to_bits) != self.envelope_params.map(f32::to_bits) {
            self.envelope_params = times;
            self.attack_step = 1000.0 / (self.rate * f64::from(p[13]));
            self.decay_factor =
                (-9.210_340_371_976_184 * 1000.0 / (self.rate * f64::from(p[14]))).exp();
            self.release_factor =
                (-9.210_340_371_976_184 * 1000.0 / (self.rate * f64::from(p[16]))).exp();
        }
        if p[20].to_bits() != self.level_db.to_bits() {
            self.level_db = p[20];
            self.level = 10.0_f64.powf(f64::from(p[20]) / 20.0);
        }
    }

    fn clock_waves(&mut self, s: f64) -> [f64; 3] {
        let alpha = if s <= self.peak {
            self.charge
        } else {
            self.discharge
        };
        let drive = if s <= self.peak { s } else { 0.0 };
        self.peak =
            (1.0 - alpha).mul_add(self.peak, alpha * (drive + self.peak_input)) / (1.0 + alpha);
        self.peak_input = drive;
        flush64(&mut self.peak);
        let positive = s >= 0.0;
        let difference = s - self.peak;
        let negative = difference < 0.0;
        let was_high = self.latch;
        let mut fraction = 1.0;
        if positive && !self.comparators.positive {
            self.latch = true;
            fraction = if s > self.previous_s {
                (-self.previous_s / (s - self.previous_s)).clamp(0.0, 1.0)
            } else {
                1.0
            };
        } else if negative && !self.comparators.negative {
            self.latch = false;
            fraction =
                (self.previous_negative / (self.previous_negative - difference)).clamp(0.0, 1.0);
        }
        self.edge_age += 1.0;
        if !self.gate || self.edge_age > self.rate * LONGEST_PERIOD {
            // A rest, or a gap no input below the lowest `low` corner can
            // make, is not a cycle: the next edge starts a new count.
            self.have_edge = false;
        }
        if self.latch && !was_high {
            if self.have_edge {
                self.period = (self.edge_age + fraction - 1.0).max(1.0);
            }
            self.have_edge = true;
            self.edge_age = 1.0 - fraction;
            self.dividers.sub1 = !self.dividers.sub1;
            if self.dividers.sub1 {
                self.dividers.sub2 = !self.dividers.sub2;
            }
        }
        self.comparators.positive = positive;
        self.comparators.negative = negative;
        self.previous_s = s;
        self.previous_negative = difference;
        [
            self.edges[0].step(self.latch, fraction, self.band_limited),
            self.edges[1].step(self.dividers.sub1, fraction, self.band_limited),
            self.edges[2].step(self.dividers.sub2, fraction, self.band_limited),
        ]
    }

    fn envelope(&mut self, x: f64, p: &[f32; N]) {
        let magnitude = x.abs();
        let coefficient = if magnitude > self.envelope {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.envelope = coefficient.mul_add(magnitude - self.envelope, self.envelope);
        flush64(&mut self.envelope);
        let db = 20.0 * self.envelope.max(1e-8).log10();
        let gate = db >= f64::from(p[18]);
        self.gate = gate;
        if db < self.valley || self.onset_age < self.rate * 0.04 {
            // The same attack can keep rising during the refractory period.
            self.valley = db;
        } else {
            self.valley = self.valley_coeff.mul_add(db - self.valley, self.valley);
        }
        self.onset_age += 1.0;
        if gate && db - self.valley >= 6.0 && self.onset_age >= self.rate * 0.04 {
            self.stage = Stage::Attack;
            self.adsr = 0.0;
            self.valley = db;
            self.onset_age = 0.0;
        }
        if !gate && self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
        let sustain = f64::from(p[15]) * 0.01;
        match self.stage {
            Stage::Attack => {
                self.adsr = (self.adsr + self.attack_step).min(1.0);
                if self.adsr >= 1.0 {
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.adsr = (self.adsr - sustain).mul_add(self.decay_factor, sustain);
                if (self.adsr - sustain).abs() < 1e-4 {
                    self.adsr = sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => self.adsr = sustain,
            Stage::Release => {
                self.adsr *= self.release_factor;
                if self.adsr < 1e-4 {
                    self.adsr = 0.0;
                    self.stage = Stage::Idle;
                }
            }
            Stage::Idle => self.adsr = 0.0,
        }
    }

    fn sample(&mut self, left: f32, right: f32) -> f32 {
        let params = self.knobs.step();
        self.tune(&params);
        if !left.is_finite() || !right.is_finite() {
            self.clear_audio();
            return 0.0;
        }
        let mono = (left.clamp(-16.0, 16.0) + right.clamp(-16.0, 16.0)) * 0.5;
        let x = f64::from(self.highpass.highpass(mono));
        let mut s = x;
        for pole in &mut self.poles {
            s = pole.lowpass(s, self.b, self.a);
        }
        let mut d = x;
        for allpass in &mut self.allpasses {
            d = allpass.allpass(d, self.a);
        }
        let waves = self.clock_waves(s);
        self.envelope(x, &params);
        let dry = self.previous_x;
        let aligned = self.previous_d;
        self.previous_x = x;
        self.previous_d = d;
        let folded = if params[7] > 0.0 {
            (FRAC_PI_2
                * (f64::from(params[8]) * aligned / self.previous_env.max(1e-4)).clamp(-8.0, 8.0))
            .sin()
        } else {
            0.0
        };
        self.previous_env = self.envelope;
        let divided = f64::from(params[4]).mul_add(
            waves[2],
            f64::from(params[3]).mul_add(waves[1], f64::from(params[2]) * waves[0]),
        );
        let processors = f64::from(params[7]).mul_add(
            folded,
            (f64::from(params[6]) * aligned * (waves[1] + 1.0))
                .mul_add(0.5, f64::from(params[5]) * aligned * waves[1]),
        );
        let w = 0.01_f64.mul_add(processors, 0.005 * divided);
        let clock_target = (self.rate / self.period).clamp(1.0, self.rate * 0.45);
        self.clock = self
            .clock_coeff
            .mul_add(clock_target - self.clock, self.clock);
        let mode = f64::from(params[17]);
        let amplitude = (self.adsr - self.envelope).mul_add(mode, self.envelope);
        let cutoff = f64::from(params[9])
            * (f64::from(params[12]) * 0.06 * self.adsr * mode).exp2()
            * (self.clock / 220.0).powf(f64::from(params[11]) * 0.01);
        let filtered = self
            .filter
            .lowpass(w, cutoff, f64::from(params[10]) * 0.01, self.rate);
        let output = self.level * (f64::from(params[19]) * 0.01).mul_add(dry, filtered * amplitude);
        if output.is_finite() {
            if output.abs() < 1e-30 {
                0.0
            } else {
                output.clamp(-4.0, 4.0) as f32
            }
        } else {
            self.clear_audio();
            0.0
        }
    }

    #[cfg(test)]
    pub(super) const fn test_band_limit(&mut self, enabled: bool) {
        self.band_limited = enabled;
    }

    #[cfg(test)]
    pub(super) fn test_sample(&mut self, input: f32) -> (f32, [f64; 4]) {
        let output = self.sample(input, input);
        (
            output,
            [self.adsr, self.envelope, self.period, self.onset_age],
        )
    }

    fn clear_audio(&mut self) {
        self.highpass.reset();
        self.poles = [Pole::default(); 4];
        self.allpasses = [Pole::default(); 2];
        self.peak = 0.0;
        self.peak_input = 0.0;
        self.previous_s = 0.0;
        self.previous_negative = 0.0;
        self.comparators = Comparators::default();
        self.latch = false;
        self.dividers = Dividers::default();
        self.edges = [EdgeWave::new(); 3];
        self.edge_age = 0.0;
        self.have_edge = false;
        self.gate = false;
        self.period = self.rate / 220.0;
        self.clock = 220.0;
        self.envelope = 0.0;
        self.valley = -160.0;
        self.onset_age = self.rate;
        self.adsr = 0.0;
        self.stage = Stage::Idle;
        self.previous_x = 0.0;
        self.previous_d = 0.0;
        self.previous_env = 0.0;
        self.filter = Svf::new();
    }
}

impl Effect for Sidestep {
    fn prepare(&mut self, sample_rate: f32) {
        self.rate = f64::from(sane_rate(sample_rate));
        self.knobs.prepare(self.rate as f32);
        self.charge = 1.0 / (2.0 * 0.00103 * self.rate);
        self.discharge = 1.0 / (2.0 * 0.47 * self.rate);
        self.clock_coeff = one_pole(0.005, self.rate);
        self.attack_coeff = one_pole(0.001, self.rate);
        self.release_coeff = one_pole(0.03, self.rate);
        self.valley_coeff = one_pole(0.08, self.rate);
        self.front = [0.0; 2];
        self.envelope_params = [0.0; 3];
        self.level_db = f32::NAN;
        self.reset();
    }

    fn reset(&mut self) {
        self.knobs.settle();
        self.clear_audio();
        self.tune(&self.knobs.values());
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if value.is_finite() {
            self.knobs.set(index, value);
        }
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        let [left, right] = output;
        let length = input[0]
            .len()
            .min(input[1].len())
            .min(left.len())
            .min(right.len());
        for ((input_left, input_right), (output_left, output_right)) in input[0]
            .iter()
            .zip(input[1])
            .zip(left.iter_mut().zip(right.iter_mut()))
            .take(length)
        {
            let sample = self.sample(*input_left, *input_right);
            *output_left = sample;
            *output_right = sample;
        }
        left[length..].fill(0.0);
        right[length..].fill(0.0);
    }

    fn latency(&self) -> usize {
        1
    }
}
