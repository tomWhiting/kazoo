//! Module DSP: one realtime processor per module kind.
//!
//! Every module renders one sub-block of [`SUB_BLOCK`] frames at a time,
//! reading its knobs (glided by the engine, and moved by any cable in their
//! jacks), its inputs (already scaled by each cable's amount) and writing its
//! outputs. Modules are built
//! on the control side, where allocating is allowed, and never allocate,
//! lock or panic while processing. Each one holds its outputs in range and
//! turns NaN or infinite input into silence; the engine still checks every
//! output and resets a module that produces a non-finite sample.

mod clock;
pub mod effect;
mod env;
mod lfo;
mod mix;
mod noise;
mod out;
pub mod oversample;
mod quant;
mod seq;
mod sh;
mod slew;
mod vca;
mod vcf;
mod vco;

use std::fmt;

pub use out::{FEED_LEFT, FEED_RIGHT};

use crate::catalogue::{Builder, Kind, KindSpec};
use crate::{MAX_INPUTS, MAX_KNOBS, MAX_OUTPUTS, SUB_BLOCK};

/// One port's samples for one sub-block.
pub type Block = [f32; SUB_BLOCK];

/// The wall's clock for one sub-block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tick {
    /// Frames per second.
    pub sample_rate: f32,
    /// Tempo, in beats per minute.
    pub bpm: f64,
    /// Song position at the sub-block's first frame, in beats.
    pub beat: f64,
    /// Beats per frame.
    pub beat_step: f64,
}

impl Tick {
    /// The clock at `sample_rate` and `bpm`, on `beat`.
    #[must_use]
    pub fn new(sample_rate: f32, bpm: f64, beat: f64) -> Self {
        Self {
            sample_rate,
            bpm,
            beat,
            beat_step: kazoo_core::ipc::follow::beats_in(1.0, bpm, f64::from(sample_rate)),
        }
    }

    /// Song position at frame `frame` of the sub-block.
    #[must_use]
    pub fn beat_at(&self, frame: usize) -> f64 {
        // Frame is below SUB_BLOCK: exact as f64.
        self.beat_step.mul_add(frame as f64, self.beat)
    }

    /// Seconds per beat.
    #[must_use]
    pub fn seconds_per_beat(&self) -> f64 {
        60.0 / self.bpm
    }
}

/// How a knob glides across one sub-block.
///
/// Knobs glide in knob position (0 to 1 along their travel), so a glide on
/// a logarithmic knob such as a cutoff moves by even musical steps. Frame
/// `f` of the sub-block sits at position `from + step × (f + 1)` while `f`
/// is below `frames`; from there on the knob rests on its value.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sweep {
    /// Knob position just before the sub-block's first frame.
    pub from: f32,
    /// Position gained per frame.
    pub step: f32,
    /// Frames of this sub-block still gliding; 0 when the knob is at rest.
    pub frames: u32,
}

impl Sweep {
    /// A knob at rest.
    pub const REST: Self = Self {
        from: 0.0,
        step: 0.0,
        frames: 0,
    };
}

/// A module's view of the engine for one sub-block.
#[derive(Debug)]
pub struct Io<'a> {
    /// The module's kind.
    pub spec: &'static KindSpec,
    /// Knob values as glided by the end of the sub-block, before any jack
    /// moves them, in catalogue order; entries past the kind's knobs are 0.
    /// Read them through [`Io::knob`] and [`Io::knob_at`].
    pub knobs: &'a [f32; MAX_KNOBS],
    /// How each knob glides through the sub-block.
    pub sweeps: &'a [Sweep; MAX_KNOBS],
    /// What arrives at each knob's jack, already scaled by the cable's
    /// amount; zero when unplugged.
    pub knob_cv: &'a [Block; MAX_KNOBS],
    /// Which knob jacks have a cable in.
    pub knob_patched: [bool; MAX_KNOBS],
    /// Input samples, already scaled by each cable's amount; zero when
    /// unplugged.
    pub inputs: &'a [Block; MAX_INPUTS],
    /// Which inputs have a cable in.
    pub connected: [bool; MAX_INPUTS],
    /// Output samples to write, in catalogue order. `out` modules write the
    /// left and right master feed to the first two.
    pub outputs: &'a mut [Block; MAX_OUTPUTS],
}

impl Io<'_> {
    /// Knob `index` once for the whole sub-block: its value at the end of
    /// the sub-block moved by the average of its jack, held to its range. A
    /// knob the kind does not have reads 0.
    ///
    /// Only for what is set once a sub-block or once a note: steps, times,
    /// divisions, and parameters the receiver smooths itself. Anything
    /// that shapes the sound sample by sample reads [`Io::knob_at`], or it
    /// steps every sub-block and zips.
    #[must_use]
    pub fn knob(&self, index: usize) -> f32 {
        let (Some(spec), Some(&value)) = (self.spec.knobs.get(index), self.knobs.get(index)) else {
            return 0.0;
        };
        if !self.knob_patched.get(index).copied().unwrap_or(false) {
            return spec.clamp(value);
        }
        let sum: f32 = self.knob_cv[index].iter().copied().map(finite).sum();
        // SUB_BLOCK is 32: exact.
        spec.modulate(value, sum / SUB_BLOCK as f32)
    }

    /// Knob `index` at `frame` of the sub-block: where its glide has got to
    /// at that frame, moved by its jack at that frame, held to its range.
    #[must_use]
    pub fn knob_at(&self, index: usize, frame: usize) -> f32 {
        let (Some(spec), Some(&value)) = (self.spec.knobs.get(index), self.knobs.get(index)) else {
            return 0.0;
        };
        let sweep = self.sweeps.get(index).copied().unwrap_or(Sweep::REST);
        // Frame is below SUB_BLOCK: exact.
        let value = if (frame as u32) < sweep.frames {
            spec.denormalise(sweep.step.mul_add((frame + 1) as f32, sweep.from))
        } else {
            value
        };
        if !self.knob_patched.get(index).copied().unwrap_or(false) {
            return spec.clamp(value);
        }
        let cv = self.knob_cv[index].get(frame).copied().unwrap_or(0.0);
        spec.modulate(value, finite(cv))
    }

    /// Whether knob `index` can move within this sub-block: it is gliding,
    /// or a cable is in its jack. A knob that cannot move reads the same at
    /// every frame, so work that depends only on it can be done once.
    #[must_use]
    pub fn knob_moves(&self, index: usize) -> bool {
        self.knob_patched(index) || self.sweeps.get(index).is_some_and(|sweep| sweep.frames > 0)
    }

    /// Whether knob `index` has a cable in its jack.
    #[must_use]
    pub fn knob_patched(&self, index: usize) -> bool {
        self.knob_patched.get(index).copied().unwrap_or(false)
    }
}

/// A module's realtime processor.
pub trait Module: Send + fmt::Debug {
    /// Render one sub-block. Must not allocate, lock or panic.
    fn process(&mut self, tick: &Tick, io: Io<'_>);

    /// Forget all state: phases, filters, delay lines, envelopes.
    fn reset(&mut self);
}

/// Build a module of `kind` for a stream at `sample_rate`. This allocates
/// (delay lines, reverb tanks) and prepares effects: call it on the control
/// side only.
#[must_use]
pub fn build(kind: Kind, sample_rate: f32) -> Box<dyn Module> {
    let sample_rate = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
        sample_rate
    } else {
        48_000.0
    };
    match kind.spec().build {
        Builder::Native(build) => build(sample_rate),
        Builder::Effect(effect) => Box::new(effect::EffectModule::new(effect, sample_rate)),
        Builder::Adapted(adapter) => crate::adapters::build(adapter, sample_rate),
    }
}

/// Builders for the wall's own modules, as the catalogue names them.
pub(crate) mod native {
    use super::{Module, clock, env, lfo, mix, noise, out, quant, seq, sh, slew, vca, vcf, vco};

    pub fn vco(sample_rate: f32) -> Box<dyn Module> {
        Box::new(vco::Vco::new(sample_rate))
    }

    pub fn lfo(_sample_rate: f32) -> Box<dyn Module> {
        Box::new(lfo::Lfo::new())
    }

    pub fn noise(sample_rate: f32) -> Box<dyn Module> {
        Box::new(noise::Noise::new(sample_rate))
    }

    pub fn vcf(sample_rate: f32) -> Box<dyn Module> {
        Box::new(vcf::Vcf::new(sample_rate))
    }

    pub fn vca(_sample_rate: f32) -> Box<dyn Module> {
        Box::new(vca::Vca::new())
    }

    pub fn env(_sample_rate: f32) -> Box<dyn Module> {
        Box::new(env::Env::new())
    }

    pub fn clock(_sample_rate: f32) -> Box<dyn Module> {
        Box::new(clock::Clock::new())
    }

    pub fn seq(sample_rate: f32) -> Box<dyn Module> {
        Box::new(seq::Seq::new(sample_rate))
    }

    pub fn sh(_sample_rate: f32) -> Box<dyn Module> {
        Box::new(sh::SampleHold::new())
    }

    pub fn quant(_sample_rate: f32) -> Box<dyn Module> {
        Box::new(quant::Quant::new())
    }

    pub fn slew(_sample_rate: f32) -> Box<dyn Module> {
        Box::new(slew::Slew::new())
    }

    pub fn mix(_sample_rate: f32) -> Box<dyn Module> {
        Box::new(mix::Mix::new())
    }

    pub fn out(_sample_rate: f32) -> Box<dyn Module> {
        Box::new(out::Out::new())
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Gate and trigger threshold.
pub const GATE_HIGH: f32 = 0.5;

/// Largest CV a module passes on: nominal ±1, with headroom for sums.
pub const CV_LIMIT: f32 = 8.0;

/// `sample` if it is a number, else silence.
#[inline]
#[must_use]
pub const fn finite(sample: f32) -> f32 {
    kazoo_core::sanitize_sample(sample)
}

/// A number held to ±[`CV_LIMIT`], silence if it is not a number.
#[inline]
#[must_use]
pub const fn bounded(sample: f32) -> f32 {
    finite(sample).clamp(-CV_LIMIT, CV_LIMIT)
}

/// Finds rising edges on a gate or trigger input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Edge {
    high: bool,
}

impl Edge {
    /// A detector that has seen a low gate.
    #[must_use]
    pub const fn new() -> Self {
        Self { high: false }
    }

    /// Whether `sample` is a rising edge.
    #[inline]
    pub fn rising(&mut self, sample: f32) -> bool {
        let high = sample > GATE_HIGH;
        let rose = high && !self.high;
        self.high = high;
        rose
    }

    /// Whether the gate is high.
    #[must_use]
    pub const fn is_high(self) -> bool {
        self.high
    }

    /// Forget the last level.
    pub const fn reset(&mut self) {
        self.high = false;
    }
}

/// A small, fast random number generator (xorshift32): not for secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rng {
    state: u32,
}

impl Rng {
    /// A generator from `seed` (any value; zero is replaced).
    #[must_use]
    pub const fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 0x9E37_79B9 } else { seed },
        }
    }

    /// The next 32 random bits.
    #[inline]
    pub const fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// A random number from 0 up to 1.
    #[inline]
    pub fn unit(&mut self) -> f32 {
        // The top 24 bits fit an f32 mantissa exactly.
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }

    /// A random number from -1 up to 1.
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        self.unit().mul_add(2.0, -1.0)
    }
}

/// A seed that differs between modules built at different moments.
fn seed() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0x2545_F491);
    NEXT.fetch_add(0x9E37_79B9, Ordering::Relaxed)
}

#[cfg(test)]
pub(crate) mod testing {
    //! Rendering modules outside the engine, for tests.

    use super::{Block, Io, Sweep, Tick};
    use crate::catalogue::{Jack, Kind};
    use crate::{MAX_INPUTS, MAX_KNOBS, MAX_OUTPUTS, SUB_BLOCK};

    /// Test sample rate.
    pub const RATE: f32 = 48_000.0;

    /// A module on a bench: knobs, inputs and a clock to drive it.
    #[derive(Debug)]
    pub struct Bench {
        pub kind: Kind,
        pub module: Box<dyn super::Module>,
        pub knobs: [f32; MAX_KNOBS],
        pub inputs: [Block; MAX_INPUTS],
        pub connected: [bool; MAX_INPUTS],
        pub knob_cv: [Block; MAX_KNOBS],
        pub knob_patched: [bool; MAX_KNOBS],
        pub sweeps: [Sweep; MAX_KNOBS],
        pub outputs: [Block; MAX_OUTPUTS],
        pub bpm: f64,
        pub beat: f64,
        pub rate: f32,
    }

    impl Bench {
        pub fn new(kind: Kind) -> Self {
            Self::at(kind, RATE)
        }

        /// A bench running at `rate`.
        pub fn at(kind: Kind, rate: f32) -> Self {
            let mut knobs = [0.0; MAX_KNOBS];
            for (slot, value) in knobs.iter_mut().zip(kind.spec().defaults()) {
                *slot = value;
            }
            Self {
                kind,
                module: super::build(kind, rate),
                knobs,
                inputs: [[0.0; SUB_BLOCK]; MAX_INPUTS],
                connected: [false; MAX_INPUTS],
                knob_cv: [[0.0; SUB_BLOCK]; MAX_KNOBS],
                knob_patched: [false; MAX_KNOBS],
                sweeps: [Sweep::REST; MAX_KNOBS],
                outputs: [[0.0; SUB_BLOCK]; MAX_OUTPUTS],
                bpm: 120.0,
                beat: 0.0,
                rate,
            }
        }

        pub fn knob(&mut self, name: &str, value: f32) -> &mut Self {
            let spec = self.kind.spec();
            let index = spec.knob_index(name).unwrap();
            self.knobs[index] = spec.knobs[index].clamp(value);
            self
        }

        /// Hold the input or knob jack `jack` at `value` (and mark it
        /// plugged in).
        pub fn hold(&mut self, jack: &str, value: f32) -> &mut Self {
            let buffer = self.jack(jack);
            *buffer = [value; SUB_BLOCK];
            self
        }

        /// The buffer behind `jack`, marked plugged in.
        pub fn jack(&mut self, jack: &str) -> &mut Block {
            match self.kind.spec().jack(jack).unwrap() {
                Jack::Input(index) => {
                    self.connected[index] = true;
                    &mut self.inputs[index]
                }
                Jack::Knob(index) => {
                    self.knob_patched[index] = true;
                    &mut self.knob_cv[index]
                }
            }
        }

        /// Render one sub-block.
        pub fn step(&mut self) {
            let tick = Tick::new(self.rate, self.bpm, self.beat);
            self.module.process(
                &tick,
                Io {
                    spec: self.kind.spec(),
                    knobs: &self.knobs,
                    sweeps: &self.sweeps,
                    knob_cv: &self.knob_cv,
                    knob_patched: self.knob_patched,
                    inputs: &self.inputs,
                    connected: self.connected,
                    outputs: &mut self.outputs,
                },
            );
            self.beat = tick.beat_at(SUB_BLOCK);
        }

        /// Render `frames` frames (whole sub-blocks) of output `port`, with
        /// `input` fed from `feed` frame by frame if given.
        pub fn render(&mut self, port: usize, frames: usize) -> Vec<f32> {
            self.render_fed(port, frames, None, |_| 0.0)
        }

        pub fn render_fed(
            &mut self,
            port: usize,
            frames: usize,
            input: Option<&str>,
            mut feed: impl FnMut(usize) -> f32,
        ) -> Vec<f32> {
            let mut out = Vec::with_capacity(frames);
            let mut frame = 0;
            while out.len() < frames {
                if let Some(name) = input {
                    let buffer = self.jack(name);
                    for (sample, target) in buffer.iter_mut().enumerate() {
                        *target = feed(frame + sample);
                    }
                }
                self.step();
                out.extend_from_slice(&self.outputs[port]);
                frame += SUB_BLOCK;
            }
            out.truncate(frames);
            out
        }
    }

    /// Frequency by counting upward zero crossings over `samples`.
    pub fn frequency(samples: &[f32]) -> f32 {
        let mut crossings = Vec::new();
        for i in 1..samples.len() {
            if samples[i - 1] < 0.0 && samples[i] >= 0.0 {
                // Interpolate the crossing point between the two samples.
                let t = -samples[i - 1] / (samples[i] - samples[i - 1]);
                crossings.push((i - 1) as f32 + t);
            }
        }
        assert!(crossings.len() >= 2, "no cycles to measure");
        let span = crossings[crossings.len() - 1] - crossings[0];
        (crossings.len() - 1) as f32 * RATE / span
    }

    /// Amplitude of the sinusoid at `hz` in `samples`, from one bin of a
    /// discrete Fourier transform under a 7-term Blackman-Harris window:
    /// its sidelobes sit near −180 dB, so a strong tone a few bins away
    /// does not leak into a faint one.
    pub fn tone(samples: &[f32], hz: f64, rate: f64) -> f64 {
        const TERMS: [f64; 7] = [
            0.271_051_400_693_42,
            -0.433_297_939_234_48,
            0.218_122_999_543_11,
            -0.065_925_446_388_03,
            0.010_811_742_098_37,
            -0.000_776_584_825_22,
            0.000_013_887_217_35,
        ];
        let n = samples.len() as f64;
        let (mut re, mut im, mut gain) = (0.0, 0.0, 0.0);
        for (i, sample) in samples.iter().enumerate() {
            let x = std::f64::consts::TAU * i as f64 / n;
            let window: f64 = TERMS
                .iter()
                .enumerate()
                .map(|(k, a)| a * (k as f64 * x).cos())
                .sum();
            let (sin, cos) = (std::f64::consts::TAU * hz * i as f64 / rate).sin_cos();
            let weighted = f64::from(*sample) * window;
            re = weighted.mul_add(cos, re);
            im = weighted.mul_add(-sin, im);
            gain += window;
        }
        2.0 * re.hypot(im) / gain
    }

    /// Decibels.
    pub fn db(ratio: f64) -> f64 {
        20.0 * ratio.max(1e-30).log10()
    }

    /// Feed every input NaN, then infinity, and check the module stays
    /// silent-or-finite and recovers.
    pub fn survives_nonsense(kind: Kind) {
        let mut bench = Bench::new(kind);
        let inputs = kind.spec().inputs.len();
        let knobs = kind.spec().knobs.len();
        for poison in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for index in 0..inputs {
                bench.inputs[index] = [poison; SUB_BLOCK];
                bench.connected[index] = true;
            }
            for index in 0..knobs {
                bench.knob_cv[index] = [poison; SUB_BLOCK];
                bench.knob_patched[index] = true;
            }
            for _ in 0..64 {
                bench.step();
                for port in &bench.outputs {
                    for sample in port {
                        assert!(sample.is_finite(), "{kind} passed {sample} for {poison}");
                    }
                }
            }
        }
        for knob in &mut bench.knobs {
            *knob = f32::NAN;
        }
        for _ in 0..16 {
            bench.step();
            for port in &bench.outputs {
                assert!(port.iter().all(|s| s.is_finite()), "{kind} with NaN knobs");
            }
        }
        bench.module.reset();
    }

    /// Render a module with every knob swept and random inputs, and check
    /// every output stays within `limit`.
    pub fn stays_in_range(kind: Kind, limit: f32) {
        let spec = kind.spec();
        let mut rng = super::Rng::new(7);
        let mut bench = Bench::new(kind);
        for round in 0..200 {
            for (index, knob) in spec.knobs.iter().enumerate() {
                bench.knobs[index] = knob.denormalise(rng.unit());
            }
            for index in 0..spec.inputs.len() {
                for sample in &mut bench.inputs[index] {
                    *sample = rng.bipolar() * if round % 3 == 0 { 4.0 } else { 1.0 };
                }
                bench.connected[index] = rng.unit() > 0.2;
            }
            for index in 0..spec.knobs.len() {
                for sample in &mut bench.knob_cv[index] {
                    *sample = rng.bipolar() * if round % 5 == 0 { 3.0 } else { 0.5 };
                }
                bench.knob_patched[index] = rng.unit() > 0.6;
            }
            for _ in 0..8 {
                bench.step();
                for (port, samples) in bench.outputs.iter().enumerate() {
                    for sample in samples {
                        assert!(
                            sample.is_finite() && sample.abs() <= limit,
                            "{kind} output {port} gave {sample}"
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_fire_once_per_rise() {
        let mut edge = Edge::default();
        assert!(!edge.rising(0.0));
        assert!(edge.rising(1.0));
        assert!(!edge.rising(1.0));
        assert!(edge.is_high());
        assert!(!edge.rising(0.2));
        assert!(edge.rising(0.9));
        assert!(!edge.rising(f32::NAN));
    }

    #[test]
    fn random_numbers_stay_in_range_and_vary() {
        let mut rng = Rng::new(0);
        let mut low = false;
        let mut high = false;
        for _ in 0..10_000 {
            let value = rng.unit();
            assert!((0.0..1.0).contains(&value));
            low |= value < 0.1;
            high |= value > 0.9;
            assert!((-1.0..1.0).contains(&rng.bipolar()));
        }
        assert!(low && high);
    }

    #[test]
    fn every_kind_builds() {
        for kind in Kind::all() {
            let mut bench = testing::Bench::new(kind);
            bench.step();
        }
        // A nonsense rate falls back to a usable one.
        let mut module = build(Kind::SEQ, f32::NAN);
        module.reset();
    }

    #[test]
    fn beat_positions_advance_with_the_frame() {
        let tick = Tick::new(48_000.0, 120.0, 2.0);
        assert!((tick.beat_at(0) - 2.0).abs() < f64::EPSILON);
        assert!((tick.beat_at(24) - (2.0 + 24.0 / 24_000.0)).abs() < 1e-12);
        assert!((tick.seconds_per_beat() - 0.5).abs() < f64::EPSILON);
    }
}
