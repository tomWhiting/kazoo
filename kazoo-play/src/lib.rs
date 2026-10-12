//! kazoo-play — pipe text notation into timestamped musical events or sound.
//!
//! `kazoo-play` is the first terminal-native musical primitive: humans and
//! agents can write compact notation, pipe it around, print timestamped events,
//! or play it immediately through the default audio device.

mod args;
mod jam;
mod run;

pub use args::{Args, Error, Mode, OutputFormat, USAGE, cli, parse_args};
pub use run::{render, run};
