//! Reusable TR-808 drum machine: procedural synthesis, sequencer, and standalone TUI.

mod app;
mod audio;
mod run;
mod ui;

pub mod sequencer;
pub mod synth;

pub use run::{Args, USAGE, cli, parse_args, run};
