//! IPC message types and payload serialization.
//!
//! Each message type has a constant tag, a struct for its payload fields,
//! and `encode` / `decode` methods that work directly on byte slices.
//! No allocation on the encode/decode path — all buffers are pre-allocated.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

// ---------------------------------------------------------------------------
// Message type constants
// ---------------------------------------------------------------------------

/// Instrument -> Hub. Registration handshake (sent once on connect).
pub const MSG_REGISTER: u8 = 0x01;
/// Hub -> Instrument. Registration confirmed (assigned strip index).
pub const MSG_REGISTERED: u8 = 0x02;
/// Instrument -> Hub. Audio block (hot path, every buffer cycle).
pub const MSG_AUDIO: u8 = 0x10;
/// Hub -> Instrument. Transport state broadcast.
pub const MSG_TRANSPORT_SYNC: u8 = 0x20;
/// Instrument -> Hub. Transport change request.
pub const MSG_TRANSPORT_REQUEST: u8 = 0x21;
/// Routed through Hub. MIDI-style note events between instruments.
pub const MSG_NOTE_EVENT: u8 = 0x30;
/// Either direction. Mixer parameter update.
pub const MSG_PARAMETER_CHANGE: u8 = 0x40;
/// Either direction. Clean disconnect.
pub const MSG_SHUTDOWN: u8 = 0xFF;

/// Fixed-size instrument name field in the Register message.
pub const REGISTER_NAME_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Transport state constants (wire representation)
// ---------------------------------------------------------------------------

/// Transport stopped (position at zero).
pub const TRANSPORT_STOPPED: u8 = 0;
/// Transport playing.
pub const TRANSPORT_PLAYING: u8 = 1;
/// Transport recording (implies playing).
pub const TRANSPORT_RECORDING: u8 = 2;
/// Transport paused (position preserved).
pub const TRANSPORT_PAUSED: u8 = 3;

// ---------------------------------------------------------------------------
// Note event type constants
// ---------------------------------------------------------------------------

/// MIDI Note On event.
pub const NOTE_ON: u8 = 0;
/// MIDI Note Off event.
pub const NOTE_OFF: u8 = 1;
/// MIDI Continuous Controller event.
pub const NOTE_CC: u8 = 2;
/// MIDI Pitch Bend event.
pub const NOTE_PITCH_BEND: u8 = 3;

// ---------------------------------------------------------------------------
// Parameter constants
// ---------------------------------------------------------------------------

/// Volume parameter on a mixer strip.
pub const PARAM_VOLUME: u8 = 0;
/// Pan parameter on a mixer strip.
pub const PARAM_PAN: u8 = 1;
/// Mute toggle on a mixer strip.
pub const PARAM_MUTE: u8 = 2;
/// Solo toggle on a mixer strip.
pub const PARAM_SOLO: u8 = 3;
/// Record-arm toggle on a mixer strip.
pub const PARAM_ARM: u8 = 4;

// ---------------------------------------------------------------------------
// Instrument ID generation
// ---------------------------------------------------------------------------

/// Monotonic counter for ID uniqueness across calls in the same nanosecond.
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a locally-unique 16-byte instrument identifier.
///
/// Uses process ID + monotonic timestamp + atomic counter. This is
/// unique enough for local IPC — not a cryptographic UUID.
#[must_use]
pub fn generate_instrument_id() -> [u8; 16] {
    let mut id = [0u8; 16];
    let pid = std::process::id();
    id[0..4].copy_from_slice(&pid.to_le_bytes());
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    id[4..12].copy_from_slice(&ts.to_le_bytes());
    // Last 4 bytes: monotonic counter guarantees uniqueness even when
    // multiple IDs are generated in the same nanosecond.
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed) as u32;
    id[12..16].copy_from_slice(&counter.to_le_bytes());
    id
}

// ---------------------------------------------------------------------------
// 0x01: Register
// ---------------------------------------------------------------------------

/// Registration message sent by an instrument on first connection.
#[derive(Debug, Clone)]
pub struct RegisterMsg {
    /// Unique instrument identifier.
    pub instrument_id: [u8; 16],
    /// Null-padded instrument name (e.g. `"kazoo-808"`).
    pub name: [u8; REGISTER_NAME_LEN],
    /// Number of audio channels (1 = mono, 2 = stereo).
    pub channel_count: u8,
    /// Instrument's sample rate in Hz.
    pub sample_rate: u32,
    /// Instrument's buffer size in samples.
    pub buffer_size: u32,
}

impl RegisterMsg {
    /// Wire size: 16 + 32 + 1 + 4 + 4 = 57 bytes.
    pub const WIRE_SIZE: usize = 16 + REGISTER_NAME_LEN + 1 + 4 + 4;

    /// Create a new register message with a generated instrument ID.
    #[must_use]
    pub fn new(name: &str, channel_count: u8, sample_rate: u32, buffer_size: u32) -> Self {
        let mut name_buf = [0u8; REGISTER_NAME_LEN];
        let name_bytes = name.as_bytes();
        let copy_len = name_bytes.len().min(REGISTER_NAME_LEN);
        name_buf[..copy_len].copy_from_slice(&name_bytes[..copy_len]);

        Self {
            instrument_id: generate_instrument_id(),
            name: name_buf,
            channel_count,
            sample_rate,
            buffer_size,
        }
    }

    /// Extract the instrument name as a UTF-8 string (stops at first null).
    #[must_use]
    pub fn name_str(&self) -> &str {
        let end = self
            .name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(REGISTER_NAME_LEN);
        std::str::from_utf8(&self.name[..end]).unwrap_or("")
    }

    /// Encode into a byte buffer. Returns the number of bytes written.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[0..16].copy_from_slice(&self.instrument_id);
        buf[16..16 + REGISTER_NAME_LEN].copy_from_slice(&self.name);
        let off = 16 + REGISTER_NAME_LEN;
        buf[off] = self.channel_count;
        buf[off + 1..off + 5].copy_from_slice(&self.sample_rate.to_le_bytes());
        buf[off + 5..off + 9].copy_from_slice(&self.buffer_size.to_le_bytes());
        Self::WIRE_SIZE
    }

    /// Decode from a byte buffer.
    #[must_use]
    pub fn decode(buf: &[u8]) -> Self {
        let mut instrument_id = [0u8; 16];
        instrument_id.copy_from_slice(&buf[0..16]);
        let mut name = [0u8; REGISTER_NAME_LEN];
        name.copy_from_slice(&buf[16..16 + REGISTER_NAME_LEN]);
        let off = 16 + REGISTER_NAME_LEN;
        Self {
            instrument_id,
            name,
            channel_count: buf[off],
            sample_rate: u32::from_le_bytes([
                buf[off + 1],
                buf[off + 2],
                buf[off + 3],
                buf[off + 4],
            ]),
            buffer_size: u32::from_le_bytes([
                buf[off + 5],
                buf[off + 6],
                buf[off + 7],
                buf[off + 8],
            ]),
        }
    }
}

// ---------------------------------------------------------------------------
// 0x02: Registered
// ---------------------------------------------------------------------------

/// Registration confirmation sent by the hub.
#[derive(Debug, Clone, Copy)]
pub struct RegisteredMsg {
    /// Assigned mixer channel strip index.
    pub strip_index: u8,
    /// Hub's authoritative sample rate.
    pub hub_sample_rate: u32,
    /// Hub's authoritative buffer size.
    pub hub_buffer_size: u32,
    /// Current transport state (see `TRANSPORT_*` constants).
    pub transport_state: u8,
    /// Current tempo in BPM.
    pub bpm: f32,
    /// Current transport position in samples.
    pub position: u64,
}

impl RegisteredMsg {
    /// Wire size: 1 + 4 + 4 + 1 + 4 + 8 = 22 bytes.
    pub const WIRE_SIZE: usize = 1 + 4 + 4 + 1 + 4 + 8;

    /// Encode into a byte buffer. Returns the number of bytes written.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[0] = self.strip_index;
        buf[1..5].copy_from_slice(&self.hub_sample_rate.to_le_bytes());
        buf[5..9].copy_from_slice(&self.hub_buffer_size.to_le_bytes());
        buf[9] = self.transport_state;
        buf[10..14].copy_from_slice(&self.bpm.to_le_bytes());
        buf[14..22].copy_from_slice(&self.position.to_le_bytes());
        Self::WIRE_SIZE
    }

    /// Decode from a byte buffer.
    #[must_use]
    pub fn decode(buf: &[u8]) -> Self {
        Self {
            strip_index: buf[0],
            hub_sample_rate: u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]),
            hub_buffer_size: u32::from_le_bytes([buf[5], buf[6], buf[7], buf[8]]),
            transport_state: buf[9],
            bpm: f32::from_le_bytes([buf[10], buf[11], buf[12], buf[13]]),
            position: u64::from_le_bytes([
                buf[14], buf[15], buf[16], buf[17], buf[18], buf[19], buf[20], buf[21],
            ]),
        }
    }
}

// ---------------------------------------------------------------------------
// 0x20: Transport Sync
// ---------------------------------------------------------------------------

/// Transport state broadcast from hub to instruments.
#[derive(Debug, Clone, Copy)]
pub struct TransportSyncMsg {
    /// Transport state (see `TRANSPORT_*` constants).
    pub state: u8,
    /// Current tempo in BPM.
    pub bpm: f32,
    /// Current position in samples.
    pub position: u64,
    /// Monotonic timestamp in nanoseconds for drift correction.
    pub timestamp: u64,
}

impl TransportSyncMsg {
    /// Wire size: 1 + 4 + 8 + 8 = 21 bytes.
    pub const WIRE_SIZE: usize = 1 + 4 + 8 + 8;

    /// Encode into a byte buffer. Returns the number of bytes written.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[0] = self.state;
        buf[1..5].copy_from_slice(&self.bpm.to_le_bytes());
        buf[5..13].copy_from_slice(&self.position.to_le_bytes());
        buf[13..21].copy_from_slice(&self.timestamp.to_le_bytes());
        Self::WIRE_SIZE
    }

    /// Decode from a byte buffer.
    #[must_use]
    pub fn decode(buf: &[u8]) -> Self {
        Self {
            state: buf[0],
            bpm: f32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]),
            position: u64::from_le_bytes([
                buf[5], buf[6], buf[7], buf[8], buf[9], buf[10], buf[11], buf[12],
            ]),
            timestamp: u64::from_le_bytes([
                buf[13], buf[14], buf[15], buf[16], buf[17], buf[18], buf[19], buf[20],
            ]),
        }
    }
}

// ---------------------------------------------------------------------------
// 0x21: Transport Request
// ---------------------------------------------------------------------------

/// Transport change request from instrument to hub.
#[derive(Debug, Clone, Copy)]
pub struct TransportRequestMsg {
    /// Requested transport state.
    pub requested_state: u8,
    /// Whether a BPM change is also requested (0 = no, 1 = yes).
    pub has_bpm: u8,
    /// Requested BPM (only meaningful when `has_bpm == 1`).
    pub requested_bpm: f32,
}

impl TransportRequestMsg {
    /// Wire size: 1 + 1 + 4 = 6 bytes.
    pub const WIRE_SIZE: usize = 1 + 1 + 4;

    /// Encode into a byte buffer. Returns the number of bytes written.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[0] = self.requested_state;
        buf[1] = self.has_bpm;
        buf[2..6].copy_from_slice(&self.requested_bpm.to_le_bytes());
        Self::WIRE_SIZE
    }

    /// Decode from a byte buffer.
    #[must_use]
    pub fn decode(buf: &[u8]) -> Self {
        Self {
            requested_state: buf[0],
            has_bpm: buf[1],
            requested_bpm: f32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]),
        }
    }
}

// ---------------------------------------------------------------------------
// 0x30: Note Event
// ---------------------------------------------------------------------------

/// MIDI-style note event routed between instruments.
#[derive(Debug, Clone, Copy)]
pub struct NoteEventMsg {
    /// Source instrument UUID.
    pub source: [u8; 16],
    /// Target instrument UUID (all zeros = broadcast).
    pub target: [u8; 16],
    /// Event type (see `NOTE_*` constants).
    pub event_type: u8,
    /// MIDI channel (0-15).
    pub channel: u8,
    /// MIDI note number (or CC number for CC events).
    pub note: u8,
    /// Velocity (or CC value for CC events).
    pub velocity: u8,
}

impl NoteEventMsg {
    /// Wire size: 16 + 16 + 1 + 1 + 1 + 1 = 36 bytes.
    pub const WIRE_SIZE: usize = 16 + 16 + 1 + 1 + 1 + 1;

    /// Encode into a byte buffer. Returns the number of bytes written.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[0..16].copy_from_slice(&self.source);
        buf[16..32].copy_from_slice(&self.target);
        buf[32] = self.event_type;
        buf[33] = self.channel;
        buf[34] = self.note;
        buf[35] = self.velocity;
        Self::WIRE_SIZE
    }

    /// Decode from a byte buffer.
    #[must_use]
    pub fn decode(buf: &[u8]) -> Self {
        let mut source = [0u8; 16];
        source.copy_from_slice(&buf[0..16]);
        let mut target = [0u8; 16];
        target.copy_from_slice(&buf[16..32]);
        Self {
            source,
            target,
            event_type: buf[32],
            channel: buf[33],
            note: buf[34],
            velocity: buf[35],
        }
    }
}

// ---------------------------------------------------------------------------
// 0x40: Parameter Change
// ---------------------------------------------------------------------------

/// Mixer parameter change.
#[derive(Debug, Clone, Copy)]
pub struct ParameterChangeMsg {
    /// Target mixer strip index.
    pub strip_index: u8,
    /// Parameter identifier (see `PARAM_*` constants).
    pub param: u8,
    /// New parameter value.
    pub value: f32,
}

impl ParameterChangeMsg {
    /// Wire size: 1 + 1 + 4 = 6 bytes.
    pub const WIRE_SIZE: usize = 1 + 1 + 4;

    /// Encode into a byte buffer. Returns the number of bytes written.
    pub fn encode(&self, buf: &mut [u8]) -> usize {
        buf[0] = self.strip_index;
        buf[1] = self.param;
        buf[2..6].copy_from_slice(&self.value.to_le_bytes());
        Self::WIRE_SIZE
    }

    /// Decode from a byte buffer.
    #[must_use]
    pub fn decode(buf: &[u8]) -> Self {
        Self {
            strip_index: buf[0],
            param: buf[1],
            value: f32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal transport sync notification
// ---------------------------------------------------------------------------

/// Transport state notification from the output callback to the IPC thread.
///
/// This is NOT a wire type — it is the internal representation the hub uses
/// to pass transport state from the real-time audio callback to the IPC
/// server thread, which then converts it to [`TransportSyncMsg`] for the
/// wire.
#[derive(Debug, Clone, Copy)]
pub struct IpcTransportNotify {
    /// Transport state (see `TRANSPORT_*` constants).
    pub state: u8,
    /// Tempo in BPM.
    pub bpm: f32,
    /// Position in samples.
    pub position_samples: u64,
    /// Monotonic timestamp in nanoseconds (from engine start).
    pub timestamp_nanos: u64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_msg_roundtrip() {
        let msg = RegisterMsg::new("kazoo-808", 2, 44_100, 128);
        let mut buf = vec![0u8; RegisterMsg::WIRE_SIZE];
        let len = msg.encode(&mut buf);
        assert_eq!(len, RegisterMsg::WIRE_SIZE);

        let decoded = RegisterMsg::decode(&buf);
        assert_eq!(decoded.instrument_id, msg.instrument_id);
        assert_eq!(decoded.name_str(), "kazoo-808");
        assert_eq!(decoded.channel_count, 2);
        assert_eq!(decoded.sample_rate, 44_100);
        assert_eq!(decoded.buffer_size, 128);
    }

    #[test]
    fn register_msg_long_name_truncated() {
        let long_name = "a]".repeat(20); // 40 chars, longer than REGISTER_NAME_LEN
        let msg = RegisterMsg::new(&long_name, 1, 48_000, 256);
        assert_eq!(msg.name_str().len(), REGISTER_NAME_LEN);
    }

    #[test]
    fn registered_msg_roundtrip() {
        let msg = RegisteredMsg {
            strip_index: 3,
            hub_sample_rate: 48_000,
            hub_buffer_size: 256,
            transport_state: TRANSPORT_PLAYING,
            bpm: 140.0,
            position: 88_200,
        };
        let mut buf = vec![0u8; RegisteredMsg::WIRE_SIZE];
        let len = msg.encode(&mut buf);
        assert_eq!(len, RegisteredMsg::WIRE_SIZE);

        let decoded = RegisteredMsg::decode(&buf);
        assert_eq!(decoded.strip_index, 3);
        assert_eq!(decoded.hub_sample_rate, 48_000);
        assert_eq!(decoded.hub_buffer_size, 256);
        assert_eq!(decoded.transport_state, TRANSPORT_PLAYING);
        assert!((decoded.bpm - 140.0).abs() < f32::EPSILON);
        assert_eq!(decoded.position, 88_200);
    }

    #[test]
    fn transport_sync_msg_roundtrip() {
        let msg = TransportSyncMsg {
            state: TRANSPORT_RECORDING,
            bpm: 120.5,
            position: 1_000_000,
            timestamp: 5_000_000_000,
        };
        let mut buf = vec![0u8; TransportSyncMsg::WIRE_SIZE];
        let len = msg.encode(&mut buf);
        assert_eq!(len, TransportSyncMsg::WIRE_SIZE);

        let decoded = TransportSyncMsg::decode(&buf);
        assert_eq!(decoded.state, TRANSPORT_RECORDING);
        assert!((decoded.bpm - 120.5).abs() < f32::EPSILON);
        assert_eq!(decoded.position, 1_000_000);
        assert_eq!(decoded.timestamp, 5_000_000_000);
    }

    #[test]
    fn transport_request_msg_roundtrip() {
        let msg = TransportRequestMsg {
            requested_state: TRANSPORT_PLAYING,
            has_bpm: 1,
            requested_bpm: 135.0,
        };
        let mut buf = vec![0u8; TransportRequestMsg::WIRE_SIZE];
        msg.encode(&mut buf);

        let decoded = TransportRequestMsg::decode(&buf);
        assert_eq!(decoded.requested_state, TRANSPORT_PLAYING);
        assert_eq!(decoded.has_bpm, 1);
        assert!((decoded.requested_bpm - 135.0).abs() < f32::EPSILON);
    }

    #[test]
    fn note_event_msg_roundtrip() {
        let msg = NoteEventMsg {
            source: [1; 16],
            target: [2; 16],
            event_type: NOTE_ON,
            channel: 0,
            note: 60,
            velocity: 100,
        };
        let mut buf = vec![0u8; NoteEventMsg::WIRE_SIZE];
        msg.encode(&mut buf);

        let decoded = NoteEventMsg::decode(&buf);
        assert_eq!(decoded.source, [1; 16]);
        assert_eq!(decoded.target, [2; 16]);
        assert_eq!(decoded.event_type, NOTE_ON);
        assert_eq!(decoded.note, 60);
        assert_eq!(decoded.velocity, 100);
    }

    #[test]
    fn parameter_change_msg_roundtrip() {
        let msg = ParameterChangeMsg {
            strip_index: 5,
            param: PARAM_VOLUME,
            value: -6.0,
        };
        let mut buf = vec![0u8; ParameterChangeMsg::WIRE_SIZE];
        msg.encode(&mut buf);

        let decoded = ParameterChangeMsg::decode(&buf);
        assert_eq!(decoded.strip_index, 5);
        assert_eq!(decoded.param, PARAM_VOLUME);
        assert!((decoded.value - (-6.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn instrument_id_is_unique() {
        let id1 = generate_instrument_id();
        let id2 = generate_instrument_id();
        // IDs should differ (same PID, but different timestamp/address).
        assert_ne!(id1, id2);
    }
}
