//! kazoo-cs80 — Yamaha CS-80 inspired pad synth.
//!
//! 8-voice polyphonic, dual-layer per voice, per-voice analog drift.
//! Also the home for generative/modular synthesis (node graph patching).
//! See `studio/kazoo-cs80.md` for full specification.

mod app;
mod audio;
mod command;
mod input;
pub mod modular;
mod preset;
mod run;
pub mod synth;
mod ui;

pub use run::{Args, USAGE, cli, parse_args, run};
