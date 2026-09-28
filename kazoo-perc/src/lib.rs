//! Kazoo's percussion, written from scratch.
//!
//! Two kinds of thing live here:
//!
//! - **Voices** ([`Voice`], described by a [`VoiceKind`]): drums, bells,
//!   shakers and noise hits. Each is mono out, struck by
//!   [`Voice::trigger`] and shaped by parameters. The analogue voices model
//!   the circuits of the classic drum machines (bridged-T resonators, square
//!   wave metal, bandpassed noise); the physical voices model the struck
//!   object itself (membranes, bars, bells, particles in a gourd).
//! - **Rhythm generators** ([`Rhythm`], described by a [`RhythmKind`]):
//!   clocked gate patterns: Euclidean, probability with ratchets, a
//!   topographic drum map, polyrhythms and bursts.
//!
//! A host (the wall, an instrument) never needs to know which voice or
//! generator it holds: it builds one from [`catalogue`] or [`rhythms`],
//! calls `prepare` off the audio thread, then feeds it triggers, clocks and
//! knob values. Parameters are described with [`kazoo_fx::ParamSpec`], the
//! same shape the effects use, so one knob widget serves both.
//!
//! # Real-time contract
//!
//! `prepare` may allocate and is only ever called off the audio thread.
//! Every other method never allocates, locks, does I/O or panics, whatever
//! it is given: a NaN or infinite parameter or velocity is ignored, and a
//! voice whose state is ever poisoned silences itself and carries on.
//!
//! # Sample-accurate triggers
//!
//! [`Voice::trigger`] takes effect at the first sample of the next
//! [`Voice::process`] call. A host that wants a hit at sample `n` of a
//! block processes samples `0..n`, triggers, then processes `n..`. Rhythm
//! generators need no such splitting: they read their clock sample by
//! sample and their gate edges land on the exact sample of the clock edge.

pub mod parts;
pub mod rhythm;
pub mod voices;

use std::fmt;

use kazoo_fx::ParamSpec;

/// A struck, mono percussion voice.
///
/// The host numbers parameters as the voice's [`VoiceKind::params`] does.
pub trait Voice: Send + fmt::Debug {
    /// Size everything for `sample_rate` and fall silent. Called off the
    /// audio thread before the first [`Self::process`] and again whenever
    /// the rate changes; the only method that may allocate. A rate that is
    /// not finite or is below 8 kHz is treated as 48 kHz; one above
    /// 768 kHz is held there.
    fn prepare(&mut self, sample_rate: f32);

    /// Silence every tail at once, keeping the parameters. Real-time safe.
    fn reset(&mut self);

    /// Set parameter `index` to `value`. The value is clamped to the
    /// parameter's range; a NaN or an index the voice does not have is
    /// ignored. Continuous parameters glide over a few milliseconds so a
    /// jump never clicks; stepped ones (models, materials) change at once.
    /// Real-time safe.
    fn set_param(&mut self, index: usize, value: f32);

    /// Strike the voice, from the next processed sample.
    ///
    /// `velocity` runs from 0 to 1 and is clamped there; a NaN is ignored.
    /// `accent` plays the hit harder in the way the instrument would: the
    /// analogue voices add level and bite as the drum machines' accent bus
    /// did, the physical ones strike harder and brighter.
    ///
    /// **Choke.** A velocity of exactly 0 (after clamping) strikes nothing:
    /// it chokes the voice, fading whatever is ringing to silence in a few
    /// milliseconds, as a hand on a cymbal or the closed hat cutting the
    /// open one. A host with a separate choke input sends it here as
    /// `trigger(0.0, false)`.
    ///
    /// A strike while the voice still rings takes over without a click: the
    /// analogue voices restart their circuit and fade the old tail out over
    /// a couple of milliseconds, the physical ones add the new strike to the
    /// ringing object as a real one would. Real-time safe.
    fn trigger(&mut self, velocity: f32, accent: bool);

    /// Render the next `out.len()` samples into `out`, overwriting it. A
    /// voice that has not been struck, or has died away, writes exact
    /// silence. Output peaks stay below +6 dBFS whatever the parameters.
    /// Real-time safe.
    fn process(&mut self, out: &mut [f32]);
}

/// What a voice is, before one is built.
#[derive(Debug, Clone, Copy)]
pub struct VoiceKind {
    /// Stable id, lower case: `kick`, `modal`. Saved patches refer to
    /// voices by it, so it never changes once published.
    pub id: &'static str,
    /// Name for people: `Kick drum`.
    pub name: &'static str,
    /// One sentence on what it is and how it sounds.
    pub description: &'static str,
    /// Every parameter, in the order [`Voice::set_param`] numbers them.
    pub params: &'static [ParamSpec],
    /// Build one, unprepared.
    pub build: fn() -> Box<dyn Voice>,
}

/// What a rhythm generator's output carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// A gate: exactly 1.0 while high, 0.0 while low.
    Gate,
    /// A control voltage, 0 up to 1.
    Cv,
}

/// One output of a rhythm generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Output {
    /// Short name, lower case: `gate`, `accent`, `kick`.
    pub name: &'static str,
    /// What it carries.
    pub signal: Signal,
}

/// The most outputs any rhythm generator has.
pub const MAX_OUTPUTS: usize = 4;

/// A clocked gate pattern.
///
/// Every generator is driven by a `clock` input and a `reset` input, both
/// gates (high above 0.5). Each rising edge of the clock is a step; a
/// rising edge of the reset input sends the pattern back to its start, so
/// the next clock edge (or one on the same sample) plays the first step.
/// A step's gate rises on the very sample the clock rises and, unless the
/// generator says otherwise, stays high for as long as the clock does.
pub trait Rhythm: Send + fmt::Debug {
    /// Size everything for `sample_rate` and go back to the start. Called
    /// off the audio thread; the only method that may allocate. The rate
    /// is sanitised as [`Voice::prepare`] describes.
    fn prepare(&mut self, sample_rate: f32);

    /// Go back to the start of the pattern with every output low, keeping
    /// the parameters. Real-time safe.
    fn reset(&mut self);

    /// Set parameter `index` to `value`, clamped to its range; a NaN or an
    /// index the generator does not have is ignored. Takes effect from the
    /// next step. Real-time safe.
    fn set_param(&mut self, index: usize, value: f32);

    /// Run one block. `clock` and `reset` are the input gates; `outputs`
    /// holds one slice per output, in the order of [`RhythmKind::outputs`].
    /// Only the shortest length of the inputs and outputs is run; anything
    /// past it in an output is set low. Missing outputs are simply not
    /// written and extra ones are set low. Non-finite input samples count
    /// as low. Real-time safe.
    fn process(&mut self, clock: &[f32], reset: &[f32], outputs: &mut [&mut [f32]]);
}

/// What a rhythm generator is, before one is built.
#[derive(Debug, Clone, Copy)]
pub struct RhythmKind {
    /// Stable id, lower case: `euclid`, `grids`.
    pub id: &'static str,
    /// Name for people.
    pub name: &'static str,
    /// One sentence on what it does.
    pub description: &'static str,
    /// Every parameter, in the order [`Rhythm::set_param`] numbers them.
    pub params: &'static [ParamSpec],
    /// Every output, in the order [`Rhythm::process`] takes them. Never
    /// more than [`MAX_OUTPUTS`].
    pub outputs: &'static [Output],
    /// Build one, unprepared.
    pub build: fn() -> Box<dyn Rhythm>,
}

/// Every voice.
pub fn catalogue() -> impl Iterator<Item = &'static VoiceKind> {
    voices::KINDS.iter()
}

/// The voice with this id, if there is one.
#[must_use]
pub fn find(id: &str) -> Option<&'static VoiceKind> {
    catalogue().find(|kind| kind.id == id)
}

/// Every rhythm generator.
pub fn rhythms() -> impl Iterator<Item = &'static RhythmKind> {
    rhythm::KINDS.iter()
}

/// The rhythm generator with this id, if there is one.
#[must_use]
pub fn find_rhythm(id: &str) -> Option<&'static RhythmKind> {
    rhythms().find(|kind| kind.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_lower_case_and_findable() {
        let voice_ids: Vec<&str> = catalogue().map(|kind| kind.id).collect();
        let rhythm_ids: Vec<&str> = rhythms().map(|kind| kind.id).collect();
        for (index, id) in voice_ids.iter().chain(rhythm_ids.iter()).enumerate() {
            assert!(id.chars().all(|c| c.is_ascii_lowercase()), "{id}");
            let later = voice_ids
                .iter()
                .chain(rhythm_ids.iter())
                .skip(index + 1)
                .any(|other| other == id);
            assert!(!later, "{id} is listed twice");
        }
        for id in &voice_ids {
            assert_eq!(find(id).map(|kind| kind.id), Some(*id));
        }
        for id in &rhythm_ids {
            assert_eq!(find_rhythm(id).map(|kind| kind.id), Some(*id));
        }
        assert!(find("nope").is_none());
        assert!(find_rhythm("nope").is_none());
    }

    #[test]
    fn every_param_default_is_inside_its_range() {
        let specs = catalogue()
            .flat_map(|kind| kind.params.iter())
            .chain(rhythms().flat_map(|kind| kind.params.iter()));
        for spec in specs {
            assert!(spec.min < spec.max, "{}", spec.name);
            assert!(
                (spec.min..=spec.max).contains(&spec.default),
                "{}",
                spec.name
            );
            if let kazoo_fx::Curve::Stepped { labels } = spec.curve {
                let steps = (spec.max - spec.min).round() as usize + 1;
                assert_eq!(labels.len(), steps, "{}", spec.name);
            }
            if spec.curve == kazoo_fx::Curve::Log {
                assert!(spec.min > 0.0, "{}", spec.name);
            }
        }
    }

    #[test]
    fn rhythm_outputs_fit() {
        for kind in rhythms() {
            assert!(!kind.outputs.is_empty() && kind.outputs.len() <= MAX_OUTPUTS);
        }
    }
}
