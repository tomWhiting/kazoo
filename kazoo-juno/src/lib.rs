//! Juno-inspired procedural polysynth crate.
//!
//! Six DCO-style voices generate saw, pulse, sub-oscillator, and deterministic
//! noise in code. The signal path models the classic simple Juno architecture:
//! DCO mixer -> static HPF -> resonant LPF -> VCA -> noisy stereo chorus.
//! No sample playback or captured waveforms are used.

mod app;
mod audio;
mod run;
pub mod synth;
mod ui;

pub use synth::{JunoSynth, NUM_VOICES, SynthParams, VoiceStatus};

pub use run::{Args, USAGE, cli, parse_args, run};
