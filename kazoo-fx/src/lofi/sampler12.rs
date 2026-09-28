//! An early sampler's converters.
//!
//! # The model
//!
//! The colour of an E-mu SP-1200 or an Akai S950 is mostly their
//! converters: 12 bits, a low sample rate, and the filters (or lack of
//! them) either side. This models that path, in order:
//!
//! 1. **Input stage.** `drive` into the converter, which clips hard at full
//!    scale as a real ADC does.
//! 2. **Anti-alias filter.** `filter` picks the machine. The SP-1200's input
//!    filter was gentle and fixed (here two poles at 12 kHz), well above
//!    the Nyquist frequency of its 26.04 kHz rate, so treble folds back as
//!    the metallic alias it is loved for. The S950's is steep and follows
//!    the sample rate (here eight Butterworth poles at 0.45 of it). `off`
//!    has none at all.
//! 3. **Sampling.** The input is sampled at the converter's clock, at the
//!    exact instant of each tick (interpolated between host samples), and
//!    quantised to `bits` with no dither.
//! 4. **Pitch-down colour.** The classic trick: sample a record at 45 and
//!    play it back pitched down, so the material was really captured at a
//!    lower rate than the machine plays it. `pitch` sets how far: the
//!    sampling clock runs that many semitones below the playback clock,
//!    which holds each sample for several of its ticks (drop-sample
//!    playback, as the SP-1200 pitches). Nothing is transposed, since the
//!    input is live; you hear the grit, not the pitch.
//! 5. **Output.** A zero-order-hold DAC at `rate`: every step lands at its
//!    exact time, band-limited against the host rate (a polynomial
//!    band-limited step), so the images the hold makes above the
//!    converter's Nyquist frequency are the converter's, not the host's.
//!    Then the reconstruction filter: the SP-1200's four-pole, slightly
//!    resonant output filter at 11 kHz, the S950's steep one at 0.45 of
//!    the rate, or none, leaving the full staircase.

use super::parts::{self, Butterworth, CONTROL, Decibels};
use crate::dsp::{DelayLine, Smoothed};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const FILTER: usize = 0;
const RATE: usize = 1;
const BITS: usize = 2;
const PITCH: usize = 3;
const DRIVE: usize = 4;
const MIX: usize = 5;
const OUTPUT: usize = 6;
const COUNT: usize = 7;

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "filter",
        min: 0.0,
        max: 2.0,
        default: 0.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["sp-1200", "s950", "off"],
        },
    },
    ParamSpec {
        name: "rate",
        min: 4_000.0,
        max: 48_000.0,
        default: 26_040.0,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "bits",
        min: 8.0,
        max: 16.0,
        default: 12.0,
        unit: "",
        curve: Curve::Stepped {
            labels: &["8", "9", "10", "11", "12", "13", "14", "15", "16"],
        },
    },
    ParamSpec {
        name: "pitch",
        min: -24.0,
        max: 0.0,
        default: 0.0,
        unit: "st",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "drive",
        min: -12.0,
        max: 12.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 100.0,
        default: 100.0,
        unit: "%",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "output",
        min: -18.0,
        max: 6.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
];

/// The sampler converters.
pub static KIND: EffectKind = EffectKind {
    id: "sampler12",
    name: "12-bit sampler",
    description: "An early sampler's converters, SP-1200 or S950 style: 12 bits, low sample \
                  rates, the real anti-alias and reconstruction filters or none, and the \
                  pitch-down resampling grit.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Sampler12::new())
}

/// The SP-1200's own input and output filter corners, Hz.
const SP_INPUT: f32 = 12_000.0;
const SP_OUTPUT: f32 = 11_000.0;

/// One channel's converters.
#[derive(Debug, Clone)]
struct Converter {
    input_filter: Butterworth,
    output_filter: Butterworth,
    resonance: parts::Biquad,
    last_in: f32,
    sampled: f32,
    held: f32,
    pending: f32,
    correction: f32,
}

impl Converter {
    fn new() -> Self {
        Self {
            input_filter: Butterworth::default(),
            output_filter: Butterworth::default(),
            resonance: parts::Biquad::new(),
            last_in: 0.0,
            sampled: 0.0,
            held: 0.0,
            pending: 0.0,
            correction: 0.0,
        }
    }

    fn reset(&mut self) {
        self.input_filter.reset();
        self.output_filter.reset();
        self.resonance.reset();
        self.last_in = 0.0;
        self.sampled = 0.0;
        self.held = 0.0;
        self.pending = 0.0;
        self.correction = 0.0;
    }

    /// The ADC: take the (filtered) input at a clock edge `back` samples
    /// before now, and quantise it.
    fn sample(&mut self, filtered: f32, back: f32, steps: f32) {
        let exact = parts::lerp(filtered, self.last_in, back.clamp(0.0, 1.0)).clamp(-1.0, 1.0);
        self.sampled = ((exact * steps).round() / steps).clamp(-1.0, 1.0);
    }

    /// The DAC: step to the sampled value at a clock edge `back` samples
    /// before now. The step is band-limited with a two-sample polynomial:
    /// half of it is spread onto the sample before the edge (still pending,
    /// since the output runs a sample late) and half onto the one after.
    fn step(&mut self, back: f32) {
        let jump = self.sampled - self.held;
        let before = back * back * 0.5;
        let after = (1.0 - back) * (1.0 - back) * 0.5;
        self.pending = jump.mul_add(before, self.pending);
        self.correction = jump.mul_add(-after, self.correction);
        self.held = self.sampled;
    }

    /// The DAC output one sample late, now that every step near it is in.
    fn emit(&mut self) -> f32 {
        let out = self.pending;
        self.pending = self.held + self.correction;
        self.correction = 0.0;
        out
    }
}

/// The two converter clocks.
#[derive(Debug, Clone, Copy, Default)]
struct Clocks {
    record: f64,
    play: f64,
}

/// An early sampler's converters. See the module documentation for the
/// model.
#[derive(Debug, Clone)]
pub struct Sampler12 {
    rate: f32,
    knobs: [Smoothed; COUNT],
    channels: [Converter; 2],
    dry: [DelayLine; 2],
    clocks: Clocks,
    output_gain: Decibels,
    drive_gain: Decibels,
    until_control: usize,
}

/// Knobs that move filters glide at control rate; the rest every sample.
const fn at_control_rate(index: usize) -> bool {
    !matches!(index, DRIVE | MIX | OUTPUT)
}

impl Sampler12 {
    /// A sampler at 48 kHz with every knob at its default.
    #[must_use]
    pub fn new() -> Self {
        let mut sampler = Self {
            rate: 48_000.0,
            knobs: PARAMS.map(|spec| Smoothed::new(spec.default)),
            channels: [Converter::new(), Converter::new()],
            dry: [DelayLine::default(), DelayLine::default()],
            clocks: Clocks::default(),
            output_gain: Decibels::new(),
            drive_gain: Decibels::new(),
            until_control: 0,
        };
        sampler.prepare(48_000.0);
        sampler
    }

    /// The converter rates: (sampling, playback), Hz.
    fn rates(&self) -> (f32, f32) {
        let play = self.knobs[RATE].value();
        let record = play * (self.knobs[PITCH].value() / 12.0).exp2();
        (record, play)
    }

    /// Every [`CONTROL`] samples: glide the slow knobs and move the filters.
    fn control(&mut self) {
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            if at_control_rate(index) {
                knob.step();
            }
        }
        let rate = self.rate;
        let (record, play) = self.rates();
        let top = rate * 0.45;
        let filter = self.knobs[FILTER].target() as usize;
        for channel in &mut self.channels {
            match filter {
                0 => {
                    channel.input_filter.lowpass(1, SP_INPUT.min(top), rate);
                    channel.output_filter.lowpass(2, SP_OUTPUT.min(top), rate);
                    channel
                        .resonance
                        .peak(SP_OUTPUT.min(top) * 0.8, 1.5, 1.5, rate);
                }
                1 => {
                    channel
                        .input_filter
                        .lowpass(4, (0.45 * record).min(top), rate);
                    channel
                        .output_filter
                        .lowpass(4, (0.45 * play).min(top), rate);
                    channel.resonance.peak(1_000.0, 1.0, 0.0, rate);
                }
                _ => {
                    // Wide open: both filters sit at the top of the host's
                    // band, where they only stop the host itself aliasing.
                    channel.input_filter.lowpass(1, top, rate);
                    channel.output_filter.lowpass(1, top, rate);
                    channel.resonance.peak(1_000.0, 1.0, 0.0, rate);
                }
            }
        }
    }

    /// One stereo sample.
    fn tick(&mut self, left: f32, right: f32) -> (f32, f32) {
        if self.until_control == 0 {
            self.control();
            self.until_control = CONTROL;
        }
        self.until_control -= 1;
        let rate = f64::from(self.rate);
        let drive = self.drive_gain.gain(self.knobs[DRIVE].step());
        let mix = self.knobs[MIX].step() / 100.0;
        let output = self.output_gain.gain(self.knobs[OUTPUT].step());
        let steps = (self.knobs[BITS].target() - 1.0).exp2();
        let (record, play) = self.rates();
        let record_step = f64::from(record) / rate;
        let play_step = f64::from(play) / rate;

        let mut filtered = [0.0f32; 2];
        for ((channel, input), value) in self
            .channels
            .iter_mut()
            .zip([left, right])
            .zip(&mut filtered)
        {
            *value = channel.input_filter.process(input * drive);
        }
        // Walk the clock edges that fall inside this sample, in time order.
        self.clocks.record += record_step;
        self.clocks.play += play_step;
        loop {
            let record_back = (self.clocks.record - 1.0) / record_step;
            let play_back = (self.clocks.play - 1.0) / play_step;
            let record_due = self.clocks.record >= 1.0;
            let play_due = self.clocks.play >= 1.0;
            if record_due && (!play_due || record_back >= play_back) {
                self.clocks.record -= 1.0;
                for (channel, value) in self.channels.iter_mut().zip(filtered) {
                    channel.sample(value, record_back as f32, steps);
                }
            } else if play_due {
                self.clocks.play -= 1.0;
                for channel in &mut self.channels {
                    channel.step((play_back as f32).clamp(0.0, 1.0));
                }
            } else {
                break;
            }
        }
        let mut out = [0.0f32; 2];
        for (((channel, line), (input, value)), sample) in self
            .channels
            .iter_mut()
            .zip(&mut self.dry)
            .zip([left, right].into_iter().zip(filtered))
            .zip(&mut out)
        {
            channel.last_in = value;
            let staircase = channel.emit();
            let rebuilt = channel
                .resonance
                .process(channel.output_filter.process(staircase));
            line.push(input);
            *sample = parts::lerp(line.read(1.0), rebuilt, mix) * output;
        }
        out.into()
    }
}

impl Default for Sampler12 {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for Sampler12 {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = parts::sane_rate(sample_rate);
        self.rate = rate;
        let control_rate = rate / CONTROL as f32;
        for (index, knob) in self.knobs.iter_mut().enumerate() {
            if at_control_rate(index) {
                knob.set_time(0.05, control_rate);
            } else {
                knob.set_time(0.02, rate);
            }
        }
        for line in &mut self.dry {
            line.resize(8);
        }
        self.reset();
    }

    fn reset(&mut self) {
        for knob in &mut self.knobs {
            knob.snap(knob.target());
        }
        for channel in &mut self.channels {
            channel.reset();
        }
        for line in &mut self.dry {
            line.clear();
        }
        self.clocks = Clocks::default();
        self.until_control = 0;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        if let Some(spec) = PARAMS.get(index) {
            if value.is_finite() {
                self.knobs[index].set(spec.clamp(value));
            }
        }
    }

    fn process(&mut self, _context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]) {
        parts::run_block(input, output, |left, right| self.tick(left, right));
    }
}

#[cfg(test)]
mod tests {
    use super::super::parts::testkit::{self, built, peak, render, rms, silence, sine};
    use super::*;
    use crate::dsp::gain_to_db;

    /// The level of `hz` in `signal`, by correlation, as linear amplitude.
    fn level_at(signal: &[f32], hz: f32) -> f32 {
        let step = f64::from(hz) / f64::from(testkit::RATE);
        let (mut s, mut c) = (0.0f64, 0.0f64);
        for (n, x) in signal.iter().enumerate() {
            let phase = std::f64::consts::TAU * (step * n as f64).fract();
            s = f64::from(*x).mul_add(phase.sin(), s);
            c = f64::from(*x).mul_add(phase.cos(), c);
        }
        (2.0 * s.hypot(c) / signal.len() as f64) as f32
    }

    #[test]
    fn it_keeps_the_effect_contract() {
        testkit::contract(&KIND);
    }

    #[test]
    fn silence_is_silent() {
        let input = silence(0.5);
        for filter in 0..3 {
            let mut sampler = built(&KIND, &[(FILTER, filter as f32), (BITS, 8.0)]);
            let (left, right) = render(sampler.as_mut(), &input, &input);
            assert!(peak(&left) < 1e-7 && peak(&right) < 1e-7);
        }
    }

    /// What is left of `signal` once the best-fitting sine at `hz` is
    /// taken out, as RMS. Exact when `signal` holds whole periods.
    fn residual(signal: &[f32], hz: f32) -> f32 {
        let step = f64::from(hz) / f64::from(testkit::RATE);
        let phase = |n: usize| std::f64::consts::TAU * (step * n as f64).fract();
        let (mut s, mut c) = (0.0f64, 0.0f64);
        for (n, x) in signal.iter().enumerate() {
            s = f64::from(*x).mul_add(phase(n).sin(), s);
            c = f64::from(*x).mul_add(phase(n).cos(), c);
        }
        let count = signal.len() as f64;
        let (s, c) = (2.0 * s / count, 2.0 * c / count);
        let mut sum = 0.0f64;
        for (n, x) in signal.iter().enumerate() {
            let left = f64::from(*x) - s.mul_add(phase(n).sin(), c * phase(n).cos());
            sum = left.mul_add(left, sum);
        }
        (sum / count).sqrt() as f32
    }

    #[test]
    fn fewer_bits_raise_the_noise_floor() {
        // 480 Hz: a whole number of periods in what is measured.
        let tone = sine(480.0, 0.5, 1.0);
        let floor = |bits: f32| {
            let mut sampler = built(&KIND, &[(FILTER, 2.0), (RATE, 48_000.0), (BITS, bits)]);
            let (left, _) = render(sampler.as_mut(), &tone, &tone);
            gain_to_db(residual(&left[4_800..], 480.0))
        };
        let (eight, sixteen) = (floor(8.0), floor(16.0));
        assert!(eight > sixteen + 30.0, "{eight} {sixteen}");
        assert!(eight > -60.0 && eight < -35.0, "{eight}");
    }

    #[test]
    fn the_s950_filter_stops_aliasing_and_off_lets_it_through() {
        // 6 kHz sampled at 8 kHz folds to 2 kHz unless it is filtered out.
        let tone = sine(6_000.0, 0.5, 1.0);
        let alias = |filter: f32| {
            let mut sampler = built(&KIND, &[(FILTER, filter), (RATE, 8_000.0)]);
            let (left, _) = render(sampler.as_mut(), &tone, &tone);
            level_at(&left[4_800..], 2_000.0)
        };
        let (s950, off) = (alias(1.0), alias(2.0));
        assert!(off > 0.1, "{off}");
        // Eight Butterworth poles: about 35 dB down at 6 kHz.
        assert!(s950 < off * 0.03, "{s950} {off}");
    }

    #[test]
    fn the_hold_images_above_the_converter_band() {
        // 1 kHz held at 8 kHz makes images at 7 and 9 kHz.
        let tone = sine(1_000.0, 0.5, 1.0);
        let image = |filter: f32| {
            let mut sampler = built(&KIND, &[(FILTER, filter), (RATE, 8_000.0)]);
            let (left, _) = render(sampler.as_mut(), &tone, &tone);
            level_at(&left[4_800..], 7_000.0)
        };
        let (off, s950) = (image(2.0), image(1.0));
        assert!(off > 0.02, "{off}");
        assert!(s950 < off * 0.01, "{s950} {off}");
    }

    #[test]
    fn pitching_down_brings_images_into_the_band() {
        // Sampled an octave down (13.02 kHz) but played at 26.04 kHz, a
        // 3 kHz tone images at 10.02 kHz, under the output filter.
        let tone = sine(3_000.0, 0.5, 1.0);
        let image = |pitch: f32| {
            let mut sampler = built(&KIND, &[(FILTER, 1.0), (PITCH, pitch)]);
            let (left, _) = render(sampler.as_mut(), &tone, &tone);
            level_at(&left[4_800..], 10_020.0)
        };
        let (straight, down) = (image(0.0), image(-12.0));
        assert!(down > 0.02 && straight < down * 0.05, "{straight} {down}");
    }

    #[test]
    fn defaults_keep_the_level() {
        let tone = sine(300.0, 0.3, 0.5);
        let mut sampler = built(&KIND, &[]);
        let (left, _) = render(sampler.as_mut(), &tone, &tone);
        let change = gain_to_db(rms(&left[4_800..])) - gain_to_db(rms(&tone[4_800..]));
        assert!(change.abs() < 1.0, "{change}");
    }
}
