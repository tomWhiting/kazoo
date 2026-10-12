//! kazoo-dx — four-operator FM synthesizer for the terminal.
//!
//! Plays from the computer keyboard, from the kazoo hub (so kazoo-arp can
//! drive it), or from a looped `kazoo-play` notation phrase given with
//! `--phrase "<notation>"`. All sound is sine-wave phase modulation computed in
//! the cpal output callback. Nothing is sampled.

mod args;
mod audio;
mod phrase;
mod run;
mod synth;
mod ui;

pub use args::{Args, USAGE, cli, parse_args};
pub use run::run;

pub(crate) use run::{App, Focus, GLOBAL_FIELDS, PhraseState, piano_offset};
