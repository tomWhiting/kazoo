//! Procedural Juno-style synthesis engine.

mod chorus;
mod engine;
mod envelope;
mod filter;
mod params;
mod voice;

/// Six voices, matching the classic Juno-60/Juno-106 family.
pub const NUM_VOICES: usize = 6;

pub use engine::JunoSynth;
pub use params::{ChorusMode, ChorusParams, DcoParams, EnvelopeParams, FilterParams, SynthParams};
pub use voice::VoiceStatus;
