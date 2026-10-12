//! Tuned modal percussion: a fixed bank of damped resonances per voice.
//!
//! The DSP has no audio-device or terminal dependencies. The library entry
//! point connects it to the mixer, transport, notation and keyboard front panel.

mod args;
mod audio;
mod demo;
pub mod patch;
mod phrase;
mod run;
pub mod synth;
mod ui;

pub use args::{Args, USAGE, cli, parse_args};
pub use run::run;

#[cfg(test)]
pub(crate) use run::KeyReleases;
pub(crate) use run::{App, PhraseState, ROWS, VOLUME_ROW, piano_offset};

/// Fail audio-thread tests if the DSP allocates or frees heap memory.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
