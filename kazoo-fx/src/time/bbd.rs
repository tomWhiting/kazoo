//! Bucket-brigade analogue delay, in the spirit of the Electro-Harmonix
//! Memory Man and the Boss DM-2.
//!
//! # The model
//!
//! A bucket-brigade chip is a chain of capacitors that pass a sampled
//! voltage along, one bucket per clock tick. This model runs that chain for
//! real: 2048 buckets (a 4096-stage chip, two stages per sample), clocked at
//! `2048 / time` ticks a second. Each tick samples the input between host
//! samples, shifts the chain and hands out the oldest bucket, which is held
//! until the next tick, and the host hears the held voltage averaged over
//! each of its sample periods. So:
//!
//! - **Bandwidth follows the clock.** A long delay means a slow clock, and a
//!   slow clock samples the audio coarsely. The anti-alias and
//!   reconstruction filters are four-pole Butterworths, as on the real
//!   boards. The pedals' filters are fixed, so their short delays are as
//!   dark as their long ones and their longest are gritty with aliasing;
//!   here the filters open to 6 kHz at short times and close down with the
//!   clock at long ones, staying a little above its Nyquist. Short echoes
//!   keep some air, long ones turn dark and a touch gritty, as the pedals'
//!   do.
//! - **Companding.** An NE570-style compressor squeezes the signal 2:1 in
//!   decibels before the chip and an expander restores it after, each from
//!   its own averaging rectifier. The chip's noise sits between them, so it
//!   is pushed down in the gaps and rises with the playing: the familiar
//!   breathing. `level` drives the chip harder into its soft clip. The
//!   compressor's gain also rides along the chain beside each bucket, and
//!   the expander is never allowed past its exact inverse: its own
//!   rectifier still shapes the breathing, but the pair can never add gain,
//!   so even the longest feedback always dies away.
//! - **Modulation.** A triangle LFO wobbles the clock (`rate`, `depth`) for
//!   chorus and vibrato on the echoes; the right channel runs a quarter cycle
//!   ahead. Turning `time` glides the clock, so the echoes bend in pitch.
//! - **Feedback** takes the expanded output back to the input, as the
//!   pedals do, capped at 0.95 and soft-clipped (anti-aliased); the chip's
//!   own clipping holds the loop too.
//!
//! The chip is clocked up to 32 times per host sample, so the shortest time
//! holds even at an 8 kHz host rate.
//!
//! Sources: the Panasonic MN3005/MN3205 and NE570 datasheets, the Memory
//! Man and DM-2 schematics, and Holters and Parker, "A combined model for a
//! bucket brigade device and its input and output filters" (Digital Audio
//! Effects conference, 2018).

use super::parts::{
    Biquad, Follower, Saturator, accept, clean, defaults, equal_power, frames, guard, silence_from,
    triangle,
};
use crate::dsp::{Noise, Phasor, Smoothed, db_to_gain, flush, sane_rate};
use crate::{Context, Curve, Effect, EffectKind, ParamSpec};

const TIME: usize = 0;
const FEEDBACK: usize = 1;
const RATE: usize = 2;
const DEPTH: usize = 3;
const LEVEL: usize = 4;
const MIX: usize = 5;

/// Samples the chip holds: a 4096-stage chip passes one per two stages.
const BUCKETS: usize = 2_048;
const MIN_TIME: f32 = 0.02;
const MAX_TIME: f32 = 0.6;
/// The shortest the modulated delay may get, in seconds.
const FLOOR_TIME: f32 = 0.01;
/// Peak delay swing at full depth, in seconds.
const SWING: f32 = 0.005;
/// The filters' highest corner, and how far above the clock's Nyquist they
/// sit as the clock slows.
const FILTER_HZ: f32 = 6_000.0;
const FILTER_SHARE: f32 = 0.6;
/// Butterworth section qualities for four poles.
const BUTTERWORTH: [f32; 2] = [0.541_196_1, 1.306_563];
/// The compander's reference level and the quietest level it will lift.
const REFERENCE: f32 = 0.25;
const FLOOR: f32 = 1e-4;
const MAX_EXPANSION: f32 = 8.0;
/// The chip's own noise, in the compressed domain.
const CHIP_NOISE: f32 = 2e-4;
const REFRESH: u32 = 32;
/// At most eight ticks per host sample, however low the host rate.
const MIN_TICK: f32 = 1.0 / 32.0;

const PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "time",
        min: MIN_TIME,
        max: MAX_TIME,
        default: 0.3,
        unit: "s",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "feedback",
        min: 0.0,
        max: 0.95,
        default: 0.35,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "rate",
        min: 0.05,
        max: 8.0,
        default: 0.5,
        unit: "Hz",
        curve: Curve::Log,
    },
    ParamSpec {
        name: "depth",
        min: 0.0,
        max: 1.0,
        default: 0.25,
        unit: "",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "level",
        min: -12.0,
        max: 12.0,
        default: 0.0,
        unit: "dB",
        curve: Curve::Linear,
    },
    ParamSpec {
        name: "mix",
        min: 0.0,
        max: 1.0,
        default: 0.35,
        unit: "",
        curve: Curve::Linear,
    },
];

/// The bucket-brigade delay.
pub static KIND: EffectKind = EffectKind {
    id: "bbd",
    name: "Bucket-brigade delay",
    description: "A clocked bucket-brigade chip with companding: warm, darkening and \
                  breathing as the time grows, with chorus on the repeats.",
    params: &PARAMS,
    build,
};

fn build() -> Box<dyn Effect> {
    Box::new(Bbd::new())
}

/// One chip with its filters and compander.
#[derive(Debug, Clone, Default)]
struct Chip {
    buckets: Vec<f32>,
    /// The compressor's gain for each bucket, carried along the chain with
    /// it (see [`Bbd`]'s notes on the compander).
    gains: Vec<f32>,
    index: usize,
    /// How far into the current host sample the next tick falls, in
    /// samples; below 1.0 means it falls in this one.
    until_tick: f32,
    held: f32,
    held_gain: f32,
    last_in: f32,
    anti_alias: [Biquad; 2],
    reconstruct: [Biquad; 2],
    compressor: Follower,
    expander: Follower,
    last_out: f32,
    clip: Saturator,
}

impl Chip {
    fn clear(&mut self) {
        self.buckets.fill(0.0);
        self.gains.fill(1.0);
        self.index = 0;
        self.until_tick = 0.0;
        self.held = 0.0;
        self.held_gain = 1.0;
        self.last_in = 0.0;
        for filter in self.anti_alias.iter_mut().chain(&mut self.reconstruct) {
            filter.reset();
        }
        self.compressor.reset();
        self.expander.reset();
        self.last_out = 0.0;
        self.clip.reset();
    }

    fn set_filters(&mut self, clock: f32, sample_rate: f32) {
        let corner = (clock * FILTER_SHARE).min(FILTER_HZ);
        for (filter, q) in self.anti_alias.iter_mut().zip(BUTTERWORTH) {
            filter.set_lowpass(corner, q, sample_rate);
        }
        for (filter, q) in self.reconstruct.iter_mut().zip(BUTTERWORTH) {
            filter.set_lowpass(corner, q, sample_rate);
        }
    }

    /// Run one host sample through the chip, `per_tick` host samples apart,
    /// with the compressor's gain `gain`; returns the held output and the
    /// held gain, each averaged over the sample.
    fn clock(&mut self, input: f32, gain: f32, per_tick: f32, noise: &mut Noise) -> (f32, f32) {
        if self.buckets.is_empty() || self.gains.len() != self.buckets.len() {
            return (0.0, 1.0);
        }
        let mut at = 0.0;
        let mut heard = 0.0;
        let mut heard_gain = 0.0;
        // Ticks falling in this sample; `per_tick` is at least a 32nd, so
        // there are never more than 33.
        let ticks = ((1.0 - self.until_tick) / per_tick).ceil().max(0.0) as usize;
        for _ in 0..ticks {
            let when = self.until_tick.max(0.0);
            if when >= 1.0 {
                break;
            }
            heard = self.held.mul_add(when - at, heard);
            heard_gain = self.held_gain.mul_add(when - at, heard_gain);
            at = when;
            let sampled = (input - self.last_in).mul_add(when, self.last_in);
            let oldest = self.buckets[self.index];
            let oldest_gain = self.gains[self.index];
            self.buckets[self.index] = noise.sample().mul_add(CHIP_NOISE, sampled.tanh());
            self.gains[self.index] = gain;
            self.index = (self.index + 1) % self.buckets.len();
            self.held = oldest;
            self.held_gain = oldest_gain;
            self.until_tick += per_tick;
        }
        heard = self.held.mul_add(1.0 - at, heard);
        heard_gain = self.held_gain.mul_add(1.0 - at, heard_gain);
        self.until_tick -= 1.0;
        self.last_in = input;
        (heard, heard_gain)
    }
}

/// The bucket-brigade delay.
#[derive(Debug)]
pub struct Bbd {
    rate: f32,
    prepared: bool,
    time: Smoothed,
    feedback: Smoothed,
    speed: Smoothed,
    depth: Smoothed,
    level: Smoothed,
    mix: Smoothed,
    lfo: Phasor,
    noise: Noise,
    chips: [Chip; 2],
    refresh: u32,
}

impl Default for Bbd {
    fn default() -> Self {
        Self::new()
    }
}

impl Bbd {
    /// A bucket-brigade delay at its defaults, unprepared.
    #[must_use]
    pub fn new() -> Self {
        let values = defaults(&PARAMS);
        Self {
            rate: 48_000.0,
            prepared: false,
            time: Smoothed::new(values[TIME]),
            feedback: Smoothed::new(values[FEEDBACK]),
            speed: Smoothed::new(values[RATE]),
            depth: Smoothed::new(values[DEPTH]),
            level: Smoothed::new(values[LEVEL]),
            mix: Smoothed::new(values[MIX]),
            lfo: Phasor::default(),
            noise: Noise::new(0x0DD5_3005),
            chips: [Chip::default(), Chip::default()],
            refresh: 0,
        }
    }

    const fn smoothers(&mut self) -> [&mut Smoothed; 6] {
        [
            &mut self.time,
            &mut self.feedback,
            &mut self.speed,
            &mut self.depth,
            &mut self.level,
            &mut self.mix,
        ]
    }

    fn render(&mut self, input: [&[f32]; 2], output: &mut [&mut [f32]; 2], n: usize) {
        let rate = self.rate;
        for i in 0..n {
            let time = self.time.step();
            let feedback = self.feedback.step();
            let speed = self.speed.step();
            let swing = self.depth.step() * SWING;
            let drive = db_to_gain(self.level.step());
            let (dry, wet) = equal_power(self.mix.step());
            let phase = self.lfo.next(speed, rate);
            let refresh = self.refresh == 0;
            if refresh {
                self.refresh = REFRESH;
            }
            self.refresh -= 1;
            for (c, chip) in self.chips.iter_mut().enumerate() {
                let x = clean(input[c][i]);
                let wobble = triangle(0.25f32.mul_add(c as f32, phase).fract());
                let delay = swing.mul_add(wobble, time).max(FLOOR_TIME);
                let per_tick = (delay * rate / BUCKETS as f32).max(MIN_TICK);
                if refresh {
                    chip.set_filters(BUCKETS as f32 / delay, rate);
                }
                let into = feedback.mul_add(chip.clip.process(chip.last_out, 2.0), x) * drive;
                let squeeze = (REFERENCE / chip.compressor.process(into).max(FLOOR)).sqrt();
                let filtered = cascade(&mut chip.anti_alias, into * squeeze);
                let (held, gain) = chip.clock(filtered, squeeze, per_tick, &mut self.noise);
                let restored = cascade(&mut chip.reconstruct, held);
                // The expander follows its own rectifier, as the NE570's
                // does, but never lifts past the exact inverse of the
                // compression this audio went in with, so the compander as
                // a whole can never gain and the feedback loop always dies.
                let undo = 1.0 / gain.max(1.0 / MAX_EXPANSION);
                let expand = (chip.expander.process(restored) / REFERENCE)
                    .min(MAX_EXPANSION)
                    .min(undo);
                let echo = restored * expand / drive;
                chip.last_out = echo;
                flush(&mut chip.last_out);
                output[c][i] = guard(dry.mul_add(x, wet * echo));
            }
        }
    }
}

/// A sample through each filter of a cascade in turn.
fn cascade(filters: &mut [Biquad; 2], input: f32) -> f32 {
    filters
        .iter_mut()
        .fold(input, |signal, filter| filter.process(signal))
}

impl Effect for Bbd {
    fn prepare(&mut self, sample_rate: f32) {
        let rate = sane_rate(sample_rate);
        self.rate = rate;
        for chip in &mut self.chips {
            chip.buckets = vec![0.0; BUCKETS];
            chip.gains = vec![1.0; BUCKETS];
            // The NE570's rectifier averages over about 20 ms each way.
            chip.compressor.set_times(0.02, 0.02, rate);
            chip.expander.set_times(0.02, 0.02, rate);
        }
        for smoother in self.smoothers() {
            smoother.set_time(0.02, rate);
        }
        // The clock glides a little slower, so time changes bend the pitch.
        self.time.set_time(0.12, rate);
        self.prepared = true;
        self.reset();
    }

    fn reset(&mut self) {
        for chip in &mut self.chips {
            chip.clear();
        }
        for smoother in self.smoothers() {
            smoother.snap(smoother.target());
        }
        self.lfo.set(0.0);
        self.refresh = 0;
    }

    fn set_param(&mut self, index: usize, value: f32) {
        let Some(value) = accept(&PARAMS, index, value) else {
            return;
        };
        match index {
            TIME => self.time.set(value),
            FEEDBACK => self.feedback.set(value),
            RATE => self.speed.set(value),
            DEPTH => self.depth.set(value),
            LEVEL => self.level.set(value),
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
    use crate::time::testkit::{RATE as HOST, context, impulse, noise, prepared, render, rms};

    fn dry_echo(time: f32) -> Box<dyn Effect> {
        let mut bbd = prepared(&KIND);
        bbd.set_param(TIME, time);
        bbd.set_param(FEEDBACK, 0.0);
        bbd.set_param(DEPTH, 0.0);
        bbd.set_param(MIX, 1.0);
        bbd.reset();
        bbd
    }

    #[test]
    fn the_echo_arrives_at_the_time() {
        let mut bbd = dry_echo(0.25);
        let input = impulse(HOST as usize, 0);
        let (left, right) = render(bbd.as_mut(), context(120.0), &input, &input, 256);
        let at = (0.25 * HOST) as usize;
        let early = (0.003 * HOST) as usize;
        let late = (0.03 * HOST) as usize;
        for channel in [&left, &right] {
            let before = rms(&channel[..at - early]);
            let during = rms(&channel[at - early..at + late]);
            // The chip's filters spread the click over a few milliseconds, so
            // its energy per sample is small; what matters is where it is.
            assert!(during > 1e-4, "{during}");
            assert!(before < during * 0.01, "{before} {during}");
            let peak = channel
                .iter()
                .enumerate()
                .fold((0, 0.0f32), |best, (n, v)| {
                    if v.abs() > best.1 { (n, v.abs()) } else { best }
                })
                .0;
            // The chip's filters hold the click's peak back a quarter of a
            // millisecond; never early, and never a millisecond late.
            let millisecond = (0.001 * HOST) as usize;
            assert!(
                (at..at + millisecond).contains(&peak),
                "peak at {peak}, echo due at {at}"
            );
        }
    }

    #[test]
    fn a_longer_time_is_darker() {
        let bright = |time: f32| {
            let mut bbd = dry_echo(time);
            let input = noise(HOST as usize * 2, 0.3, 41);
            let (left, _) = render(bbd.as_mut(), context(120.0), &input, &input, 256);
            let tail = &left[HOST as usize..];
            let edges: Vec<f32> = tail.windows(2).map(|pair| pair[1] - pair[0]).collect();
            rms(&edges) / rms(tail).max(1e-9)
        };
        let short = bright(0.05);
        let long = bright(0.5);
        assert!(long < short * 0.7, "{short} {long}");
    }
}
