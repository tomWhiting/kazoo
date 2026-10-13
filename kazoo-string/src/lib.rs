//! kazoo-string — plucked-string physical-model synthesizer for the terminal.
//!
//! Plays from the computer keyboard, from the kazoo hub (so kazoo-arp can
//! drive it), or from a looped `kazoo-play` notation phrase given with
//! `--phrase "<notation>"`. All sound is a Karplus-Strong digital waveguide
//! computed in the cpal output callback. Nothing is sampled.

mod args;
mod audio;
mod body;
mod patch;
mod phrase;
mod run;
mod synth;
mod ui;
mod waveguide;

pub use args::{Args, USAGE, cli, parse_args};
pub use run::run;

pub(crate) use run::{App, PhraseState, ROWS, VOLUME_ROW};
