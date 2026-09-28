//! Modules adapted from the studio's other crates: `kazoo-perc`'s voices and
//! rhythm generators, and `kazoo-speech`'s vocoder and phrase player.
//!
//! Each becomes a wall module kind (see [`crate::catalogue`]) whose knobs
//! are its parameters. Everything is built and prepared here, on the
//! control side; the modules never allocate once in the engine.

pub mod perc;
pub mod speech;

use kazoo_perc::{RhythmKind, VoiceKind};

use crate::catalogue::{Builder, Candidate, PortSpec, Signal};
use kazoo_speech::PhraseFeed;

use crate::MAX_KNOBS;
use crate::dsp::{Io, Module};

/// Which adapted thing a module kind builds.
#[derive(Debug, Clone, Copy)]
pub enum Adapter {
    /// A percussion voice.
    Voice(&'static VoiceKind),
    /// A rhythm generator.
    Rhythm(&'static RhythmKind),
    /// The channel vocoder.
    Vocoder,
    /// The phrase player the `speak` op feeds.
    Speak,
}

/// Build and prepare a module for `adapter` at `sample_rate` (this
/// allocates). A `speak` module built here has no feed, so it never gets a
/// phrase: the daemon builds speakers with [`build_speaker`].
#[must_use]
pub fn build(adapter: Adapter, sample_rate: f32) -> Box<dyn Module> {
    match adapter {
        Adapter::Voice(kind) => Box::new(perc::VoiceModule::new(kind, sample_rate)),
        Adapter::Rhythm(kind) => Box::new(perc::RhythmModule::new(kind, sample_rate)),
        Adapter::Vocoder => Box::new(speech::VocoderModule::new(sample_rate)),
        Adapter::Speak => build_speaker(sample_rate).0,
    }
}

/// Build a `speak` module and the feed that hands it phrases.
#[must_use]
pub fn build_speaker(sample_rate: f32) -> (Box<dyn Module>, PhraseFeed) {
    let (module, feed) = speech::SpeakModule::new(sample_rate);
    (Box::new(module), feed)
}

/// Hand each knob whose value changed since the last block to `set`, as
/// `(index, value)`. `applied` holds the values handed over; NaN means
/// never (no knob value matches it), so the first block hands over all.
fn apply(io: &Io<'_>, applied: &mut [f32; MAX_KNOBS], mut set: impl FnMut(usize, f32)) {
    for (index, held) in applied.iter_mut().enumerate().take(io.spec.knobs.len()) {
        let value = io.knob(index);
        if held.to_bits() != value.to_bits() {
            set(index, value);
            *held = value;
        }
    }
}

/// Every adapted kind, for the registry: each percussion voice and rhythm
/// generator, the vocoder and the speaker.
#[must_use]
pub fn candidates() -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for kind in kazoo_perc::catalogue() {
        candidates.push(Candidate {
            name: kind.id,
            family: "perc",
            about: format!("{}: {}", kind.name, kind.description),
            params: kind.params,
            inputs: vec![
                port(
                    "trigger",
                    Signal::Gate,
                    "strikes on a rising gate; its level is the velocity",
                ),
                port(
                    "accent",
                    Signal::Gate,
                    "high as it is struck: an accented hit",
                ),
                port("choke", Signal::Gate, "damps the voice on a rising gate"),
            ],
            outputs: vec![port("out", Signal::Audio, "the voice")],
            build: Builder::Adapted(Adapter::Voice(kind)),
        });
    }
    for kind in kazoo_perc::rhythms() {
        candidates.push(Candidate {
            name: kind.id,
            family: "rhythm",
            about: format!("{}: {}", kind.name, kind.description),
            params: kind.params,
            inputs: vec![
                port("clock", Signal::Gate, "steps the pattern on a rising gate"),
                port("reset", Signal::Gate, "the next clock plays the first step"),
            ],
            outputs: kind
                .outputs
                .iter()
                .map(|output| match output.signal {
                    kazoo_perc::Signal::Gate => port(output.name, Signal::Gate, "a pattern lane"),
                    kazoo_perc::Signal::Cv => {
                        port(output.name, Signal::Cv, "a pattern lane, 0 to 1")
                    }
                })
                .collect(),
            build: Builder::Adapted(Adapter::Rhythm(kind)),
        });
    }
    candidates.push(Candidate {
        name: "vocoder",
        family: "speech",
        about: "Vocoder: the carrier speaks with the modulator's mouth".to_string(),
        params: &kazoo_speech::vocoder::PARAMS,
        inputs: vec![
            port("carrier_left", Signal::Audio, "what speaks: left or mono"),
            port(
                "carrier_right",
                Signal::Audio,
                "what speaks: right; the left when unplugged",
            ),
            port("modulator", Signal::Audio, "the voice it speaks with"),
        ],
        outputs: vec![
            port("left", Signal::Audio, "the vocoded sound, left"),
            port("right", Signal::Audio, "the vocoded sound, right"),
        ],
        build: Builder::Adapted(Adapter::Vocoder),
    });
    candidates.push(Candidate {
        name: "speak",
        family: "speech",
        about: "Speaker: plays the words the speak op gave it, on its gate".to_string(),
        params: &kazoo_speech::player::PARAMS,
        inputs: vec![port(
            "gate",
            Signal::Gate,
            "plays the phrase (once, looped or held)",
        )],
        outputs: vec![port("out", Signal::Audio, "the words")],
        build: Builder::Adapted(Adapter::Speak),
    });
    candidates
}

const fn port(name: &'static str, signal: Signal, about: &'static str) -> PortSpec {
    PortSpec {
        name,
        signal,
        about,
    }
}
