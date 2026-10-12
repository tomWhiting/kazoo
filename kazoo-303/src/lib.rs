//! kazoo-303 — TB-303 inspired acid bassline synthesizer.
//!
//! This instrument is fully procedural: no samples, wavetables, or recordings are
//! played back. The sound is generated from oscillator math, envelopes, glide,
//! accent dynamics, and a resonant low-pass filter.

mod audio;
mod run;
mod sequencer;
mod synth;
mod ui;

pub use run::{Args, USAGE, cli, parse_args, run};

pub(crate) use run::App;
