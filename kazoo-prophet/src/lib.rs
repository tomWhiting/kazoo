//! Prophet-5 inspired synthesizer crate.
//!
//! Five polyphonic voices, two VCOs per voice, a resonant 4-pole low-pass,
//! ADSR envelopes, poly-mod routing, oscillator sync, and per-voice analog
//! drift. The implementation is original and intentionally models the
//! instrument architecture rather than a circuit clone.

pub mod synth;

pub use synth::{NUM_VOICES, ProphetSynth, SynthParams, VoiceStatus};
