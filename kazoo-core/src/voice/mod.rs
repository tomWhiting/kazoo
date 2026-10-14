//! Steady voice frames, with callback-safe tracking and a worker-side judge.

mod api;
pub(crate) mod attack;
pub mod gate;
mod gate_settings;
pub use gate_settings::GateSettings;
pub mod judge;
pub mod profile;
pub mod quantise;
pub mod tracker;

pub use api::{
    C4_HZ, LOUDNESS_FLOOR_DB, VOICE_HOP, VoiceEvents, VoiceFrame, hz_from_volts, volts_from_hz,
};
pub use tracker::{TrackSettings, VoiceTracker};

#[cfg(test)]
mod tests;
