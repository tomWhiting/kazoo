//! IPC client: instrument-side connection to the hub.
//!
//! [`HubIpcClient`] manages a single Unix or TCP connection to the
//! hub (kazoo-mix). It handles registration, sending audio and control
//! messages, and receiving transport, note and shutdown messages.
//!
//! Sends are queued whole in an [`Outbox`] and written as the socket accepts
//! them, so a full socket never leaves half a frame on the wire. The client
//! does socket I/O and is meant for a link thread, not an audio callback;
//! [`super::link`] provides the callback-safe side.

use std::io::{self, Write};
use std::time::Duration;

use super::link::HubAddress;
use super::outbox::{Outbox, OutboxError};
use super::protocol::{self, FrameBuffer};
use super::stream::HubStream;
use super::types::{
    self, DeskHealthMsg, DeskPaceMsg, MSG_AUDIO, MSG_DESK_HEALTH, MSG_DESK_PACE, MSG_NOTE_EVENT,
    MSG_PARAMETER_CHANGE, MSG_REFUSED, MSG_REGISTER, MSG_REGISTERED, MSG_SHUTDOWN,
    MSG_TRANSPORT_REQUEST, MSG_TRANSPORT_SYNC, NoteEventMsg, ParameterChangeMsg, RegisterMsg,
    RegisteredMsg, TransportRequestMsg, TransportSyncMsg,
};
use super::types::{FeaturesMsg, MSG_DESK_FEATURES, MSG_VOICE, MSG_VOICE_FOLLOW, VoiceFollowMsg};

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
    /// A named stream was seated or refused.
    StreamReady {
        stream: types::StreamId,
        strip: Option<u8>,
    },
    /// Hub requested shutdown.
    Shutdown,
}

/// A socket read; voice bytes stay borrowed until the next read.
#[derive(Debug)]
pub enum HubRead<'a> {
    /// A note, transport or other ordinary instrument message.
    Message(HubMessage),
    /// A voice batch for decoding directly into its dedicated ring.
    Voice(&'a [u8]),
    /// New capabilities are available through `take_desk_features`.
    DeskFeatures,
}

// ---------------------------------------------------------------------------
// HubIpcClient
// ---------------------------------------------------------------------------

/// Client connection from an instrument to the hub.
///
/// Owns a desk stream, a receive buffer and an [`Outbox`] for sends.
pub struct HubIpcClient {
    stream: HubStream,
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
    /// The desk's latest word on how it is taking this stream, until taken
    /// ([`Self::take_desk_health`]).
    desk_health: Option<DeskHealthMsg>,
    last_stream_refusal: Option<(types::StreamId, String)>,
    /// Whether the desk answered the registration with a strip per stream:
    /// a desk older than named streams answers without, and must never be
    /// sent 0x11, 0x12 or 0x13.
    desk_takes_streams: bool,
    desk_features: u32,
    new_desk_features: bool,
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
    /// Uses [`HubAddress::Discover`]: `KAZOO_DESK` selects a TCP address or
    /// socket path before local discovery. An invalid override is refused.
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
        let reg = RegisterMsg::new(name, channel_count, sample_rate, buffer_size);
        Self::register_with_timeout(&HubAddress::Discover, &reg, REGISTRATION_TIMEOUT)
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
        Self::register_with_timeout(
            &HubAddress::Socket(socket_path.to_path_buf()),
            reg,
            REGISTRATION_TIMEOUT,
        )
    }

    pub(crate) fn register_with_timeout(
        address: &HubAddress,
        reg: &RegisterMsg,
        timeout: Duration,
    ) -> io::Result<Self> {
        if !reg.streams.is_empty() {
            reg.check_streams()
                .map_err(|why| io::Error::new(io::ErrorKind::InvalidInput, why))?;
        }
        let mut stream = HubStream::connect(address, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;

        let instrument_id = reg.instrument_id;
        let channel_count = reg
            .streams
            .first()
            .map_or(reg.channel_count, |spec| spec.channels);

        // Send Register message.
        let mut write_buf = FrameBuffer::new();
        let len = reg.encode(write_buf.payload_mut());
        write_buf.write_frame(MSG_REGISTER, 0, len, &mut stream)?;

        // Read Registered response.
        let mut read_buf = FrameBuffer::new();
        let header = read_buf.read_frame(&mut stream)?;
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
        let payload = &read_buf.payload()[..len];
        // A desk that knows streams answers one strip per stream registered
        // (a register without a list is one stream), and stream 0's strip
        // is the one byte 0 names. An old desk answers the 22 bytes alone.
        let expected = reg.streams.len().max(1);
        if len != RegisteredMsg::WIRE_SIZE
            && (len != 23 + expected
                || usize::from(payload[22]) != expected
                || payload[23] != payload[0])
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("the desk answered {len} bytes of strips for {expected} streams"),
            ));
        }
        let resp = RegisteredMsg::decode(payload);

        // Switch to non-blocking for audio path.
        stream.set_nonblocking(true)?;

        Ok(Self {
            stream,
            write_buf,
            read_buf,
            outbox: Outbox::prepared(),
            instrument_id,
            strip_index: resp.strip_index,
            channel_count,
            hub_sample_rate: resp.hub_sample_rate,
            hub_buffer_size: resp.hub_buffer_size,
            registration: resp,
            desk_pace: None,
            desk_health: None,
            last_stream_refusal: None,
            desk_takes_streams: len != RegisteredMsg::WIRE_SIZE,
            desk_features: 0,
            new_desk_features: false,
        })
    }

    /// The hub's answer to the registration, including the transport state
    /// and tempo at the moment the instrument joined.
    #[must_use]
    pub const fn registration(&self) -> &RegisteredMsg {
        &self.registration
    }

    /// Whether the desk takes named streams: it answered the registration
    /// with a strip for each. One that did not would drop the instrument
    /// for a named stream's audio, an add or a remove.
    #[must_use]
    pub const fn desk_takes_streams(&self) -> bool {
        self.desk_takes_streams
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

    /// Write bytes borrowed from the link's ring without copying them.
    /// The caller preserves frame boundaries across partial writes.
    pub fn write_audio_bytes(&mut self, bytes: &[u8]) -> io::Result<usize> {
        (&self.stream).write(bytes)
    }

    /// Bytes queued and not yet written.
    #[must_use]
    pub fn pending_bytes(&self) -> usize {
        self.outbox.waiting()
    }

    pub(crate) const fn written_bytes(&self) -> u64 {
        self.outbox.written_bytes()
    }

    // -----------------------------------------------------------------------
    // Message receive (non-blocking)
    // -----------------------------------------------------------------------

    /// Try to receive a message from the hub (non-blocking).
    ///
    /// Returns `Ok(Some(msg))` if a complete message was received,
    /// `Ok(None)` if no data is available, or `Err` on connection failure.
    ///
    /// A desk pace ([`MSG_DESK_PACE`]) or desk health
    /// ([`MSG_DESK_HEALTH`]) is not a message for the instrument: it is
    /// kept for [`Self::take_desk_pace`] or [`Self::take_desk_health`].
    /// They, and any message of a type this instrument does not know, are
    /// passed over and reading goes on to the next message.
    /// Voice batches are validated and skipped here; voice readers use
    /// [`Self::try_read`]. Features are kept by [`Self::take_desk_features`].
    ///
    /// # Errors
    ///
    /// The connection failed, or the hub sent a message of a known type
    /// shorter than that type is ([`io::ErrorKind::InvalidData`]).
    pub fn try_recv(&mut self) -> io::Result<Option<HubMessage>> {
        loop {
            match self.try_read()? {
                Some(HubRead::Message(message)) => return Ok(Some(message)),
                Some(HubRead::Voice(payload)) => {
                    // Callers interested in voice use try_read; ordinary message
                    // readers still reject malformed batches instead of hiding them.
                    super::voice::decode_batch(payload, |_| {})
                        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
                }
                Some(HubRead::DeskFeatures) => {}
                None => return Ok(None),
            }
        }
    }

    /// Read ordinary messages or borrow a voice payload, without putting voice
    /// frames into [`HubMessage`]. Capability announcements are kept separately.
    pub fn try_read(&mut self) -> io::Result<Option<HubRead<'_>>> {
        loop {
            let header = match self.read_buf.try_read_frame(&mut &self.stream) {
                Ok(Some(h)) => h,
                Ok(None) => return Ok(None),
                Err(e) => return Err(e),
            };
            let len = header.payload_len as usize;
            if header.msg_type == MSG_DESK_FEATURES {
                let payload = self.whole(header.msg_type, len, FeaturesMsg::WIRE_SIZE)?;
                if len != FeaturesMsg::WIRE_SIZE {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "desk features need exactly four bytes",
                    ));
                }
                self.desk_features = FeaturesMsg::decode(payload).features;
                self.new_desk_features = true;
                return Ok(Some(HubRead::DeskFeatures));
            } else if header.msg_type == MSG_VOICE {
                return Ok(Some(HubRead::Voice(&self.read_buf.payload()[..len])));
            } else if header.msg_type == MSG_DESK_PACE {
                let payload = self.whole(header.msg_type, len, DeskPaceMsg::WIRE_SIZE)?;
                self.desk_pace = Some(DeskPaceMsg::decode(payload));
            } else if header.msg_type == MSG_DESK_HEALTH {
                let payload = self.whole(header.msg_type, len, DeskHealthMsg::WIRE_SIZE)?;
                self.desk_health = Some(DeskHealthMsg::decode(payload));
            } else if let Some(message) = self.decode(header.msg_type, len)? {
                return Ok(Some(HubRead::Message(message)));
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

    /// The desk's latest health report since the last call, if one has
    /// arrived.
    pub const fn take_desk_health(&mut self) -> Option<DeskHealthMsg> {
        self.desk_health.take()
    }

    /// The latest capability announcement since the last call.
    pub const fn take_desk_features(&mut self) -> Option<u32> {
        if self.new_desk_features {
            self.new_desk_features = false;
            Some(self.desk_features)
        } else {
            None
        }
    }

    /// The instrument's message in the read buffer, of type `msg_type`
    /// and `len` bytes; `None` for a type this instrument does not know.
    fn decode(&mut self, msg_type: u8, len: usize) -> io::Result<Option<HubMessage>> {
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
            types::MSG_STREAM_READY => {
                let payload = self.whole(msg_type, len, types::StreamReadyMsg::WIRE_SIZE)?;
                if len > types::StreamReadyMsg::WIRE_SIZE + types::STREAM_REFUSAL_LEN {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "stream refusal reason exceeds 256 bytes",
                    ));
                }
                std::str::from_utf8(&payload[3..])
                    .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
                let (ready, reason) = types::StreamReadyMsg::decode(payload);
                if usize::from(ready.stream) >= types::MAX_STREAMS {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "stream id exceeds link capacity",
                    ));
                }
                let refusal =
                    (ready.strip == types::NO_STRIP).then(|| (ready.stream, reason.to_string()));
                if refusal.is_some() {
                    self.last_stream_refusal = refusal;
                }
                Some(HubMessage::StreamReady {
                    stream: ready.stream,
                    strip: (ready.strip != types::NO_STRIP).then_some(ready.strip),
                })
            }
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

    /// Queue an encoded voice batch through the control outbox.
    pub fn send_voice_batch(&mut self, payload: &[u8]) -> Result<(), OutboxError> {
        self.outbox.queue(MSG_VOICE, payload)?;
        self.flush()
    }

    /// Tell a desk carrying voice whether this instrument follows it.
    pub fn send_voice_follow(&mut self, on: bool) -> Result<(), OutboxError> {
        let mut payload = [0; VoiceFollowMsg::WIRE_SIZE];
        VoiceFollowMsg { on }.encode(&mut payload);
        self.outbox.queue(MSG_VOICE_FOLLOW, &payload)?;
        self.flush()
    }

    /// Take the last stream refusal without disconnecting the instrument.
    pub const fn take_stream_refusal(&mut self) -> Option<(types::StreamId, String)> {
        self.last_stream_refusal.take()
    }

    /// Announce a stream through the control outbox.
    pub fn send_stream_add(&mut self, msg: &types::StreamAddMsg) -> Result<(), OutboxError> {
        let mut payload = [0; types::StreamAddMsg::WIRE_SIZE];
        msg.encode(&mut payload);
        self.outbox.queue(types::MSG_STREAM_ADD, &payload)?;
        self.flush()
    }

    /// Withdraw a stream through the control outbox.
    pub fn send_stream_remove(&mut self, stream: types::StreamId) -> Result<(), OutboxError> {
        let mut payload = [0; types::StreamRemoveMsg::WIRE_SIZE];
        types::StreamRemoveMsg { stream }.encode(&mut payload);
        self.outbox.queue(types::MSG_STREAM_REMOVE, &payload)?;
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
    use std::os::unix::net::UnixStream;

    #[test]
    fn connect_to_nonexistent_socket_fails() {
        let path = std::path::Path::new("/tmp/kazoo-nonexistent-test.sock");
        let result = HubIpcClient::connect_to(path, "test-instrument", 2, 44_100, 128);
        assert!(result.is_err());
    }

    /// A client registered with a stand-in hub, and the hub's end.
    pub(super) fn connected() -> (HubIpcClient, UnixStream, FrameBuffer) {
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
                strips: Vec::new(),
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
    fn desk_health_is_kept_and_the_next_message_still_arrives() {
        let (mut client, mut hub, mut buf) = connected();
        let health = DeskHealthMsg {
            placed_afresh: 3,
            dropped: 1,
        };
        health.encode(buf.payload_mut());
        buf.write_frame(MSG_DESK_HEALTH, 0, DeskHealthMsg::WIRE_SIZE, &mut hub)
            .unwrap();
        buf.write_frame(MSG_SHUTDOWN, 1, 0, &mut hub).unwrap();
        assert!(matches!(next(&mut client), Ok(Some(HubMessage::Shutdown))));
        assert_eq!(client.take_desk_health(), Some(health));
        assert_eq!(client.take_desk_health(), None);
    }

    #[test]
    fn a_short_message_of_a_known_type_is_an_error_not_old_bytes() {
        for (msg_type, len) in [
            (MSG_TRANSPORT_SYNC, TransportSyncMsg::WIRE_SIZE - 1),
            (MSG_NOTE_EVENT, 3),
            (MSG_PARAMETER_CHANGE, 0),
            (MSG_DESK_PACE, DeskPaceMsg::WIRE_SIZE - 4),
            (MSG_DESK_HEALTH, DeskHealthMsg::WIRE_SIZE - 1),
        ] {
            let (mut client, mut hub, mut buf) = connected();
            buf.write_frame(msg_type, 0, len, &mut hub).unwrap();
            let err = next(&mut client).expect_err("a short message");
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "0x{msg_type:02X}");
            assert!(err.to_string().contains("needs"), "{err}");
        }
    }
}

#[cfg(test)]
mod streams_tests;
#[cfg(test)]
mod voice_tests;
