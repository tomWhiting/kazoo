//! Lightweight state snapshots for UI and host integrations.

/// Snapshot of one voice for UI display.
#[derive(Debug, Clone, Copy)]
pub struct VoiceStatus {
    pub index: u8,
    pub active: bool,
    pub releasing: bool,
    pub note: Option<u8>,
    pub drift_cents: f32,
}
