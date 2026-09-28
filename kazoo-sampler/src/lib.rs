//! Kazoo's sampler: record your own little sounds, keep them, and play them
//! back beautifully.
//!
//! Four pieces, each usable on its own:
//!
//! - [`SampleStore`]: a directory of WAV files. It names, lists, loads,
//!   saves and deletes samples, converts whatever a WAV holds into stereo
//!   `f32` at the engine's rate with a windowed-sinc [`resample`]r, and keeps
//!   the memory that loaded samples take under a cap.
//! - [`Recorder`] and its [`RecordTap`]: the tap sits on the audio thread and
//!   hands stereo blocks over a lock-free ring to a writer thread, which
//!   streams the take to disk, trims leading silence, normalises and saves
//!   it into the store atomically.
//! - [`SamplePlayer`]: a polyphonic playback voice with one-shot, gate, loop,
//!   ping-pong, slice and granular modes, band-limited pitching and an amp
//!   envelope. It runs on the audio thread.
//! - [`onset`]: the slice-point analyser that runs when a sample is built.
//!
//! # Real-time contract
//!
//! [`RecordTap::process`], [`SamplePlayer::process`] and the player's knob,
//! note and load methods never allocate, lock, do I/O, free memory or panic,
//! whatever they are given. Everything else belongs off the audio thread.
//! A loaded sample reaches the audio thread as an `Arc<SampleData>` and
//! leaves it through the player's [`SampleReaper`], so the last reference is
//! always dropped off the audio thread.

mod error;
mod name;
pub mod onset;
pub mod player;
pub mod recorder;
pub mod resample;
mod sample;
pub mod store;

pub use error::Error;
pub use name::{MAX_NAME_LEN, SampleName};
pub use player::{PlayerInputs, SamplePlayer, SampleReaper};
pub use recorder::{RecordTap, Recorder, RecorderStatus, Take, TakeId, TakeReport, TakeRequest};
pub use sample::SampleData;
pub use store::{Listing, Overwrite, SampleInfo, SampleStore, StoreLimits, Unreadable};

/// The crate's result type.
pub type Result<T> = std::result::Result<T, Error>;

/// The lowest sample rate a sample or an engine may have, in Hz.
pub const MIN_RATE: u32 = 1_000;

/// The highest sample rate a sample or an engine may have, in Hz.
pub const MAX_RATE: u32 = 768_000;

/// Whether `rate` is a sample rate this crate works with.
#[must_use]
pub const fn rate_is_valid(rate: u32) -> bool {
    rate >= MIN_RATE && rate <= MAX_RATE
}
