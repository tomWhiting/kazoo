//! A studio channel vocoder in the manner of the EMS, Roland and Sennheiser
//! classics.
//!
//! The modulator (a voice, or a rendered phrase) runs through an analysis
//! bank of 8 to 40 fourth-order bandpasses laid out on the Bark scale. A
//! follower on each band measures its loudness. The carrier (a synth, a
//! chord, anything rich) runs through a matching synthesis bank, and each
//! carrier band is played at the loudness of its modulator band. The sum is
//! the carrier speaking with the modulator's mouth.
//!
//! Around that core sit the things that make a vocoder intelligible and
//! playable:
//!
//! - **Formant shift** slides the carrier bank up or down against the
//!   analysis bank, so the voice sounds larger or smaller without changing
//!   the carrier's pitch.
//! - **Unvoiced detection** listens for consonants (hiss and sibilance: much
//!   more energy high than low, and many zero crossings) and blends shaped
//!   noise into the carrier of the high bands while they last, so "s", "t"
//!   and "f" come through even from a carrier with no top end.
//! - **Emphasis** tilts the bank's output towards the highs.
//! - **Hold** freezes every band's loudness, so the carrier keeps singing
//!   the last vowel.
//! - **Dry** blends in the plain modulator, **mix** blends wet against the
//!   plain carrier, and **level** sets the output.
//!
//! # Real-time contract
//!
//! [`Vocoder::new`] and [`Vocoder::prepare`] allocate and belong off the
//! audio thread. They design the analysis bank for every band count up
//! front, so a band-count change on the audio thread is a switch to a design
//! that already exists: a short fade out, the switch, a fade back in.
//! Re-tuning the carrier bank for the formant shift is arithmetic on
//! pre-allocated state, done at most once every 32 samples.
//!
//! [`Vocoder::process`], [`Vocoder::set_param`] and [`Vocoder::reset`] never
//! allocate, lock, do I/O or panic. A NaN or infinite input is heard as
//! silence, a NaN parameter is ignored, and the output never leaves ±8.

use std::f32::consts::SQRT_2;

use kazoo_fx::dsp::{DcBlocker, Noise, OnePole, Smoothed, db_to_gain};
use kazoo_fx::{Curve, ParamSpec};

use crate::bank::{Band4, Coeffs, Layout, MAX_BANDS, MIN_BANDS};

/// Parameter indices for [`Vocoder::set_param`], in the order of [`PARAMS`].
pub mod param {
    /// Band count, 8 to 40.
    pub const BANDS: usize = 0;
    /// Band follower attack, ms.
    pub const ATTACK: usize = 1;
    /// Band follower release, ms.
    pub const RELEASE: usize = 2;
    /// Formant shift of the carrier bank, semitones.
    pub const SHIFT: usize = 3;
    /// How much noise replaces the high carrier bands on consonants.
    pub const UNVOICED: usize = 4;
    /// How readily a sound counts as a consonant.
    pub const SENSE: usize = 5;
    /// High-frequency tilt of the bank's output, dB at the top band.
    pub const EMPHASIS: usize = 6;
    /// Freeze the band loudnesses.
    pub const HOLD: usize = 7;
    /// Plain modulator blended into the output.
    pub const DRY: usize = 8;
    /// Vocoded against plain carrier.
    pub const MIX: usize = 9;
    /// Output level, dB.
    pub const LEVEL: usize = 10;
    /// How many parameters there are.
    pub const COUNT: usize = 11;
}

static BAND_LABELS: [&str; MAX_BANDS - MIN_BANDS + 1] = [
    "8", "9", "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23",
    "24", "25", "26", "27", "28", "29", "30", "31", "32", "33", "34", "35", "36", "37", "38", "39",
    "40",
];

static OFF_ON: [&str; 2] = ["off", "on"];

/// Every vocoder parameter, numbered as [`param`] numbers them.
pub static PARAMS: [ParamSpec; param::COUNT] = [
    ParamSpec {
        name: "bands",
        min: MIN_BANDS as f32,
        max: MAX_BANDS as f32,
        default: 20.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &BAND_LABELS,
        },
    },
    ParamSpec {
        name: "attack",
        min: 0.5,
        max: 200.0,
        default: 4.0,
        unit: "ms",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "release",
        min: 5.0,
        max: 2_000.0,
        default: 60.0,
        unit: "ms",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "shift",
        min: -12.0,
        max: 12.0,
        default: 0.0,
        unit: "st",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "unvoiced",
        min: 0.0,
        max: 1.0,
        default: 0.7,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "sense",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "emphasis",
        min: -12.0,
        max: 24.0,
        default: 6.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "hold",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped { labels: &OFF_ON },
    },
    ParamSpec {
        name: "dry",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 1.0,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "level",
        min: -36.0,
        max: 12.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
];

/// Samples between control-rate updates (detector decisions, carrier
/// re-tuning, emphasis, denormal flushing).
const CONTROL_BLOCK: usize = 32;

/// Length of each half of the fade around a band-count switch.
const DUCK_SECONDS: f32 = 0.003;

/// Peak level of the noise blended into the high bands on consonants.
const NOISE_LEVEL: f32 = 0.5;

/// Gain applied to the summed bands so a typical voice on a typical carrier
/// comes out near the carrier's own level at 0 dB.
const MAKEUP: f32 = 4.0;

/// The output is held within this, well above the nominal ±1: a guard
/// against extreme settings, not a limiter.
const SAFETY_CEILING: f32 = 8.0;

/// Time constant for the knobs smoothed per sample.
const SMOOTH_SECONDS: f32 = 0.02;

/// Time constant for the formant shift and emphasis glides.
const GLIDE_SECONDS: f32 = 0.03;

/// Band centres between these carry consonant noise, fading in across the
/// span.
const NOISE_SPAN_HZ: (f32, f32) = (1_800.0, 5_000.0);

/// A carrier band shifted below this, or above 45% of the sample rate, is
/// silent.
const LOWEST_CARRIER_HZ: f32 = 30.0;

/// One analysis design: where the bands sit and their coefficients.
#[derive(Debug, Clone, Copy)]
struct Design {
    layout: Layout,
    analysis: [Coeffs; MAX_BANDS],
}

impl Design {
    fn new(count: usize, sample_rate: f32) -> Self {
        let layout = Layout::new(count, sample_rate);
        let mut analysis = [Coeffs::default(); MAX_BANDS];
        for ((coeffs, &centre), &q) in analysis.iter_mut().zip(layout.centres()).zip(layout.qs()) {
            *coeffs = Coeffs::band_section(centre, q, sample_rate);
        }
        Self { layout, analysis }
    }
}

/// Everything one band carries from sample to sample.
#[derive(Debug, Clone, Copy, Default)]
struct Band {
    analysis: Band4,
    synthesis: [Band4; 2],
    carrier: Coeffs,
    carrier_live: bool,
    power: f32,
    attack: f32,
    release: f32,
    noise_weight: f32,
    tilt: f32,
}

/// `x` from 0 to 1 eased at both ends; held outside that range.
fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * 2.0f32.mul_add(-x, 3.0)
}

/// One-pole coefficient for a time constant of `seconds`.
fn pole(seconds: f32, sample_rate: f32) -> f32 {
    let samples = seconds * sample_rate;
    if samples.is_finite() && samples > 1.0 {
        1.0 - (-1.0 / samples).exp()
    } else {
        1.0
    }
}

/// Silence for anything non-finite.
const fn finite(sample: f32) -> f32 {
    if sample.is_finite() { sample } else { 0.0 }
}

/// Listens to the modulator for consonants: energy above 3 kHz against
/// energy below 1 kHz, the zero-crossing rate, and enough level to matter.
#[derive(Debug, Clone)]
struct Detector {
    sample_rate: f32,
    dc: DcBlocker,
    high: [OnePole; 2],
    low: [OnePole; 2],
    high_power: f32,
    low_power: f32,
    total_power: f32,
    follow: f32,
    crossings: f32,
    crossing_pole: f32,
    positive: bool,
    score: f32,
    target: f32,
    rise: f32,
    fall: f32,
    ratio_threshold_db: f32,
    crossing_threshold: f32,
}

impl Detector {
    fn new(sample_rate: f32) -> Self {
        let mut high = [OnePole::default(); 2];
        let mut low = [OnePole::default(); 2];
        for filter in &mut high {
            filter.set_cutoff(3_000.0, sample_rate);
        }
        for filter in &mut low {
            filter.set_cutoff(1_000.0, sample_rate);
        }
        let mut detector = Self {
            sample_rate,
            dc: DcBlocker::new(sample_rate),
            high,
            low,
            high_power: 0.0,
            low_power: 0.0,
            total_power: 0.0,
            follow: pole(0.008, sample_rate),
            crossings: 0.0,
            crossing_pole: pole(0.015, sample_rate),
            positive: false,
            score: 0.0,
            target: 0.0,
            rise: pole(0.003, sample_rate),
            fall: pole(0.03, sample_rate),
            ratio_threshold_db: 0.0,
            crossing_threshold: 0.0,
        };
        detector.set_sense(PARAMS[param::SENSE].default);
        detector
    }

    /// 0 hears consonants only when they are unmistakable; 1 hears them
    /// readily.
    fn set_sense(&mut self, sense: f32) {
        let sense = sense.clamp(0.0, 1.0);
        self.ratio_threshold_db = (-24.0f32).mul_add(sense, 6.0);
        self.crossing_threshold = (-2_800.0f32).mul_add(sense, 4_000.0);
    }

    fn sample(&mut self, input: f32) {
        let x = self.dc.process(input);
        let once = self.high[0].highpass(x);
        let high = self.high[1].highpass(once);
        let once = self.low[0].lowpass(x);
        let low = self.low[1].lowpass(once);
        self.high_power = high
            .mul_add(high, -self.high_power)
            .mul_add(self.follow, self.high_power);
        self.low_power = low
            .mul_add(low, -self.low_power)
            .mul_add(self.follow, self.low_power);
        self.total_power = x
            .mul_add(x, -self.total_power)
            .mul_add(self.follow, self.total_power);
        // Crossings with a little hysteresis, so the noise floor of a quiet
        // input does not count.
        let crossed = if self.positive { x < -1e-4 } else { x > 1e-4 };
        if crossed {
            self.positive = !self.positive;
        }
        let hit = if crossed { 1.0 } else { 0.0 };
        self.crossings = (hit - self.crossings).mul_add(self.crossing_pole, self.crossings);
        let speed = if self.target > self.score {
            self.rise
        } else {
            self.fall
        };
        self.score = (self.target - self.score).mul_add(speed, self.score);
    }

    /// Re-judge the last few milliseconds; called at control rate.
    fn decide(&mut self) {
        for power in [
            &mut self.high_power,
            &mut self.low_power,
            &mut self.total_power,
        ] {
            if !power.is_finite() || *power < 1e-20 {
                *power = 0.0;
            }
        }
        if !self.crossings.is_finite() {
            self.crossings = 0.0;
        }
        if !self.score.is_finite() {
            self.score = 0.0;
        }
        let ratio_db = 10.0 * ((self.high_power + 1e-12) / (self.low_power + 1e-12)).log10();
        let level_db = 10.0 * (self.total_power + 1e-12).log10();
        let crossings_per_second = self.crossings * self.sample_rate;
        let spectral = smoothstep((ratio_db - self.ratio_threshold_db + 6.0) / 12.0);
        let busy = smoothstep((crossings_per_second - self.crossing_threshold + 1_000.0) / 2_000.0);
        let loud = smoothstep((level_db + 66.0) / 12.0);
        self.target = spectral * busy * loud;
    }

    fn reset(&mut self) {
        self.dc.reset();
        for filter in self.high.iter_mut().chain(self.low.iter_mut()) {
            filter.reset();
        }
        self.high_power = 0.0;
        self.low_power = 0.0;
        self.total_power = 0.0;
        self.crossings = 0.0;
        self.positive = false;
        self.score = 0.0;
        self.target = 0.0;
    }
}

/// A studio channel vocoder: see the [module documentation](self).
#[derive(Debug)]
pub struct Vocoder {
    sample_rate: f32,
    designs: Vec<Design>,
    bands: Vec<Band>,
    active: usize,
    wanted: usize,
    duck: f32,
    duck_step: f32,
    values: [f32; param::COUNT],
    shift: Smoothed,
    designed_shift: f32,
    emphasis: Smoothed,
    designed_emphasis: f32,
    unvoiced: Smoothed,
    dry: Smoothed,
    mix: Smoothed,
    level: Smoothed,
    detector: Detector,
    noise: [Noise; 2],
    countdown: usize,
}

impl Vocoder {
    /// A vocoder ready to run at `sample_rate` (a non-finite or non-positive
    /// rate is taken as 48 kHz), every parameter at its default. Allocates.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let mut values = [0.0; param::COUNT];
        for (value, spec) in values.iter_mut().zip(&PARAMS) {
            *value = spec.default;
        }
        let mut vocoder = Self {
            sample_rate: 48_000.0,
            designs: Vec::new(),
            bands: Vec::new(),
            active: 0,
            wanted: PARAMS[param::BANDS].default as usize,
            duck: 1.0,
            duck_step: 0.0,
            values,
            shift: Smoothed::new(0.0),
            designed_shift: 0.0,
            emphasis: Smoothed::new(0.0),
            designed_emphasis: 0.0,
            unvoiced: Smoothed::new(0.0),
            dry: Smoothed::new(0.0),
            mix: Smoothed::new(0.0),
            level: Smoothed::new(0.0),
            detector: Detector::new(48_000.0),
            noise: [Noise::new(0x5EED_0001), Noise::new(0x5EED_0002)],
            countdown: 0,
        };
        vocoder.prepare(sample_rate);
        vocoder
    }

    /// Re-size for `sample_rate` (a non-finite or non-positive rate is taken
    /// as 48 kHz) and forget the old audio, keeping the parameters. Designs
    /// the analysis bank for every band count. Allocates: call it off the
    /// audio thread.
    pub fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };
        self.designs = (MIN_BANDS..=MAX_BANDS)
            .map(|count| Design::new(count, self.sample_rate))
            .collect();
        self.bands = vec![Band::default(); MAX_BANDS];
        self.detector = Detector::new(self.sample_rate);
        self.detector.set_sense(self.values[param::SENSE]);
        let control_rate = self.sample_rate / CONTROL_BLOCK as f32;
        self.shift.set_time(GLIDE_SECONDS, control_rate);
        self.emphasis.set_time(GLIDE_SECONDS, control_rate);
        for smoothed in [
            &mut self.unvoiced,
            &mut self.dry,
            &mut self.mix,
            &mut self.level,
        ] {
            smoothed.set_time(SMOOTH_SECONDS, self.sample_rate);
        }
        self.reset();
    }

    /// The sample rate it runs at.
    #[must_use]
    pub const fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Silence every band and follower and settle every glide on its
    /// target, keeping the parameters. A pending band-count change is
    /// applied at once. Real-time safe.
    pub fn reset(&mut self) {
        for band in &mut self.bands {
            band.analysis.reset();
            for side in &mut band.synthesis {
                side.reset();
            }
            band.power = 0.0;
        }
        self.detector.reset();
        self.shift.snap(self.values[param::SHIFT]);
        self.emphasis.snap(self.values[param::EMPHASIS]);
        self.unvoiced.snap(self.values[param::UNVOICED]);
        self.dry.snap(self.values[param::DRY]);
        self.mix.snap(self.values[param::MIX]);
        self.level.snap(db_to_gain(self.values[param::LEVEL]));
        self.duck = 1.0;
        self.duck_step = 0.0;
        self.countdown = 0;
        self.switch_design();
    }

    /// Set parameter `index` (see [`param`]) to `value`, held within its
    /// range. A NaN or an index out of range is ignored. Real-time safe.
    pub fn set_param(&mut self, index: usize, value: f32) {
        if value.is_nan() {
            return;
        }
        let Some(spec) = PARAMS.get(index) else {
            return;
        };
        let value = spec.clamp(value);
        self.values[index] = value;
        match index {
            param::BANDS => self.wanted = value as usize,
            param::ATTACK | param::RELEASE => self.set_ballistics(),
            param::SHIFT => self.shift.set(value),
            param::UNVOICED => self.unvoiced.set(value),
            param::SENSE => self.detector.set_sense(value),
            param::EMPHASIS => self.emphasis.set(value),
            param::DRY => self.dry.set(value),
            param::MIX => self.mix.set(value),
            param::LEVEL => self.level.set(db_to_gain(value)),
            _ => {}
        }
    }

    /// The value parameter `index` was last set to (its default if never
    /// set), or `None` for an index out of range.
    #[must_use]
    pub fn param(&self, index: usize) -> Option<f32> {
        self.values.get(index).copied()
    }

    /// How many bands are sounding now. After a band-count change this
    /// follows a few milliseconds later, once the switch has faded through.
    #[must_use]
    pub fn active_bands(&self) -> usize {
        self.layout().count()
    }

    /// The centre of every analysis band now sounding, lowest first, in Hz.
    #[must_use]
    pub fn band_centres(&self) -> &[f32] {
        self.designs
            .get(self.active)
            .map_or(&[], |design| design.layout.centres())
    }

    /// The loudness each analysis band's follower holds now (the peak
    /// amplitude of a steady sine in that band), lowest band first. For
    /// meters; allocates nothing.
    pub fn band_levels(&self) -> impl Iterator<Item = f32> + '_ {
        self.bands[..self.active_bands().min(self.bands.len())]
            .iter()
            .map(|band| band.power.max(0.0).sqrt() * SQRT_2)
    }

    /// How sure the detector is that the modulator is a consonant right
    /// now, 0 to 1.
    #[must_use]
    pub const fn unvoiced_level(&self) -> f32 {
        self.detector.score
    }

    /// Vocode one block: `carrier` left and right (feed a mono carrier as
    /// the same slice twice) spoken through `modulator`, into `output` left
    /// and right. If the lengths differ, the shortest is processed and the
    /// rest of the outputs is silenced. Real-time safe.
    pub fn process(&mut self, carrier: [&[f32]; 2], modulator: &[f32], output: [&mut [f32]; 2]) {
        let [out_left, out_right] = output;
        let len = carrier[0]
            .len()
            .min(carrier[1].len())
            .min(modulator.len())
            .min(out_left.len())
            .min(out_right.len());
        if self.bands.len() < MAX_BANDS || self.designs.is_empty() {
            out_left.fill(0.0);
            out_right.fill(0.0);
            return;
        }
        for frame in 0..len {
            if self.countdown == 0 {
                self.control();
                self.countdown = CONTROL_BLOCK;
            }
            self.countdown -= 1;
            let voice = finite(modulator[frame]);
            let sides = [finite(carrier[0][frame]), finite(carrier[1][frame])];
            let [left, right] = self.frame(voice, sides);
            out_left[frame] = left;
            out_right[frame] = right;
        }
        out_left[len..].fill(0.0);
        out_right[len..].fill(0.0);
    }

    /// One frame through the banks and the output stage.
    fn frame(&mut self, voice: f32, carrier: [f32; 2]) -> [f32; 2] {
        self.detector.sample(voice);
        let consonant = self.detector.score * self.unvoiced.step();
        let noise = [
            self.noise[0].sample() * NOISE_LEVEL,
            self.noise[1].sample() * NOISE_LEVEL,
        ];
        let hold = self.values[param::HOLD] >= 0.5;
        let design = &self.designs[self.active];
        let count = design.layout.count();
        let mut wet = [0.0f32; 2];
        for (band, coeffs) in self.bands[..count].iter_mut().zip(&design.analysis) {
            let analysed = band.analysis.process(coeffs, f64::from(voice)) as f32;
            if !hold {
                let power = analysed * analysed;
                let speed = if power > band.power {
                    band.attack
                } else {
                    band.release
                };
                band.power = (power - band.power).mul_add(speed, band.power);
            }
            if !band.carrier_live {
                continue;
            }
            let amplitude = band.power.max(0.0).sqrt() * SQRT_2 * band.tilt;
            let blend = consonant * band.noise_weight;
            for side in 0..2 {
                let source = (noise[side] - carrier[side]).mul_add(blend, carrier[side]);
                let voiced = band.synthesis[side].process(&band.carrier, f64::from(source)) as f32;
                wet[side] = voiced.mul_add(amplitude, wet[side]);
            }
        }
        let duck = self.duck;
        self.step_duck();
        let dry = self.dry.step();
        let mix = self.mix.step();
        let level = self.level.step();
        let mut out = [0.0; 2];
        for side in 0..2 {
            let vocoded = wet[side] * MAKEUP * duck;
            let blended = (vocoded - carrier[side]).mul_add(mix, carrier[side]);
            let sample = dry.mul_add(voice, blended) * level;
            out[side] = finite(sample).clamp(-SAFETY_CEILING, SAFETY_CEILING);
        }
        out
    }

    /// Advance the fade around a band-count switch by one sample, making
    /// the switch when the fade out reaches silence.
    fn step_duck(&mut self) {
        if self.duck_step < 0.0 {
            self.duck += self.duck_step;
            if self.duck <= 0.0 {
                self.duck = 0.0;
                self.switch_design();
                self.duck_step = 1.0 / (DUCK_SECONDS * self.sample_rate).max(1.0);
            }
        } else if self.duck_step > 0.0 {
            self.duck += self.duck_step;
            if self.duck >= 1.0 {
                self.duck = 1.0;
                self.duck_step = 0.0;
            }
        }
    }

    /// Control-rate work: the detector's judgement, a pending band-count
    /// switch, the formant shift and emphasis glides, denormal flushing.
    fn control(&mut self) {
        self.detector.decide();
        if self.wanted != self.layout().count() && self.duck_step >= 0.0 {
            self.duck_step = -1.0 / (DUCK_SECONDS * self.sample_rate).max(1.0);
        }
        let shift = self.shift.step();
        if (shift - self.designed_shift).abs() > 1e-3 {
            self.tune_carriers(shift);
        }
        let emphasis = self.emphasis.step();
        if (emphasis - self.designed_emphasis).abs() > 1e-3 {
            self.set_tilt(emphasis);
        }
        for band in &mut self.bands {
            band.analysis.flush();
            for side in &mut band.synthesis {
                side.flush();
            }
            if !band.power.is_finite() || band.power < 1e-20 {
                band.power = 0.0;
            }
        }
    }

    fn layout(&self) -> Layout {
        self.designs.get(self.active).map_or_else(
            || Layout::new(MIN_BANDS, self.sample_rate),
            |design| design.layout,
        )
    }

    /// Make the wanted band count the sounding one: re-tune everything that
    /// depends on where the bands sit and clear the old bands' state.
    fn switch_design(&mut self) {
        let index = self.wanted.clamp(MIN_BANDS, MAX_BANDS) - MIN_BANDS;
        if index >= self.designs.len() || self.bands.len() < MAX_BANDS {
            return;
        }
        self.active = index;
        for band in &mut self.bands {
            band.analysis.reset();
            for side in &mut band.synthesis {
                side.reset();
            }
            band.power = 0.0;
        }
        let (low, high) = NOISE_SPAN_HZ;
        let centres = self.designs[index].layout.centres();
        for (band, &centre) in self.bands.iter_mut().zip(centres) {
            band.noise_weight = smoothstep((centre - low) / (high - low));
        }
        self.set_ballistics();
        self.tune_carriers(self.shift.value());
        self.set_tilt(self.emphasis.value());
    }

    /// Place the carrier bank `shift` semitones from the analysis bank.
    fn tune_carriers(&mut self, shift: f32) {
        self.designed_shift = shift;
        let ratio = (shift / 12.0).exp2();
        let layout = self.layout();
        let ceiling = self.sample_rate * 0.45;
        for ((band, &centre), &q) in self.bands.iter_mut().zip(layout.centres()).zip(layout.qs()) {
            let moved = centre * ratio;
            band.carrier_live = moved > LOWEST_CARRIER_HZ && moved < ceiling;
            if band.carrier_live {
                band.carrier = Coeffs::band_section(moved, q, self.sample_rate);
            }
        }
    }

    /// Tilt the band outputs from 0 dB at the lowest band to `emphasis` dB
    /// at the highest, evenly in Bark.
    fn set_tilt(&mut self, emphasis: f32) {
        self.designed_emphasis = emphasis;
        let count = self.layout().count();
        let last = (count - 1).max(1) as f32;
        for (index, band) in self.bands[..count].iter_mut().enumerate() {
            band.tilt = db_to_gain(emphasis * index as f32 / last);
        }
    }

    /// Follower coefficients for every band. The followers track power, so
    /// each time constant is halved to give the knob's time on amplitude. A
    /// band's follower is never faster than half its own period on attack,
    /// or its whole period on release, so the low bands measure loudness
    /// rather than ripple.
    fn set_ballistics(&mut self) {
        let attack = self.values[param::ATTACK] / 1_000.0;
        let release = self.values[param::RELEASE] / 1_000.0;
        let layout = self.layout();
        for (band, &centre) in self.bands.iter_mut().zip(layout.centres()) {
            let period = 1.0 / centre.max(1.0);
            band.attack = pole(0.5 * attack.max(0.5 * period), self.sample_rate);
            band.release = pole(0.5 * release.max(period), self.sample_rate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const RATE: f32 = 48_000.0;

    fn sine(hz: f32, len: usize, amplitude: f32) -> Vec<f32> {
        (0..len)
            .map(|n| amplitude * (TAU * hz * n as f32 / RATE).sin())
            .collect()
    }

    fn saw(hz: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| ((hz * n as f32 / RATE).fract()).mul_add(2.0, -1.0) * 0.5)
            .collect()
    }

    /// White noise through an eighth-order band around `centre`.
    fn band_noise(centre: f32, q: f32, len: usize, seed: u32) -> Vec<f32> {
        let coeffs = Coeffs::band_section(centre, q, RATE);
        let mut bands = [Band4::default(); 2];
        let mut noise = Noise::new(seed);
        (0..len)
            .map(|_| {
                let once = bands[0].process(&coeffs, f64::from(noise.sample()));
                bands[1].process(&coeffs, once) as f32
            })
            .collect()
    }

    /// A vowel-like voice: a 120 Hz pulse train's harmonics shaped by
    /// formants at 700 Hz and 1.2 kHz, nothing much above 3 kHz.
    fn vowel(len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| {
                let t = n as f32 / RATE;
                (1..=25)
                    .map(|harmonic| {
                        let hz = 120.0 * harmonic as f32;
                        let first = (-((hz - 700.0) / 250.0).powi(2)).exp();
                        let second = (-((hz - 1_200.0) / 300.0).powi(2)).exp();
                        let formant = 0.6f32.mul_add(second, first);
                        (0.02 + formant) / harmonic as f32 * (TAU * hz * t).sin()
                    })
                    .sum::<f32>()
                    * 0.5
            })
            .collect()
    }

    fn run(vocoder: &mut Vocoder, carrier: &[f32], modulator: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut left = vec![0.0; carrier.len()];
        let mut right = vec![0.0; carrier.len()];
        for ((c, m), (l, r)) in carrier
            .chunks(256)
            .zip(modulator.chunks(256))
            .zip(left.chunks_mut(256).zip(right.chunks_mut(256)))
        {
            vocoder.process([c, c], m, <[&mut [f32]; 2]>::from((l, r)));
        }
        (left, right)
    }

    fn rms(block: &[f32]) -> f32 {
        (block.iter().map(|x| x * x).sum::<f32>() / block.len().max(1) as f32).sqrt()
    }

    #[test]
    fn only_the_excited_bands_speak() {
        let len = RATE as usize;
        let carrier = sine(1_000.0, len, 0.5);
        // Consonant noise is a separate path, tested on its own below.
        let quiet = |vocoder: &mut Vocoder| vocoder.set_param(param::UNVOICED, 0.0);
        // Noise around 1 kHz excites the band the carrier sits in.
        let mut vocoder = Vocoder::new(RATE);
        quiet(&mut vocoder);
        let (inside, _) = run(&mut vocoder, &carrier, &band_noise(1_000.0, 6.0, len, 7));
        let levels: Vec<f32> = vocoder.band_levels().collect();
        let centres = vocoder.band_centres().to_vec();
        let loudest = levels.iter().copied().fold(0.0f32, f32::max);
        for (&centre, &level) in centres.iter().zip(&levels) {
            let octaves = (centre / 1_000.0).log2().abs();
            if octaves > 1.5 {
                assert!(level < loudest * 0.05, "band {centre} Hz at {level}");
            }
        }
        // Noise around 5 kHz leaves the 1 kHz carrier band silent.
        let mut vocoder = Vocoder::new(RATE);
        quiet(&mut vocoder);
        let (outside, _) = run(&mut vocoder, &carrier, &band_noise(5_000.0, 6.0, len, 7));
        let settled = len / 2..len;
        let inside = rms(&inside[settled.clone()]);
        let outside = rms(&outside[settled]);
        assert!(inside > 0.01, "{inside}");
        // Fourth-order skirts leave the far bands about 28 dB down.
        assert!(outside < inside * 0.06, "{outside} vs {inside}");
    }

    #[test]
    fn band_count_changes_switch_designs() {
        let mut vocoder = Vocoder::new(RATE);
        assert_eq!(vocoder.active_bands(), 20);
        let carrier = saw(110.0, 4_800);
        let voice = vowel(4_800);
        vocoder.set_param(param::BANDS, 33.4);
        assert_eq!(vocoder.param(param::BANDS), Some(33.0));
        run(&mut vocoder, &carrier, &voice);
        assert_eq!(vocoder.active_bands(), 33);
        assert_eq!(vocoder.band_centres().len(), 33);
        vocoder.set_param(param::BANDS, 2.0);
        run(&mut vocoder, &carrier, &voice);
        assert_eq!(vocoder.active_bands(), MIN_BANDS);
    }

    #[test]
    fn a_band_switch_does_not_click() {
        let mut vocoder = Vocoder::new(RATE);
        let len = RATE as usize / 2;
        let carrier = saw(110.0, len);
        let voice = vowel(len);
        run(&mut vocoder, &carrier, &voice);
        vocoder.set_param(param::BANDS, 40.0);
        let (left, _) = run(&mut vocoder, &carrier[..4_800], &voice[..4_800]);
        let jump = left
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0f32, f32::max);
        let steady = left.iter().fold(0.0f32, |peak, x| peak.max(x.abs()));
        assert!(jump < steady.max(1e-3), "jump {jump} against peak {steady}");
    }

    #[test]
    fn sibilance_is_heard_and_vowels_are_not() {
        let len = RATE as usize / 2;
        let carrier = saw(110.0, len);
        let mut vocoder = Vocoder::new(RATE);
        run(&mut vocoder, &carrier, &band_noise(7_000.0, 1.5, len, 3));
        assert!(
            vocoder.unvoiced_level() > 0.8,
            "{}",
            vocoder.unvoiced_level()
        );

        let mut vocoder = Vocoder::new(RATE);
        let voice = vowel(len);
        let mut highest = 0.0f32;
        for (c, m) in carrier.chunks(64).zip(voice.chunks(64)) {
            let mut left = [0.0; 64];
            let mut right = [0.0; 64];
            vocoder.process([c, c], m, [&mut left[..c.len()], &mut right[..c.len()]]);
            highest = highest.max(vocoder.unvoiced_level());
        }
        assert!(highest < 0.1, "{highest}");
    }

    #[test]
    fn consonant_noise_brightens_a_dull_carrier() {
        // A pure 200 Hz carrier has nothing up high; sibilance must still
        // come through the high bands.
        let len = RATE as usize / 2;
        let carrier = sine(200.0, len, 0.5);
        let hiss = band_noise(7_000.0, 1.5, len, 11);
        let mut without = Vocoder::new(RATE);
        without.set_param(param::UNVOICED, 0.0);
        let (plain, _) = run(&mut without, &carrier, &hiss);
        let mut with = Vocoder::new(RATE);
        with.set_param(param::UNVOICED, 1.0);
        let (bright, _) = run(&mut with, &carrier, &hiss);
        let settled = len / 2..len;
        assert!(rms(&bright[settled.clone()]) > 10.0 * rms(&plain[settled]));
    }

    #[test]
    fn silence_in_is_silence_out() {
        let mut vocoder = Vocoder::new(RATE);
        let zeros = vec![0.0; 9_600];
        let (left, right) = run(&mut vocoder, &zeros, &zeros);
        assert!(left.iter().chain(&right).all(|x| x.abs() < 1e-9));
        // A carrier with no voice is silent too, once fully wet.
        let (left, _) = run(&mut vocoder, &saw(110.0, 9_600), &zeros);
        assert!(left.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn poison_in_is_silence_out_and_does_not_linger() {
        let mut vocoder = Vocoder::new(RATE);
        let poison = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0e30, -1.0e30];
        let carrier: Vec<f32> = poison.iter().copied().cycle().take(4_800).collect();
        let (left, right) = run(&mut vocoder, &carrier, &carrier);
        assert!(
            left.iter()
                .chain(&right)
                .all(|x| x.is_finite() && x.abs() <= SAFETY_CEILING)
        );
        let (left, _) = run(&mut vocoder, &saw(110.0, 9_600), &vowel(9_600));
        assert!(left.iter().all(|x| x.is_finite()));
        assert!(rms(&left[4_800..]) > 1e-3);
        for (index, spec) in PARAMS.iter().enumerate() {
            vocoder.set_param(index, f32::NAN);
            assert_eq!(vocoder.param(index), Some(spec.default));
        }
        vocoder.set_param(param::COUNT + 3, 1.0);
        assert_eq!(vocoder.param(param::COUNT + 3), None);
    }

    #[test]
    fn output_stays_bounded_at_extreme_settings() {
        let len = RATE as usize / 4;
        let carrier = saw(55.0, len);
        let voice = vowel(len);
        for extreme in [0.0f32, 1.0] {
            let mut vocoder = Vocoder::new(RATE);
            for (index, spec) in PARAMS.iter().enumerate() {
                let value = if extreme > 0.5 { spec.max } else { spec.min };
                vocoder.set_param(index, value);
            }
            vocoder.set_param(param::LEVEL, PARAMS[param::LEVEL].max);
            let (left, right) = run(&mut vocoder, &carrier, &voice);
            let peak = left
                .iter()
                .chain(&right)
                .fold(0.0f32, |p, x| p.max(x.abs()));
            assert!(peak.is_finite() && peak <= SAFETY_CEILING, "{peak}");
        }
    }

    #[test]
    fn a_typical_voice_on_a_saw_comes_out_near_unity() {
        let len = RATE as usize;
        let mut vocoder = Vocoder::new(RATE);
        let (left, _) = run(&mut vocoder, &saw(110.0, len), &vowel(len));
        let level = rms(&left[len / 2..]);
        assert!((0.03..1.0).contains(&level), "{level}");
    }

    #[test]
    fn hold_freezes_the_vowel() {
        let len = RATE as usize;
        let carrier = saw(110.0, len);
        let mut vocoder = Vocoder::new(RATE);
        run(&mut vocoder, &carrier, &vowel(len));
        vocoder.set_param(param::HOLD, 1.0);
        let silence = vec![0.0; len];
        let (left, _) = run(&mut vocoder, &carrier, &silence);
        assert!(rms(&left[len / 2..]) > 0.01);
        vocoder.set_param(param::HOLD, 0.0);
        let (left, _) = run(&mut vocoder, &carrier, &silence);
        assert!(rms(&left[len / 2..]) < 1e-3);
    }

    #[test]
    fn formant_shift_moves_the_carrier_bank() {
        let len = RATE as usize / 2;
        // Voice at 1 kHz, carrier a sine at 2 kHz: silent unshifted, heard
        // when the carrier bank is shifted up an octave.
        let voice = band_noise(1_000.0, 6.0, len, 5);
        let carrier = sine(2_000.0, len, 0.5);
        let mut plain = Vocoder::new(RATE);
        plain.set_param(param::UNVOICED, 0.0);
        let (unshifted, _) = run(&mut plain, &carrier, &voice);
        let mut moved = Vocoder::new(RATE);
        moved.set_param(param::UNVOICED, 0.0);
        moved.set_param(param::SHIFT, 12.0);
        moved.reset();
        let (shifted, _) = run(&mut moved, &carrier, &voice);
        let settled = len / 2..len;
        assert!(rms(&shifted[settled.clone()]) > 5.0 * rms(&unshifted[settled]));
    }

    #[test]
    fn mix_and_dry_blend_the_plain_signals() {
        let len = 4_800;
        let carrier = saw(110.0, len);
        let voice = vowel(len);
        let mut vocoder = Vocoder::new(RATE);
        vocoder.set_param(param::MIX, 0.0);
        vocoder.set_param(param::DRY, 1.0);
        vocoder.reset();
        let (left, _) = run(&mut vocoder, &carrier, &voice);
        for ((out, c), v) in left.iter().zip(&carrier).zip(&voice) {
            assert!((out - (c + v)).abs() < 1e-5);
        }
    }

    #[test]
    fn mismatched_lengths_silence_the_rest() {
        let mut vocoder = Vocoder::new(RATE);
        let carrier = saw(110.0, 64);
        let voice = vowel(32);
        let mut left = [1.0; 64];
        let mut right = [1.0; 64];
        vocoder.process([&carrier, &carrier], &voice, [&mut left, &mut right]);
        assert!(
            left[32..]
                .iter()
                .chain(&right[32..])
                .all(|x| x.abs() < f32::EPSILON)
        );
    }

    #[test]
    fn every_param_spec_is_well_formed() {
        for spec in &PARAMS {
            assert!(spec.min < spec.max, "{}", spec.name);
            assert!(
                (spec.min..=spec.max).contains(&spec.default),
                "{}",
                spec.name
            );
            if let Curve::Stepped { labels } = spec.curve {
                assert_eq!(
                    labels.len(),
                    (spec.max - spec.min) as usize + 1,
                    "{}",
                    spec.name
                );
            }
            if spec.curve == Curve::Log {
                assert!(spec.min > 0.0, "{}", spec.name);
            }
        }
    }
}
