//! Kazoo's effects, written from scratch.
//!
//! Every effect is stereo in and stereo out and speaks one small interface,
//! [`Effect`], described by an [`EffectKind`]: its id, its words and its
//! parameters. A host (the wall, a desk strip, an instrument) never needs to
//! know which effect it holds: it builds one from the [`catalogue`], calls
//! [`Effect::prepare`] off the audio thread, then feeds it audio and knob
//! values. That is what lets any effect be patched into anything.
//!
//! The effects come in four families, each in its own module:
//!
//! - [`drive`]: distortion, folding, crushing, ring modulation, dynamics.
//! - [`time`]: delays and the modulation effects built on short delays.
//! - [`space`]: reverbs, resonators and tone shaping.
//! - [`lofi`]: worn media: VHS, cassette and reel tape, vinyl, generation
//!   loss.
//!
//! # Real-time contract
//!
//! [`Effect::prepare`] may allocate and is only ever called off the audio
//! thread. [`Effect::process`], [`Effect::set_param`] and [`Effect::reset`]
//! never allocate, lock, do I/O or panic, whatever they are given: a NaN or
//! infinite input or parameter comes out as silence or is ignored, and the
//! effect's state is never left poisoned by one.

pub mod drive;
pub mod dsp;
pub mod lofi;
pub mod space;
pub mod time;

use std::fmt;

/// How a parameter's knob maps its travel onto its range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    /// Even steps across the range.
    Linear,
    /// Even steps in ratio: frequencies, times. The range must be positive.
    Log,
    /// Whole numbers only, each one named by the matching entry of `labels`
    /// (the value `min + i` is `labels[i]`).
    Stepped {
        /// The name of every step, in order.
        labels: &'static [&'static str],
    },
}

/// One parameter of an effect: what the knob is called, where it can go and
/// where it starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// Short name, lower case, no spaces: `time`, `feedback`, `mix`.
    pub name: &'static str,
    /// Lowest value.
    pub min: f32,
    /// Highest value. Where a high value would be dangerous (runaway
    /// feedback, ear-splitting gain), the cap is here.
    pub max: f32,
    /// Where the knob starts.
    pub default: f32,
    /// Unit shown after the value: `Hz`, `s`, `dB`, `%`, `st`, or empty.
    pub unit: &'static str,
    /// How the knob travels.
    pub curve: Curve,
}

impl ParamSpec {
    /// `value` held inside this parameter's range, rounded to a whole step
    /// for a stepped parameter. A NaN becomes the default.
    #[must_use]
    pub fn clamp(&self, value: f32) -> f32 {
        if value.is_nan() {
            return self.default;
        }
        let held = value.clamp(self.min, self.max);
        match self.curve {
            Curve::Stepped { .. } => held.round(),
            Curve::Linear | Curve::Log => held,
        }
    }
}

/// What an effect is, before one is built.
#[derive(Debug, Clone, Copy)]
pub struct EffectKind {
    /// Stable id, lower case: `plate`, `tape`, `fold`. Saved patches refer
    /// to effects by it, so it never changes once published.
    pub id: &'static str,
    /// Name for people: `Plate reverb`.
    pub name: &'static str,
    /// One sentence on what it does and how it sounds.
    pub description: &'static str,
    /// Every parameter, in the order [`Effect::set_param`] numbers them.
    pub params: &'static [ParamSpec],
    /// Build one, unprepared.
    pub build: fn() -> Box<dyn Effect>,
}

/// What the host tells an effect about the music on every call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Context {
    /// Tempo in beats per minute, always finite and positive. Tempo-synced
    /// effects (delays, tremolo) read it on every call.
    pub bpm: f64,
}

impl Context {
    /// Seconds per beat at this tempo.
    #[must_use]
    pub fn beat_seconds(&self) -> f64 {
        60.0 / self.bpm
    }
}

/// A stereo effect.
///
/// The host numbers parameters as the effect's [`EffectKind::params`] does.
/// A mono source is fed as the same signal on both inputs.
pub trait Effect: Send + fmt::Debug {
    /// Size everything for `sample_rate` (and forget the old audio). Called
    /// off the audio thread before the first [`Self::process`] and again
    /// whenever the rate changes; the only method that may allocate.
    fn prepare(&mut self, sample_rate: f32);

    /// Silence every tail and restart every internal oscillator, keeping the
    /// parameters. Real-time safe.
    fn reset(&mut self);

    /// Set parameter `index` to `value`. The value is clamped to the
    /// parameter's range; a NaN or an index the effect does not have is
    /// ignored. Changes are smoothed inside the effect, so a jump never
    /// clicks. Real-time safe.
    fn set_param(&mut self, index: usize, value: f32);

    /// Process one block: `input` left and right into `output` left and
    /// right. All four slices have the same length, which may be anything
    /// from 0 up; if they differ, only the shortest length is processed and
    /// the rest of the outputs is silenced. Real-time safe.
    fn process(&mut self, context: &Context, input: [&[f32]; 2], output: [&mut [f32]; 2]);

    /// How many samples, at the host rate, the effect delays its input by:
    /// oversampling filters and the like, not the effect's own sound (a
    /// delay's echo is not latency). A host running a dry signal in
    /// parallel delays it by this much to keep the two in phase. Valid
    /// after [`Self::prepare`]. It may change with the rate or with a
    /// stepped (discrete) parameter such as a mode switch, never with a
    /// continuous knob, so a host can re-read it on those events alone.
    /// Real-time safe.
    fn latency(&self) -> usize {
        0
    }
}

/// Every effect in every family.
pub fn catalogue() -> impl Iterator<Item = &'static EffectKind> {
    drive::KINDS
        .iter()
        .chain(time::KINDS.iter())
        .chain(space::KINDS.iter())
        .chain(lofi::KINDS.iter())
}

/// The effect with this id, if there is one.
#[must_use]
pub fn find(id: &str) -> Option<&'static EffectKind> {
    catalogue().find(|kind| kind.id == id)
}
