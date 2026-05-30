//! IPC client: instrument-side connection to the hub.
//!
//! [`HubIpcClient`] manages a single Unix domain socket connection to
//! the kazoo-tui hub. It handles registration, audio block sending,
//! and transport sync receiving — all with pre-allocated buffers and
//! zero allocations on the audio hot path.

use std::io;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use super::protocol::{self, FrameBuffer};
use super::types::{
    self, MSG_AUDIO, MSG_NOTE_EVENT, MSG_PARAMETER_CHANGE, MSG_REGISTER, MSG_REGISTERED,
    MSG_SHUTDOWN, MSG_TRANSPORT_REQUEST, MSG_TRANSPORT_SYNC, NoteEventMsg, ParameterChangeMsg,
    RegisterMsg, RegisteredMsg, TransportRequestMsg, TransportSyncMsg,
};

/// Registration timeout: how long the client waits for the hub to respond
/// to a Register message before giving up.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Received message enum
// ---------------------------------------------------------------------------

/// A message received from the hub.
#[derive(Debug)]
pub enum HubMessage {
    /// Transport state update.
    TransportSync(TransportSyncMsg),
    /// Note event routed through the hub.
    NoteEvent(NoteEventMsg),
    /// Mixer parameter change from the hub.
    ParameterChange(ParameterChangeMsg),
    /// Hub requested shutdown.
    Shutdown,
}

// ---------------------------------------------------------------------------
// HubIpcClient
// ---------------------------------------------------------------------------

/// Client connection from an instrument to the kazoo-tui hub.
///
/// Owns a Unix domain socket and pre-allocated send/receive buffers.
/// The audio send path (`send_audio`) does zero allocations.
pub struct HubIpcClient {
    stream: UnixStream,
    write_buf: FrameBuffer,
    read_buf: FrameBuffer,
    seq: u32,
    instrument_id: [u8; 16],
    strip_index: u8,
    channel_count: u8,
    hub_sample_rate: u32,
    hub_buffer_size: u32,
}

// UnixStream is !Debug on some platforms.
impl std::fmt::Debug for HubIpcClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubIpcClient")
            .field("strip_index", &self.strip_index)
            .field("channel_count", &self.channel_count)
            .field("hub_sample_rate", &self.hub_sample_rate)
            .field("hub_buffer_size", &self.hub_buffer_size)
            .finish_non_exhaustive()
    }
}

impl HubIpcClient {
    /// Connect to the hub, perform the registration handshake, and return
    /// a ready-to-use client.
    ///
    /// Discovers the hub via [`super::discovery::discover_hub`]. If the hub
    /// is not running, returns an error.
    ///
    /// # Arguments
    ///
    /// * `name` — Instrument name (e.g. `"kazoo-808"`), max 32 bytes.
    /// * `channel_count` — 1 for mono, 2 for stereo.
    /// * `sample_rate` — Instrument's sample rate (must match hub).
    /// * `buffer_size` — Instrument's buffer size in samples.
    pub fn connect(
        name: &str,
        channel_count: u8,
        sample_rate: u32,
        buffer_size: u32,
    ) -> io::Result<Self> {
        let socket_path = super::discovery::discover_hub()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "hub is not running"))?;
        Self::connect_to(&socket_path, name, channel_count, sample_rate, buffer_size)
    }

    /// Connect to the hub at the given socket path.
    ///
    /// This is the lower-level entry point used by [`connect`](Self::connect)
    /// and by tests that need to specify an explicit path.
    pub fn connect_to(
        socket_path: &std::path::Path,
        name: &str,
        channel_count: u8,
        sample_rate: u32,
        buffer_size: u32,
    ) -> io::Result<Self> {
        let stream = UnixStream::connect(socket_path)?;
        stream.set_read_timeout(Some(REGISTRATION_TIMEOUT))?;
        stream.set_write_timeout(Some(REGISTRATION_TIMEOUT))?;

        let reg = RegisterMsg::new(name, channel_count, sample_rate, buffer_size);
        let instrument_id = reg.instrument_id;

        // Send Register message.
        let mut write_buf = FrameBuffer::new();
        reg.encode(write_buf.payload_mut());
        write_buf.write_frame(MSG_REGISTER, 0, RegisterMsg::WIRE_SIZE, &mut &stream)?;

        // Read Registered response.
        let mut read_buf = FrameBuffer::new();
        let header = read_buf.read_frame(&mut &stream)?;
        if header.msg_type != MSG_REGISTERED {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected Registered (0x02), got 0x{:02X}", header.msg_type),
            ));
        }
        let resp = RegisteredMsg::decode(read_buf.payload());

        // Switch to non-blocking for audio path.
        stream.set_nonblocking(true)?;

        Ok(Self {
            stream,
            write_buf,
            read_buf,
            seq: 1,
            instrument_id,
            strip_index: resp.strip_index,
            channel_count,
            hub_sample_rate: resp.hub_sample_rate,
            hub_buffer_size: resp.hub_buffer_size,
        })
    }

    /// The instrument ID assigned during construction.
    #[must_use]
    pub const fn instrument_id(&self) -> &[u8; 16] {
        &self.instrument_id
    }

    /// The mixer strip index assigned by the hub.
    #[must_use]
    pub const fn strip_index(&self) -> u8 {
        self.strip_index
    }

    /// The hub's authoritative sample rate.
    #[must_use]
    pub const fn hub_sample_rate(&self) -> u32 {
        self.hub_sample_rate
    }

    /// The hub's authoritative buffer size.
    #[must_use]
    pub const fn hub_buffer_size(&self) -> u32 {
        self.hub_buffer_size
    }

    // -----------------------------------------------------------------------
    // Audio send (hot path — zero allocations)
    // -----------------------------------------------------------------------

    /// Send an audio block to the hub.
    ///
    /// `samples` must contain `frame_count * channel_count` interleaved
    /// f32 values. NaN/Inf values are sanitized to `0.0` before sending.
    ///
    /// This is the hot path. Zero allocations — uses pre-allocated buffers.
    pub fn send_audio(&mut self, frame_count: u32, samples: &[f32]) -> io::Result<()> {
        let payload_len =
            protocol::encode_audio_payload(frame_count, samples, self.write_buf.payload_mut());
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        match self
            .write_buf
            .write_frame(MSG_AUDIO, seq, payload_len, &mut &self.stream)
        {
            Ok(()) => Ok(()),
            // Socket buffer full — silently drop this audio frame rather
            // than disconnecting. The hub's hold-last-value will cover the
            // gap. This is the expected back-pressure behaviour under load.
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e),
        }
    }

    // -----------------------------------------------------------------------
    // Message receive (non-blocking)
    // -----------------------------------------------------------------------

    /// Try to receive a message from the hub (non-blocking).
    ///
    /// Returns `Ok(Some(msg))` if a complete message was received,
    /// `Ok(None)` if no data is available, or `Err` on connection failure.
    pub fn try_recv(&mut self) -> io::Result<Option<HubMessage>> {
        let header = match self.read_buf.try_read_frame(&mut &self.stream) {
            Ok(Some(h)) => h,
            Ok(None) => return Ok(None),
            Err(e) => return Err(e),
        };

        let payload = self.read_buf.payload();
        match header.msg_type {
            MSG_TRANSPORT_SYNC => {
                let msg = TransportSyncMsg::decode(payload);
                Ok(Some(HubMessage::TransportSync(msg)))
            }
            MSG_NOTE_EVENT => {
                let msg = NoteEventMsg::decode(payload);
                Ok(Some(HubMessage::NoteEvent(msg)))
            }
            MSG_PARAMETER_CHANGE => {
                let msg = ParameterChangeMsg::decode(payload);
                Ok(Some(HubMessage::ParameterChange(msg)))
            }
            MSG_SHUTDOWN => Ok(Some(HubMessage::Shutdown)),
            _ => {
                // Unknown message type — skip it.
                Ok(None)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Control message send
    // -----------------------------------------------------------------------

    /// Request a transport state change from the hub.
    pub fn send_transport_request(&mut self, state: u8, bpm: Option<f32>) -> io::Result<()> {
        let msg = TransportRequestMsg {
            requested_state: state,
            has_bpm: u8::from(bpm.is_some()),
            requested_bpm: bpm.unwrap_or(0.0),
        };
        msg.encode(self.write_buf.payload_mut());
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        self.write_buf.write_frame(
            MSG_TRANSPORT_REQUEST,
            seq,
            TransportRequestMsg::WIRE_SIZE,
            &mut &self.stream,
        )
    }

    /// Send a note event to another instrument (routed through the hub).
    pub fn send_note_event(&mut self, event: &types::NoteEventMsg) -> io::Result<()> {
        event.encode(self.write_buf.payload_mut());
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        self.write_buf.write_frame(
            MSG_NOTE_EVENT,
            seq,
            NoteEventMsg::WIRE_SIZE,
            &mut &self.stream,
        )
    }

    /// Send a clean shutdown notification to the hub.
    pub fn send_shutdown(&mut self) -> io::Result<()> {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        self.write_buf
            .write_frame(MSG_SHUTDOWN, seq, 0, &mut &self.stream)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_fails_when_hub_not_running() {
        // Ensure no stale PID file interferes.
        super::super::discovery::remove_pid_file();
        let result = HubIpcClient::connect("test-instrument", 2, 44_100, 128);
        assert!(result.is_err());
    }

    #[test]
    fn connect_to_nonexistent_socket_fails() {
        let path = std::path::Path::new("/tmp/kazoo-nonexistent-test.sock");
        let result = HubIpcClient::connect_to(path, "test-instrument", 2, 44_100, 128);
        assert!(result.is_err());
    }
}
