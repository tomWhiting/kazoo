//! One connected instrument: its socket, its ring into the desk, and the
//! clock that places its audio on the studio timeline.

use std::io;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use kazoo_core::audio_transport::{AudioBlock, AudioBlockProducer, AudioRingPushError};
use kazoo_core::ipc::protocol::{
    AUDIO_PAYLOAD_HEADER, FrameBuffer, FrameHeader, decode_audio_frame_count, decode_audio_samples,
    decode_audio_stream_frame,
};
use kazoo_core::ipc::types::{
    DeskHealthMsg, DeskPaceMsg, MSG_AUDIO, MSG_DESK_HEALTH, MSG_DESK_PACE, MSG_NOTE_EVENT,
    MSG_SHUTDOWN, MSG_TRANSPORT_REQUEST, NoteEventMsg, TransportRequestMsg,
};
use kazoo_core::protocol::{AudioBlockHeader, BlockFlags};

use kazoo_core::ipc::outbox::{Outbox, OutboxError};

use crate::studio_clock::{DeskClock, StudioStamp};

/// Largest audio message accepted, in frames: the instruments' own ceiling.
pub const MAX_MESSAGE_FRAMES: usize = 4096;

/// How often a paced instrument is told where the desk is playing its
/// stream: often enough that its own clock never carries it far between
/// words.
pub const PACE_INTERVAL: Duration = Duration::from_millis(5);

/// Least time between two reports to an instrument of how the desk has
/// been taking its audio: a stream placed afresh again and again is told
/// ten times a second, not once a block, so what an instrument hears last
/// before it leaves is at most a tenth of a second old. (No final report is
/// sent as it leaves: up to that tenth of a second goes untold.)
pub const HEALTH_INTERVAL: Duration = Duration::from_millis(100);

/// Something an instrument sent that the hub must act on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Inbound {
    /// A block of audio went into the desk.
    Audio {
        /// The block was re-placed on the studio clock.
        resynced: bool,
    },
    /// A block of audio was lost because the desk's ring was full.
    AudioDropped,
    /// A note for other instruments.
    Note(NoteMessage),
    /// A transport change request.
    Transport {
        /// Requested play state (see `TRANSPORT_*`).
        state: u8,
        /// Requested tempo, if any.
        bpm: Option<f32>,
    },
    /// The instrument is leaving.
    Goodbye,
}

/// A note event, kept as plain data for routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteMessage {
    /// Instrument the note is for; all zeros means every instrument.
    pub target: [u8; 16],
    /// The encoded message, forwarded as is.
    pub payload: [u8; NoteEventMsg::WIRE_SIZE],
}

/// Why an instrument's message was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// The payload is shorter than its message type needs.
    Truncated {
        /// Message type.
        msg_type: u8,
        /// Bytes needed.
        needed: usize,
        /// Bytes sent.
        got: usize,
    },
    /// An audio block larger than [`MAX_MESSAGE_FRAMES`].
    BlockTooLarge {
        /// Frames claimed.
        frames: usize,
    },
    /// A message type the hub does not know.
    Unknown {
        /// Message type.
        msg_type: u8,
    },
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated {
                msg_type,
                needed,
                got,
            } => write!(
                f,
                "message 0x{msg_type:02X} carried {got} bytes, needs {needed}"
            ),
            Self::BlockTooLarge { frames } => {
                write!(
                    f,
                    "audio block of {frames} frames (max {MAX_MESSAGE_FRAMES})"
                )
            }
            Self::Unknown { msg_type } => write!(f, "unknown message 0x{msg_type:02X}"),
        }
    }
}

/// A registered instrument.
#[derive(Debug)]
pub struct Instrument {
    pub stream: UnixStream,
    pub id: [u8; 16],
    pub name: String,
    pub slot: usize,
    pub channels: u16,
    pub outbox: Outbox,
    read_buf: FrameBuffer,
    producer: AudioBlockProducer,
    stamp: StudioStamp,
    scratch: Vec<f32>,
    /// Owed the whole song state: it has just joined, or its stream moved on
    /// the studio clock.
    pub needs_song: bool,
    /// Whether it renders on a timer and is told where the desk is playing
    /// its stream.
    paced: bool,
    /// When it was last told.
    last_pace: Option<Instant>,
    /// The lead beyond the usual it asked for at registration.
    asked_lead: u32,
    /// Frames its strip's ring holds.
    ring_frames: u64,
    /// The desk's rate.
    sample_rate: u32,
    /// Times its stream was placed on the studio clock, the first included.
    placements: u64,
    /// Blocks lost because the strip's ring was full.
    dropped: u64,
    /// Whether the stream was let go because the ring was full, and not
    /// placed again since.
    dropped_since: bool,
    /// What it was last told of those, and when.
    last_health: (DeskHealthMsg, Option<Instant>),
}

impl Instrument {
    /// Wrap a registered connection.
    #[must_use]
    pub fn new(
        stream: UnixStream,
        registration: Registration,
        read_buf: FrameBuffer,
        producer: AudioBlockProducer,
    ) -> Self {
        Self {
            stream,
            id: registration.id,
            name: registration.name,
            slot: registration.slot,
            channels: registration.channels,
            outbox: Outbox::default(),
            read_buf,
            producer,
            // Fitted to the desk's clock at the first block or pace (see
            // `fit_lead`).
            stamp: StudioStamp::with_extra_lead(0),
            needs_song: true,
            paced: registration.pace_lead_frames > 0,
            asked_lead: registration.pace_lead_frames,
            ring_frames: registration.ring_frames,
            sample_rate: registration.sample_rate,
            last_pace: None,
            placements: 0,
            dropped: 0,
            dropped_since: false,
            last_health: (DeskHealthMsg::default(), None),
            scratch: vec![0.0; MAX_MESSAGE_FRAMES * usize::from(registration.channels)],
        }
    }

    /// Read the next complete message, if one has arrived, and act on it.
    ///
    /// # Errors
    ///
    /// [`ConnectionError::Io`] if the socket failed or closed, and
    /// [`ConnectionError::Protocol`] for a malformed message.
    pub fn poll(&mut self, clock: DeskClock) -> Result<Option<Inbound>, ConnectionError> {
        self.fit_lead(clock);
        let Some(header) = self
            .read_buf
            .try_read_frame(&mut &self.stream)
            .map_err(ConnectionError::Io)?
        else {
            return Ok(None);
        };
        self.handle(header, clock)
            .map(Some)
            .map_err(ConnectionError::Protocol)
    }

    fn handle(&mut self, header: FrameHeader, clock: DeskClock) -> Result<Inbound, ProtocolError> {
        let len = header.payload_len as usize;
        let payload = &self.read_buf.payload()[..len];
        let need = |needed: usize| {
            if len < needed {
                Err(ProtocolError::Truncated {
                    msg_type: header.msg_type,
                    needed,
                    got: len,
                })
            } else {
                Ok(())
            }
        };
        match header.msg_type {
            MSG_AUDIO => {
                need(AUDIO_PAYLOAD_HEADER)?;
                let frames = decode_audio_frame_count(payload) as usize;
                if frames > MAX_MESSAGE_FRAMES {
                    return Err(ProtocolError::BlockTooLarge { frames });
                }
                let samples = frames * usize::from(self.channels);
                need(AUDIO_PAYLOAD_HEADER + samples * 4)?;
                if frames == 0 {
                    return Ok(Inbound::Audio { resynced: false });
                }
                decode_audio_samples(payload, &mut self.scratch, samples);
                let stream_frame = decode_audio_stream_frame(payload);
                Ok(self.push_audio(stream_frame, frames, samples, clock))
            }
            MSG_NOTE_EVENT => {
                need(NoteEventMsg::WIRE_SIZE)?;
                let mut message = NoteMessage {
                    target: [0; 16],
                    payload: [0; NoteEventMsg::WIRE_SIZE],
                };
                message
                    .payload
                    .copy_from_slice(&payload[..NoteEventMsg::WIRE_SIZE]);
                message.target = NoteEventMsg::decode(payload).target;
                Ok(Inbound::Note(message))
            }
            MSG_TRANSPORT_REQUEST => {
                need(TransportRequestMsg::WIRE_SIZE)?;
                let request = TransportRequestMsg::decode(payload);
                Ok(Inbound::Transport {
                    state: request.requested_state,
                    bpm: (request.has_bpm == 1).then_some(request.requested_bpm),
                })
            }
            MSG_SHUTDOWN => Ok(Inbound::Goodbye),
            msg_type => Err(ProtocolError::Unknown { msg_type }),
        }
    }

    fn push_audio(
        &mut self,
        stream_frame: u64,
        frames: usize,
        samples: usize,
        clock: DeskClock,
    ) -> Inbound {
        // At most MAX_MESSAGE_FRAMES, checked by the caller.
        let frames_u32 = u32::try_from(frames).unwrap_or(u32::MAX);
        let stamped = self.stamp.stamp(stream_frame, frames_u32, clock);
        let pushed = self.producer.push_block(AudioBlock {
            header: AudioBlockHeader {
                start_frame: stamped.start_frame,
                frames: frames_u32,
                channels: self.channels,
                sequence: 0,
                flags: BlockFlags::default(),
            },
            samples: &self.scratch[..samples],
        });
        // A placement after the ring was full is the desk starting the
        // stream afresh once it takes it again, not a block that came late:
        // the drop was counted, and that is not.
        if stamped.resynced && !std::mem::take(&mut self.dropped_since) {
            self.placements = self.placements.saturating_add(1);
        }
        match pushed {
            Ok(()) => Inbound::Audio {
                resynced: stamped.resynced,
            },
            Err(AudioRingPushError::Full | AudioRingPushError::InvalidSampleCount { .. }) => {
                // The desk is not consuming (stream stopped, or the strip is
                // still being patched in): start afresh when it is.
                self.stamp.reset();
                self.dropped = self.dropped.saturating_add(1);
                self.dropped_since = true;
                Inbound::AudioDropped
            }
        }
    }

    /// Where this instrument's stream sits on the studio clock.
    #[must_use]
    pub const fn stamp(&self) -> &StudioStamp {
        &self.stamp
    }

    /// Tell a paced instrument where the desk is playing its stream, at
    /// most every [`PACE_INTERVAL`] and once its stream is placed. Nothing
    /// for an instrument clocked by its own device.
    ///
    /// # Errors
    ///
    /// See [`Outbox::queue`].
    pub fn pace(&mut self, clock: DeskClock, now: Instant) -> Result<(), OutboxError> {
        self.fit_lead(clock);
        if !self.paced
            || self
                .last_pace
                .is_some_and(|last| now.saturating_duration_since(last) < PACE_INTERVAL)
        {
            return Ok(());
        }
        let Some(playing_stream) = self.stamp.stream_at(clock.playing_now()) else {
            return Ok(());
        };
        let pace = DeskPaceMsg {
            playing_stream,
            lead_frames: u32::try_from(self.stamp.lead(clock.device_frames, clock.margin_frames))
                .unwrap_or(u32::MAX),
        };
        let mut payload = [0_u8; DeskPaceMsg::WIRE_SIZE];
        pace.encode(&mut payload);
        self.last_pace = Some(now);
        self.send(MSG_DESK_PACE, &payload)
    }

    /// Give the instrument the extra lead it asked for, held to what its
    /// strip's ring takes against the desk's `clock` now (see
    /// [`super::most_extra_lead`]): the device's buffer is only known once
    /// the desk has played, and may change when the device is rebuilt, so
    /// the hold is worked out again whenever the clock is read.
    fn fit_lead(&mut self, clock: DeskClock) {
        let most = super::most_extra_lead(self.ring_frames, clock, self.sample_rate);
        self.stamp.set_extra_lead(self.asked_lead.min(most));
    }

    /// How the desk has been taking this instrument's audio since it
    /// registered.
    #[must_use]
    pub const fn health(&self) -> DeskHealthMsg {
        DeskHealthMsg {
            placed_afresh: self.placements.saturating_sub(1),
            dropped: self.dropped,
        }
    }

    /// Tell the instrument how the desk has been taking its audio, when
    /// that has changed since it was last told, at most every
    /// [`HEALTH_INTERVAL`].
    ///
    /// # Errors
    ///
    /// See [`Outbox::queue`].
    pub fn report_health(&mut self, now: Instant) -> Result<(), OutboxError> {
        let health = self.health();
        let (told, at) = self.last_health;
        if health == told
            || at.is_some_and(|at| now.saturating_duration_since(at) < HEALTH_INTERVAL)
        {
            return Ok(());
        }
        let mut payload = [0_u8; DeskHealthMsg::WIRE_SIZE];
        health.encode(&mut payload);
        self.last_health = (health, Some(now));
        self.send(MSG_DESK_HEALTH, &payload)
    }

    /// Queue a frame to this instrument.
    ///
    /// # Errors
    ///
    /// See [`Outbox::queue`].
    pub fn send(&mut self, msg_type: u8, payload: &[u8]) -> Result<(), OutboxError> {
        self.outbox.queue(msg_type, payload)
    }

    /// Write queued frames.
    ///
    /// # Errors
    ///
    /// See [`Outbox::flush`].
    pub fn flush(&mut self) -> Result<(), OutboxError> {
        self.outbox.flush(&mut &self.stream)
    }
}

/// What an instrument said about itself when it registered, after checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub id: [u8; 16],
    pub name: String,
    pub slot: usize,
    pub channels: u16,
    /// Extra lead asked for by an instrument that renders on a timer; 0
    /// for one clocked by its own device (see
    /// [`kazoo_core::ipc::types::RegisterMsg::pace_lead_frames`]). The
    /// desk gives it as much of that as its strip's ring takes.
    pub pace_lead_frames: u32,
    /// Frames the strip's ring holds.
    pub ring_frames: u64,
    /// The desk's rate.
    pub sample_rate: u32,
}

/// How a connection failed.
#[derive(Debug)]
pub enum ConnectionError {
    /// The socket failed or closed.
    Io(io::Error),
    /// The instrument broke the protocol.
    Protocol(ProtocolError),
}
