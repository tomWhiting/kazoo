//! Juno-inspired procedural polysynth crate.
//!
//! Six DCO-style voices generate saw, pulse, sub-oscillator, and deterministic
//! noise in code. The signal path models the classic simple Juno architecture:
//! DCO mixer -> static HPF -> resonant LPF -> VCA -> noisy stereo chorus.
//! No sample playback or captured waveforms are used.

pub mod synth;

pub use synth::{JunoSynth, NUM_VOICES, SynthParams, VoiceStatus};
