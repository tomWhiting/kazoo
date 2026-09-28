//! IPC client: instrument-side connection to the hub.
//!
//! [`HubIpcClient`] manages a single Unix domain socket connection to the
//! hub (kazoo-mix). It handles registration, sending audio and control
//! messages, and receiving transport, note and shutdown messages.
//!
//! Sends are queued whole in an [`Outbox`] and written as the socket accepts
//! them, so a full socket never leaves half a frame on the wire. The client
//! does socket I/O and is meant for a link thread, not an audio callback;
//! [`super::link`] provides the callback-safe side.

use std::io;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use super::outbox::{Outbox, OutboxError};
use super::protocol::{self, FrameBuffer};
use super::types::{
    self, DeskPaceMsg, MSG_AUDIO, MSG_DESK_PACE, MSG_NOTE_EVENT, MSG_PARAMETER_CHANGE, MSG_REFUSED,
    MSG_REGISTER, MSG_REGISTERED, MSG_SHUTDOWN, MSG_TRANSPORT_REQUEST, MSG_TRANSPORT_SYNC,
    NoteEventMsg, ParameterChangeMsg, RegisterMsg, RegisteredMsg, TransportRequestMsg,
    TransportSyncMsg,
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

/// Client connection from an instrument to the hub.
///
/// Owns a Unix domain socket, a receive buffer and an [`Outbox`] for sends.
pub struct HubIpcClient {
    stream: UnixStream,
    write_buf: FrameBuffer,
    read_buf: FrameBuffer,
    outbox: Outbox,
    instrument_id: [u8; 16],
    strip_index: u8,
    channel_count: u8,
    hub_sample_rate: u32,
    hub_buffer_size: u32,
    registration: RegisteredMsg,
    /// The desk's latest word on where it is playing this stream, until
    /// taken ([`Self::take_desk_pace`]).
    desk_pace: Option<DeskPaceMsg>,
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
    /// Finds the hub's socket via [`super::discovery::hub_socket`]. If no
    /// hub is running there, the connection is refused and an error
    /// returned.
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
        let socket_path = super::discovery::hub_socket()?;
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
        let reg = RegisterMsg::new(name, channel_count, sample_rate, buffer_size);
        Self::register_at(socket_path, &reg)
    }

    /// Connect to the hub at the given socket path with `reg` as the
    /// registration: how an instrument that renders on a timer asks the
    /// desk to pace it (see [`RegisterMsg::pace_lead_frames`]).
    pub fn register_at(socket_path: &std::path::Path, reg: &RegisterMsg) -> io::Result<Self> {
        let stream = UnixStream::connect(socket_path)?;
        stream.set_read_timeout(Some(REGISTRATION_TIMEOUT))?;
        stream.set_write_timeout(Some(REGISTRATION_TIMEOUT))?;

        let instrument_id = reg.instrument_id;
        let channel_count = reg.channel_count;

        // Send Register message.
        let mut write_buf = FrameBuffer::new();
        let len = reg.encode(write_buf.payload_mut());
        write_buf.write_frame(MSG_REGISTER, 0, len, &mut &stream)?;

        // Read Registered response.
        let mut read_buf = FrameBuffer::new();
        let header = read_buf.read_frame(&mut &stream)?;
        let len = header.payload_len as usize;
        if header.msg_type == MSG_REFUSED {
            let reason = String::from_utf8_lossy(&read_buf.payload()[..len]).into_owned();
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, reason));
        }
        if header.msg_type != MSG_REGISTERED || len < RegisteredMsg::WIRE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "expected Registered (0x02, {} bytes), got 0x{:02X} with {len} bytes",
                    RegisteredMsg::WIRE_SIZE,
                    header.msg_type
                ),
            ));
        }
        let resp = RegisteredMsg::decode(read_buf.payload());

        // Switch to non-blocking for audio path.
        stream.set_nonblocking(true)?;

        Ok(Self {
            stream,
            write_buf,
            read_buf,
            outbox: Outbox::default(),
            instrument_id,
            strip_index: resp.strip_index,
            channel_count,
            hub_sample_rate: resp.hub_sample_rate,
            hub_buffer_size: resp.hub_buffer_size,
            registration: resp,
            desk_pace: None,
        })
    }

    /// The hub's answer to the registration, including the transport state
    /// and tempo at the moment the instrument joined.
    #[must_use]
    pub const fn registration(&self) -> RegisteredMsg {
        self.registration
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
    // Sending
    // -----------------------------------------------------------------------

    /// Queue an audio block for the hub and write what the socket accepts.
    ///
    /// `stream_frame` is the block's first frame in the instrument's own
    /// stream (every frame it has rendered, counted from 0, including any it
    /// dropped): the hub uses it to place the block, and any gap before it,
    /// on the studio timeline. `samples` must contain
    /// `frame_count * channel_count` interleaved f32 values. NaN/Inf values
    /// are sanitized to `0.0` before sending.
    ///
    /// # Errors
    ///
    /// [`OutboxError::Stuck`] if the hub has stopped reading (this block was
    /// not queued), [`OutboxError::TooLarge`] for a block over the protocol
    /// limit, and [`OutboxError::Io`] if the connection failed.
    pub fn send_audio(
        &mut self,
        stream_frame: u64,
        frame_count: u32,
        samples: &[f32],
    ) -> Result<(), OutboxError> {
        let max_samples = (protocol::MAX_PAYLOAD_SIZE - protocol::AUDIO_PAYLOAD_HEADER)
            / std::mem::size_of::<f32>();
        if samples.len() > max_samples {
            return Err(OutboxError::TooLarge);
        }
        let payload_len = protocol::encode_audio_payload(
            stream_frame,
            frame_count,
            samples,
            self.write_buf.payload_mut(),
        );
        self.outbox
            .queue(MSG_AUDIO, &self.write_buf.payload()[..payload_len])?;
        self.flush()
    }

    /// Write queued frames as far as the socket accepts them now.
    ///
    /// # Errors
    ///
    /// [`OutboxError::Io`] if the connection failed.
    pub fn flush(&mut self) -> Result<(), OutboxError> {
        self.outbox.flush(&mut &self.stream)
    }

    /// Bytes queued and not yet written.
    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        self.outbox.waiting()
    }

    // -----------------------------------------------------------------------
    // Message receive (non-blocking)
    // -----------------------------------------------------------------------

    /// Try to receive a message from the hub (non-blocking).
    ///
    /// Returns `Ok(Some(msg))` if a complete message was received,
    /// `Ok(None)` if no data is available, or `Err` on connection failure.
    ///
    /// A desk pace ([`MSG_DESK_PACE`]) is not a message for the
    /// instrument: it is kept for [`Self::take_desk_pace`]. It, and any
    /// message of a type this instrument does not know, is passed over and
    /// reading goes on to the next message.
    ///
    /// # Errors
    ///
    /// The connection failed, or the hub sent a message of a known type
    /// shorter than that type is ([`io::ErrorKind::InvalidData`]).
    pub fn try_recv(&mut self) -> io::Result<Option<HubMessage>> {
        loop {
            let header = match self.read_buf.try_read_frame(&mut &self.stream) {
                Ok(Some(h)) => h,
                Ok(None) => return Ok(None),
                Err(e) => return Err(e),
            };
            let len = header.payload_len as usize;
            if header.msg_type == MSG_DESK_PACE {
                let payload = self.whole(header.msg_type, len, DeskPaceMsg::WIRE_SIZE)?;
                self.desk_pace = Some(DeskPaceMsg::decode(payload));
            } else if let Some(message) = self.decode(header.msg_type, len)? {
                return Ok(Some(message));
            }
        }
    }

    /// The `len` bytes of the message of type `msg_type` in the read
    /// buffer, which must be at least `needed`.
    fn whole(&self, msg_type: u8, len: usize, needed: usize) -> io::Result<&[u8]> {
        if len < needed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "the desk sent message 0x{msg_type:02X} with {len} bytes; it needs {needed}"
                ),
            ));
        }
        Ok(&self.read_buf.payload()[..len])
    }

    /// The desk's latest pace since the last call, if one has arrived.
    pub const fn take_desk_pace(&mut self) -> Option<DeskPaceMsg> {
        self.desk_pace.take()
    }

    /// The instrument's message in the read buffer, of type `msg_type`
    /// and `len` bytes; `None` for a type this instrument does not know.
    fn decode(&self, msg_type: u8, len: usize) -> io::Result<Option<HubMessage>> {
        Ok(match msg_type {
            MSG_TRANSPORT_SYNC => Some(HubMessage::TransportSync(TransportSyncMsg::decode(
                self.whole(msg_type, len, TransportSyncMsg::WIRE_SIZE)?,
            ))),
            MSG_NOTE_EVENT => Some(HubMessage::NoteEvent(NoteEventMsg::decode(self.whole(
                msg_type,
                len,
                NoteEventMsg::WIRE_SIZE,
            )?))),
            MSG_PARAMETER_CHANGE => Some(HubMessage::ParameterChange(ParameterChangeMsg::decode(
                self.whole(msg_type, len, ParameterChangeMsg::WIRE_SIZE)?,
            ))),
            MSG_SHUTDOWN => Some(HubMessage::Shutdown),
            // A message type this instrument does not know: skipped.
            _ => None,
        })
    }

    // -----------------------------------------------------------------------
    // Control message send
    // -----------------------------------------------------------------------

    /// Request a transport state change from the hub.
    ///
    /// # Errors
    ///
    /// As [`Self::send_audio`].
    pub fn send_transport_request(
        &mut self,
        state: u8,
        bpm: Option<f32>,
    ) -> Result<(), OutboxError> {
        let msg = TransportRequestMsg {
            requested_state: state,
            has_bpm: u8::from(bpm.is_some()),
            requested_bpm: bpm.unwrap_or(0.0),
        };
        let mut payload = [0_u8; TransportRequestMsg::WIRE_SIZE];
        msg.encode(&mut payload);
        self.outbox.queue(MSG_TRANSPORT_REQUEST, &payload)?;
        self.flush()
    }

    /// Send a note event to another instrument (routed through the hub).
    ///
    /// # Errors
    ///
    /// As [`Self::send_audio`].
    pub fn send_note_event(&mut self, event: &types::NoteEventMsg) -> Result<(), OutboxError> {
        let mut payload = [0_u8; NoteEventMsg::WIRE_SIZE];
        event.encode(&mut payload);
        self.outbox.queue(MSG_NOTE_EVENT, &payload)?;
        self.flush()
    }

    /// Tell the hub this instrument is leaving.
    ///
    /// # Errors
    ///
    /// As [`Self::send_audio`].
    pub fn send_shutdown(&mut self) -> Result<(), OutboxError> {
        self.outbox.queue(MSG_SHUTDOWN, &[])?;
        self.flush()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_to_nonexistent_socket_fails() {
        let path = std::path::Path::new("/tmp/kazoo-nonexistent-test.sock");
        let result = HubIpcClient::connect_to(path, "test-instrument", 2, 44_100, 128);
        assert!(result.is_err());
    }

    /// A client registered with a stand-in hub, and the hub's end.
    fn connected() -> (HubIpcClient, UnixStream, FrameBuffer) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("kzc-client-{}-{n}.sock", std::process::id()));
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let hub = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = FrameBuffer::new();
            buf.read_frame(&mut stream).unwrap();
            let reply = RegisteredMsg {
                strip_index: 0,
                hub_sample_rate: 48_000,
                hub_buffer_size: 256,
                transport_state: 0,
                bpm: 120.0,
                position: 0,
            };
            reply.encode(buf.payload_mut());
            buf.write_frame(MSG_REGISTERED, 0, RegisteredMsg::WIRE_SIZE, &mut stream)
                .unwrap();
            (stream, buf)
        });
        let client = HubIpcClient::connect_to(&path, "kazoo-test", 2, 48_000, 256).unwrap();
        let (stream, buf) = hub.join().unwrap();
        std::fs::remove_file(&path).unwrap();
        (client, stream, buf)
    }

    /// Read until something other than "nothing yet" comes back.
    fn next(client: &mut HubIpcClient) -> io::Result<Option<HubMessage>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match client.try_recv() {
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                other => return other,
            }
        }
    }

    #[test]
    fn a_pace_is_kept_and_the_next_message_still_arrives() {
        let (mut client, mut hub, mut buf) = connected();
        let pace = DeskPaceMsg {
            playing_stream: 4_800,
            lead_frames: 2_576,
        };
        pace.encode(buf.payload_mut());
        buf.write_frame(MSG_DESK_PACE, 0, DeskPaceMsg::WIRE_SIZE, &mut hub)
            .unwrap();
        buf.write_frame(0x7E, 1, 0, &mut hub).unwrap();
        buf.write_frame(MSG_SHUTDOWN, 2, 0, &mut hub).unwrap();
        assert!(matches!(next(&mut client), Ok(Some(HubMessage::Shutdown))));
        assert_eq!(client.take_desk_pace(), Some(pace));
        assert_eq!(client.take_desk_pace(), None);
    }

    #[test]
    fn a_short_message_of_a_known_type_is_an_error_not_old_bytes() {
        for (msg_type, len) in [
            (MSG_TRANSPORT_SYNC, TransportSyncMsg::WIRE_SIZE - 1),
            (MSG_NOTE_EVENT, 3),
            (MSG_PARAMETER_CHANGE, 0),
            (MSG_DESK_PACE, DeskPaceMsg::WIRE_SIZE - 4),
        ] {
            let (mut client, mut hub, mut buf) = connected();
            buf.write_frame(msg_type, 0, len, &mut hub).unwrap();
            let err = next(&mut client).expect_err("a short message");
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "0x{msg_type:02X}");
            assert!(err.to_string().contains("needs"), "{err}");
        }
    }
}
