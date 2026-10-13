//! Steady voice frames, with callback-safe tracking and a worker-side judge.

mod api;
pub mod gate;
pub mod judge;
pub mod quantise;
pub mod tracker;

pub use api::{C4_HZ, LOUDNESS_FLOOR_DB, VOICE_HOP, VoiceFrame, hz_from_volts, volts_from_hz};
pub use tracker::VoiceTracker;

#[cfg(test)]
mod tests;
