//! Library pieces for `kazoo-mix`.
//!
//! Everything testable lives here: the callback-owned engine and its EQ, the
//! control model, the lock-free state shared with the audio callback, meter
//! ballistics, and the desk's interaction model and rendering. The binary
//! only opens the audio device and runs the event loop.

pub mod callback;
pub mod controls;
pub mod desk;
pub mod engine;
pub mod eq;
pub mod hub;
pub mod meters;
pub mod metronome;
pub mod patchbay;
pub mod shared;
pub mod song;
pub mod source;
pub mod studio_clock;
pub mod tap;
pub mod terminal;
#[cfg(test)]
mod test_support;
pub mod ui;
pub mod worker;
