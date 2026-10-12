//! kazoo-mini — Moog Minimoog inspired bass/lead synth.
//!
//! Monophonic. Three VCOs, 24 dB/oct ladder filter with nonlinear
//! saturation, rate-based glide, cross-modulation.
//! See `studio/kazoo-mini.md` for full specification.
//!
//! Standalone operation: direct cpal audio output with lock-free
//! command channel (UI -> Audio) and display channel (Audio -> UI).

mod app;
mod audio;
mod command;
mod input;
mod params;
mod run;
mod synth;
mod terminal;
mod ui;

pub use run::{Args, USAGE, cli, parse_args, run};
