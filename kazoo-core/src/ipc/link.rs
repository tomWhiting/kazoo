//! An instrument's link to the hub, split so its audio callback stays
//! real-time safe.
//!
//! A program starts its link with
//! [`crate::host::link::connect_or_direct`] (or
//! [`crate::host::link::connect_config_or_direct`] for a full
//! [`LinkConfig`]), the one setup path, which returns two halves:
//!
//! - [`HubLink`] stays with the instrument's UI. It owns a link thread that
//!   finds the hub, registers, sends the audio, reads the hub's messages, and
//!   keeps trying to connect whenever the hub is not there, so an instrument
//!   started before kazoo-mix plugs in as soon as the desk appears.
//! - [`HubLinkAudio`] moves into the audio callback. It queues rendered audio
//!   and requests, and hands over the hub's messages, through lock-free
//!   rings: no socket I/O, locks or allocation on the audio thread.
//!   Queuing a block or request wakes the link thread immediately. Otherwise
//!   the connected thread ticks at [`LINK_TICK`] to read the hub's messages.
//!
//! An instrument asks for its link thread to be scheduled as real-time by default
//! while it is plugged in ([`LinkConfig::real_time`]; see
//! [`crate::realtime`]): a block the thread carries late reaches the desk
//! after its time, and the desk places the stream afresh, which is heard as
//! a jump. The desk says how it has been taking the audio
//! ([`HubLink::desk_health`]), so a jump can be seen as well as heard.
//!
//! While [`HubLinkAudio::send_audio`] returns `true`, the desk is playing the
//! instrument's audio, and the instrument should silence its own output so it
//! is not heard twice.
//!
//! On joining, the callback first receives a [`HubMessage::TransportSync`]
//! with the desk's tempo and play state, so a new instrument starts in step.
//! Either half can ask the desk to play, stop or change tempo, and send notes
//! to the other instruments.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};

use super::client::{HubIpcClient, HubMessage};
use super::discovery;
use super::outbox::OutboxError;
use super::protocol::{AUDIO_PAYLOAD_HEADER, FrameHeader, HEADER_SIZE, encode_header};
use super::types::{
    DeskHealthMsg, DeskPaceMsg, NOTE_CC, NOTE_OFF, NOTE_ON, NOTE_PITCH_BEND, NoteEventMsg,
    RegisterMsg, SYNC_NOW, TRANSPORT_PAUSED, TRANSPORT_PLAYING, TRANSPORT_RECORDING,
    TRANSPORT_STOPPED, TRANSPORT_UNCHANGED, TransportRequestMsg, TransportSyncMsg,
};
use super::types::{
    MAX_STREAMS, MSG_AUDIO, MSG_STREAM_AUDIO, NO_STRIP, STREAM_NAME_LEN, StreamAddMsg, StreamId,
    StreamRemoveMsg, StreamSpec,
};
use crate::realtime::{NotRealTime, RealTime};
use crate::sanitize_sample;

mod audio;
mod voice;
pub use voice::{VOICE_RING, VoiceLinkStats, VoiceRx};
mod streams;
pub use streams::StreamConfig;
use streams::{OLD_DESK_REFUSAL, set_stream_refusal, stream_update};

mod priority;
mod shutdown;
#[cfg(test)]
pub(crate) use shutdown::LINK_CONSTRUCTIONS;
pub use shutdown::{stuck_shutdowns, undelivered_completions};
mod wire;

/// How often the link looks for a hub while none is answering.
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Longest wait between attempts after the desk refuses the instrument.
const MAX_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// How long the hub may stop reading before the link gives up on it.
const STUCK_LIMIT: Duration = Duration::from_millis(500);

/// Longest connected wait without a callback or UI wake.
///
/// Five milliseconds matches the desk's pace interval, so pace, health and shutdown
/// are read at least as often as pace is sent. Also the real-time period.
pub const LINK_TICK: Duration = Duration::from_millis(5);

/// Margin separating an unpark from a connected timeout.
///
/// `park_timeout` never returns more than a scheduling delay early on a timeout;
/// half a millisecond is far past any measured early return.
const WAKE_SLACK: Duration = Duration::from_micros(500);

/// Link thread poll interval while looking for a hub.
const IDLE_INTERVAL: Duration = Duration::from_millis(20);

/// Largest block sent to the hub, in frames; bigger callbacks are split.
pub const MAX_LINK_BLOCK_FRAMES: u32 = 16_384;

/// Audio retained between the callback and link thread, in milliseconds.
pub const LINK_BACKLOG_MS: u64 = 250;

/// Hub messages buffered for the callback.
const MESSAGE_BACKLOG: usize = 256;

/// Requests buffered from each half for the link thread.
const REQUEST_BACKLOG: usize = 64;

/// How old the desk's last pace may be and still be followed.
///
/// The desk sends one every few milliseconds; one this old means it has
/// stopped pacing (or the link is down), and the instrument keeps its own
/// time.
pub const PACE_STALE: Duration = Duration::from_millis(250);

/// Where the link looks for the hub.
#[derive(Debug, Clone)]
pub enum HubAddress {
    /// The hub kazoo-mix advertises through its PID file.
    Discover,
    /// A specific socket.
    Socket(PathBuf),
}

/// How the instrument describes itself to the hub.
#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// Instrument name, e.g. `kazoo-808`.
    pub name: String,
    /// 1 for mono, 2 for stereo.
    pub channels: u8,
    /// Named outputs; empty means one stream called main.
    pub streams: Vec<StreamConfig>,
    /// The instrument's sample rate; the hub only accepts its own.
    pub sample_rate: u32,
    /// Largest block the instrument renders, in frames. Larger blocks are
    /// still accepted: they are split before they reach the hub.
    pub max_block_frames: u32,
    /// Where to find the hub.
    pub address: HubAddress,
    /// 0 for an instrument clocked by its own audio device. An instrument
    /// that renders on a timer sets the extra lead it needs, in frames, and
    /// follows [`HubLinkAudio::desk_owes`] (see
    /// [`RegisterMsg::pace_lead_frames`]).
    pub pace_lead_frames: u32,
    /// Schedule the link thread as real-time while it is plugged in, so a
    /// busy machine does not hold the audio up on its way to the desk. On
    /// by default: the link thread carries audio to the desk. A refusal is
    /// reported through [`HubLink::real_time()`], never printed by the link.
    pub real_time: bool,
    #[cfg(test)]
    pub(crate) wake_audio: bool,
}

impl LinkConfig {
    /// An instrument that finds the hub the standard way.
    #[must_use]
    pub fn new(name: &str, channels: u8, sample_rate: u32, max_block_frames: u32) -> Self {
        Self {
            name: name.to_string(),
            channels,
            streams: Vec::new(),
            sample_rate,
            max_block_frames,
            address: HubAddress::Discover,
            pace_lead_frames: 0,
            real_time: true,
            #[cfg(test)]
            wake_audio: true,
        }
    }

    /// Disable audio wakes so the wake test can detect service by the tick.
    #[cfg(test)]
    pub(crate) fn for_test_without_wake(
        name: &str,
        channels: u8,
        sample_rate: u32,
        max_block_frames: u32,
    ) -> Self {
        Self {
            wake_audio: false,
            ..Self::new(name, channels, sample_rate, max_block_frames)
        }
    }
}

/// Why a request to the hub was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestError {
    /// The instrument is not plugged into the desk; act on it locally.
    NotConnected,
    /// Too many requests are waiting for the link thread.
    Full,
    /// The request is not valid: an unknown transport state or note event,
    /// a value out of MIDI range, or a tempo that is not a positive number.
    Invalid,
    /// The connected desk has not announced voice support.
    Unsupported,
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotConnected => "not plugged into the desk",
            Self::Full => "too many requests waiting for the hub",
            Self::Invalid => "invalid request",
            Self::Unsupported => "this desk is too old to carry the voice",
        })
    }
}

impl std::error::Error for RequestError {}

/// Something the instrument asks of the hub.
#[derive(Debug, Clone, Copy)]
enum Request {
    Transport { state: u8, bpm: Option<f32> },
    Note(NoteEventMsg),
    StreamAdd { stream: StreamId, spec: StreamSpec },
    StreamRemove { stream: StreamId },
    VoiceFollow(bool),
}

impl Request {
    fn transport(state: u8, bpm: Option<f32>) -> Result<Self, RequestError> {
        let known = match state {
            TRANSPORT_STOPPED | TRANSPORT_PLAYING | TRANSPORT_RECORDING | TRANSPORT_PAUSED => true,
            // Leaving the state alone only makes sense with a tempo to set.
            TRANSPORT_UNCHANGED => bpm.is_some(),
            _ => false,
        };
        let bad_tempo = bpm.is_some_and(|bpm| !(bpm.is_finite() && bpm > 0.0));
        if !known || bad_tempo {
            return Err(RequestError::Invalid);
        }
        Ok(Self::Transport { state, bpm })
    }

    const fn note(event: NoteEventMsg) -> Result<Self, RequestError> {
        let known = matches!(
            event.event_type,
            NOTE_ON | NOTE_OFF | NOTE_CC | NOTE_PITCH_BEND
        );
        if !known || event.channel > 15 || event.note > 127 || event.velocity > 127 {
            return Err(RequestError::Invalid);
        }
        Ok(Self::Note(event))
    }
}

/// State both halves and the link thread share.
#[derive(Debug)]
struct LinkState {
    connected: Arc<AtomicBool>,
    desk_features: AtomicU32,
    follow_voice: AtomicBool,
    voice_sent: AtomicU64,
    voice_dropped: AtomicU64,
    voice_received: AtomicU64,
    voice_overflow: AtomicU64,
    strips: [AtomicU8; MAX_STREAMS],
    streams: std::sync::Mutex<[Option<StreamConfig>; MAX_STREAMS]>,
    stream_live: AtomicU64,
    last_stream_refusal: std::sync::Mutex<Option<(StreamId, String)>>,
    blocks_sent: AtomicU64,
    blocks_dropped: AtomicU64,
    messages_dropped: AtomicU64,
    connections: AtomicU64,
    woken: AtomicU64,
    ticked: AtomicU64,
    last_refusal: std::sync::Mutex<Option<String>>,
    pace: PaceSlot,
    /// Scheduling is published before serving audio on each connection.
    real_time: AtomicU8,
    /// The first refusal reason remains available without a UI lock.
    real_time_refusal: std::sync::OnceLock<Result<(), NotRealTime>>,
    /// The desk's count of fresh placements over every connection so far.
    placed_afresh: AtomicU64,
    /// The desk's count of dropped blocks over every connection so far.
    desk_dropped: AtomicU64,
}

/// The desk's latest pace, written by the link thread and read by the
/// audio half without a lock: a sequence count, odd while a write is under
/// way, tells a reader whether what it read was whole.
#[derive(Debug)]
struct PaceSlot {
    /// Where time is counted from.
    epoch: Instant,
    sequence: AtomicU64,
    playing_stream: AtomicU64,
    lead_frames: AtomicU64,
    /// Nanoseconds after `epoch` the pace arrived; [`Self::NONE`] when
    /// there is none.
    arrived: AtomicU64,
}

/// What a paced instrument owes the desk (see
/// [`HubLinkAudio::desk_owes`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeskOwes {
    /// Frames to render now to be as far ahead as the desk places the
    /// stream: positive, render (a block at a time); zero or less, that many
    /// frames ahead, so wait. More than [`Self::lead_frames`] means the
    /// desk is already playing past the next frame: that block will be late
    /// and the desk will place the stream afresh, so rendering the rest of
    /// the debt only piles audio up behind it.
    pub frames: i64,
    /// How far ahead of the frame playing the desk places the stream.
    pub lead_frames: u64,
    /// When the pace this is reckoned from arrived.
    pub paced_at: Instant,
}

/// Frames owed at `age` after `pace` arrived, by an instrument at `rate`
/// whose next frame is `next_frame`: where the desk wants the stream up
/// to, less where it is, as a signed distance (stream frames wrap).
fn owed(pace: Pace, age: Duration, rate: u32, next_frame: u64) -> i64 {
    // At most PACE_STALE of frames: far inside u64, and exact enough.
    let since = (age.as_secs_f64() * f64::from(rate)) as u64;
    let wanted = pace
        .playing_stream
        .wrapping_add(since)
        .wrapping_add(pace.lead_frames);
    // Two's-complement reinterpretation: the signed distance.
    i64::from_ne_bytes(wanted.wrapping_sub(next_frame).to_ne_bytes())
}

/// A pace as the audio half reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pace {
    playing_stream: u64,
    lead_frames: u64,
    arrived: Duration,
}

impl PaceSlot {
    const NONE: u64 = u64::MAX;

    fn new() -> Self {
        Self {
            epoch: Instant::now(),
            sequence: AtomicU64::new(0),
            playing_stream: AtomicU64::new(0),
            lead_frames: AtomicU64::new(0),
            arrived: AtomicU64::new(Self::NONE),
        }
    }

    /// Keep `pace`, which arrived at `at`, or forget the pace (`None`).
    /// Only the link thread writes.
    fn store(&self, pace: Option<(DeskPaceMsg, Instant)>) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        match pace {
            Some((pace, at)) => {
                // Centuries of nanoseconds fit u64; saturate beyond.
                let arrived = u64::try_from(at.saturating_duration_since(self.epoch).as_nanos())
                    .unwrap_or(Self::NONE - 1);
                self.playing_stream
                    .store(pace.playing_stream, Ordering::Release);
                self.lead_frames
                    .store(u64::from(pace.lead_frames), Ordering::Release);
                self.arrived.store(arrived, Ordering::Release);
            }
            None => self.arrived.store(Self::NONE, Ordering::Release),
        }
        self.sequence.fetch_add(1, Ordering::AcqRel);
    }

    /// The pace, read whole; `None` when there is none, or when the link
    /// thread kept writing through every try. Real-time safe.
    fn load(&self) -> Option<Pace> {
        for _ in 0..4 {
            let before = self.sequence.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let playing_stream = self.playing_stream.load(Ordering::Acquire);
            let lead_frames = self.lead_frames.load(Ordering::Acquire);
            let arrived = self.arrived.load(Ordering::Acquire);
            if self.sequence.load(Ordering::Acquire) != before {
                continue;
            }
            return (arrived != Self::NONE).then(|| Pace {
                playing_stream,
                lead_frames,
                arrived: Duration::from_nanos(arrived),
            });
        }
        None
    }
}

/// The voice's way to an instrument through the desk.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoiceLink {
    /// The instrument's persistent follow wish.
    pub follows: bool,
    /// The connected desk announced voice support.
    pub desk_carries: bool,
}

/// Counters for the instrument's UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkStatus {
    /// Whether the voice reaches this instrument through the desk.
    pub voice: VoiceLink,
    /// The link was refused real-time scheduling.
    pub normal_priority: bool,
    /// Plugged into the desk.
    pub connected: bool,
    /// Desk strip, when plugged in (0-based).
    pub strip: Option<u8>,
    /// Audio blocks the callback queued for the hub (a discarded backlog also counts as dropped).
    pub blocks_sent: u64,
    /// Audio blocks lost: malformed, the link could not keep up, or the hub
    /// stopped reading. A discarded backlog counts once.
    pub blocks_dropped: u64,
    /// Messages lost in either direction: hub messages the callback did not
    /// read in time, and requests lost because the link went down or the hub
    /// stopped reading.
    pub messages_dropped: u64,
    /// Times the link has connected.
    pub connections: u64,
    /// Why the link last failed to plug in or was unplugged, if it was.
    pub last_refusal: Option<String>,
}

/// How `last_refusal` begins when the desk itself refused the instrument:
/// this, the instrument's name, `": "`, then the desk's reason.
pub(crate) const DESK_REFUSED: &str = "the desk refused ";

impl LinkStatus {
    /// The desk's own reason, when the desk refused this instrument; `None`
    /// when it did not (connected, never tried, or the link failed or came
    /// down for another reason, which `last_refusal` still holds).
    #[must_use]
    pub fn desk_refusal(&self) -> Option<&str> {
        self.last_refusal
            .as_deref()?
            .strip_prefix(DESK_REFUSED)?
            .split_once(": ")
            .map(|(_, reason)| reason)
    }

    /// The desk's rate, when the desk refused this instrument for running
    /// at another (see [`rate_refusal`]) and said a rate an instrument can
    /// run at; `None` otherwise.
    #[must_use]
    pub fn refused_for_rate(&self) -> Option<u32> {
        refused_desk_rate(self.desk_refusal()?)
    }
}

/// The lowest and highest rates the desk's rate is believed at.
const DESK_RATES: std::ops::RangeInclusive<u32> = 8_000..=768_000;

/// The words a desk refuses an instrument at `instrument` Hz with when it
/// runs at `desk` Hz.
///
/// The desk says them, and an instrument reads the desk's rate back from
/// them (see [`LinkStatus::refused_for_rate`]).
#[must_use]
pub fn rate_refusal(instrument: u32, desk: u32) -> String {
    format!("it runs at {instrument} Hz, the desk at {desk} Hz (use the same device)")
}

/// The desk's rate in `reason`, when it is a [`rate_refusal`] naming a
/// rate in [`DESK_RATES`].
fn refused_desk_rate(reason: &str) -> Option<u32> {
    let (_, rest) = reason
        .strip_prefix("it runs at ")?
        .split_once(" Hz, the desk at ")?;
    let digits = rest.strip_suffix(" Hz (use the same device)")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits
        .parse::<u32>()
        .map_or(None, |rate| DESK_RATES.contains(&rate).then_some(rate))
}

/// The UI half of the link. Dropping it says goodbye to the hub.
#[derive(Debug)]
pub struct HubLink {
    state: Arc<LinkState>,
    requests: SyncSender<Request>,
    running: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    finished: Receiver<()>,
}

/// The audio-callback half of the link.
pub struct HubLinkAudio {
    voice_out: ringbuf::HeapProd<crate::voice::VoiceFrame>,
    pub voice: VoiceRx,
    wire: ringbuf::HeapProd<u8>,
    seq: u32,
    pub events: crate::host::link::EventRx,
    requests: ringbuf::HeapProd<Request>,
    waker: Thread,
    #[cfg(test)]
    wake_audio: bool,
    state: Arc<LinkState>,
    channels: u16,
    block_frames: u32,
    next_frames: [u64; MAX_STREAMS],
    stream_channels: [Option<u16>; MAX_STREAMS],
    stream_updates: Arc<[AtomicU64; MAX_STREAMS]>,
    stream_versions: [u64; MAX_STREAMS],
    /// Bytes that may wait in `wire` at once: a quarter second of the
    /// streams there are now (see [`wire::link_ring_capacity`]).
    ring_budget: usize,
    sample_rate: u32,
}

impl fmt::Debug for HubLinkAudio {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HubLinkAudio")
            .field("channels", &self.channels)
            .field("block_frames", &self.block_frames)
            .field("connected", &self.is_connected())
            .finish_non_exhaustive()
    }
}

/// Start a link that looks for the hub on its own thread.
///
/// Only kazoo-core's tests call this. Programs go through
/// [`crate::host::link::connect_or_direct`], the one setup path.
///
/// # Errors
///
/// Fails only if the link thread cannot be started. Not finding a hub is not
/// an error: the link keeps looking.
#[cfg(test)]
pub(crate) fn hub_link(config: LinkConfig) -> io::Result<(HubLink, HubLinkAudio)> {
    start_link(config, None, true)
}

impl LinkState {
    /// A link not yet plugged in, carrying the resolved streams.
    fn fresh(slots: [Option<StreamConfig>; MAX_STREAMS]) -> Self {
        Self {
            connected: Arc::new(AtomicBool::new(false)),
            desk_features: AtomicU32::new(0),
            follow_voice: AtomicBool::new(false),
            voice_sent: AtomicU64::new(0),
            voice_dropped: AtomicU64::new(0),
            voice_received: AtomicU64::new(0),
            voice_overflow: AtomicU64::new(0),
            strips: std::array::from_fn(|_| AtomicU8::new(NO_STRIP)),
            stream_live: AtomicU64::new((1 << slots.iter().flatten().count()) - 1),
            streams: streams::table(slots),
            last_stream_refusal: std::sync::Mutex::new(None),
            blocks_sent: AtomicU64::new(0),
            blocks_dropped: AtomicU64::new(0),
            messages_dropped: AtomicU64::new(0),
            connections: AtomicU64::new(0),
            woken: AtomicU64::new(0),
            ticked: AtomicU64::new(0),
            last_refusal: std::sync::Mutex::new(None),
            pace: PaceSlot::new(),
            real_time: AtomicU8::new(priority::NOT_ASKED),
            real_time_refusal: std::sync::OnceLock::new(),
            placed_afresh: AtomicU64::new(0),
            desk_dropped: AtomicU64::new(0),
        }
    }
}

pub(crate) fn start_link(
    config: LinkConfig,
    client: Option<HubIpcClient>,
    start: bool,
) -> io::Result<(HubLink, HubLinkAudio)> {
    #[cfg(test)]
    LINK_CONSTRUCTIONS.with(|count| count.set(count.get() + 1));
    let slots = config.resolved_streams()?;
    let stream_channels =
        std::array::from_fn(|id| slots[id].as_ref().map(|slot| u16::from(slot.channels)));
    let channels = stream_channels[0]
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "stream 0 is missing"))?;
    let block_frames = config.max_block_frames.clamp(1, MAX_LINK_BLOCK_FRAMES);
    let wire_bytes = wire::link_ring_capacity(config.sample_rate, block_frames);
    let ring_budget = audio::ring_budget(config.sample_rate, &stream_channels, block_frames);
    let (wire_tx, wire_rx) = HeapRb::<u8>::new(wire_bytes).split();
    let (message_tx, message_rx) = HeapRb::<ToAudio>::new(MESSAGE_BACKLOG).split();
    let (callback_tx, callback_rx) = HeapRb::<Request>::new(REQUEST_BACKLOG).split();
    let (voice_out, voice, worker_voice) = voice::rings();
    let (ui_tx, ui_rx) = mpsc::sync_channel(REQUEST_BACKLOG);
    let state = Arc::new(LinkState::fresh(slots));
    let sample_rate = config.sample_rate;
    #[cfg(test)]
    let wake_audio = config.wake_audio;
    let running = Arc::new(AtomicBool::new(start));
    let mut worker = LinkWorker {
        voice: worker_voice,
        config,
        state: Arc::clone(&state),
        wire: wire_rx,
        audio_frame_left: 0,
        messages: message_tx,
        callback_requests: callback_rx,
        ui_requests: ui_rx,
        client: None,
        stuck_since: None,
        refusal_delay: RETRY_INTERVAL,
        health_before: DeskHealthMsg::default(),
        health_now: DeskHealthMsg::default(),
    };
    if let Some(client) = client {
        worker.plug_in(client);
    }
    let (finished_tx, finished) = mpsc::sync_channel(1);
    let thread_running = Arc::clone(&running);
    let spawn = || {
        thread::Builder::new()
            .name("kazoo-hub-link".to_string())
            .spawn(move || {
                worker.run(&thread_running);
                shutdown::report_finished(&finished_tx);
            })
    };
    let join = start.then(spawn).transpose()?;
    let waker = join
        .as_ref()
        .map_or_else(thread::current, |join| join.thread().clone());
    let events = crate::host::link::EventRx::new(message_rx);
    let stream_updates = Arc::clone(&events.stream_updates);
    for (id, channels) in stream_channels.iter().enumerate() {
        stream_updates[id].store(u64::from(channels.unwrap_or(0)), Ordering::Release);
    }
    Ok((
        HubLink {
            state: Arc::clone(&state),
            requests: ui_tx,
            running,
            join,
            finished,
        },
        HubLinkAudio {
            voice_out,
            voice,
            wire: wire_tx,
            seq: 0,
            events,
            requests: callback_tx,
            waker,
            #[cfg(test)]
            wake_audio,
            state,
            channels,
            block_frames,
            next_frames: [0; MAX_STREAMS],
            stream_channels,
            stream_updates,
            stream_versions: [0; MAX_STREAMS],
            ring_budget,
            sample_rate,
        },
    ))
}

impl HubLink {
    /// Connected parks ending early and at the tick, over the link's lifetime.
    ///
    /// Returns `(woken, ticked)` for tests and diagnostics, separate from
    /// the UI counters in [`LinkStatus`]. Idle parks are not counted.
    #[must_use]
    pub fn wake_counts(&self) -> (u64, u64) {
        (
            self.state.woken.load(Ordering::Relaxed),
            self.state.ticked.load(Ordering::Relaxed),
        )
    }

    /// Whether the instrument is plugged into the desk.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.state.connected.load(Ordering::Acquire)
    }

    /// A shared flag that follows [`Self::is_connected`], for UIs that poll.
    #[must_use]
    pub fn connected_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.state.connected)
    }

    /// The latest real-time scheduling result, or `None` before connecting
    /// or when scheduling was disabled. A refusal carries its reason.
    #[must_use]
    pub fn real_time(&self) -> Option<&Result<(), NotRealTime>> {
        priority::outcome(&self.state)
    }

    /// How the desk has taken this instrument's audio, over every
    /// connection so far: zero until the desk reports anything (a desk too
    /// old to report never does).
    #[must_use]
    pub fn desk_health(&self) -> DeskHealthMsg {
        DeskHealthMsg {
            placed_afresh: self.state.placed_afresh.load(Ordering::Relaxed),
            dropped: self.state.desk_dropped.load(Ordering::Relaxed),
        }
    }

    /// The link's counters.
    #[must_use]
    pub fn status(&self) -> LinkStatus {
        let strip = self.state.strips[0].load(Ordering::Acquire);
        let last_refusal = match self.state.last_refusal.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        LinkStatus {
            voice: VoiceLink {
                follows: self.state.follow_voice.load(Ordering::Acquire),
                desk_carries: self.desk_carries_voice(),
            },
            normal_priority: self.real_time().is_some_and(Result::is_err),
            connected: self.is_connected(),
            strip: (strip != NO_STRIP).then_some(strip),
            blocks_sent: self.state.blocks_sent.load(Ordering::Relaxed),
            blocks_dropped: self.state.blocks_dropped.load(Ordering::Relaxed),
            messages_dropped: self.state.messages_dropped.load(Ordering::Relaxed),
            connections: self.state.connections.load(Ordering::Relaxed),
            last_refusal,
        }
    }

    /// Ask the desk to change its transport: one of the `TRANSPORT_*`
    /// states, and optionally a new tempo. Every instrument, this one
    /// included, hears the result as a [`HubMessage::TransportSync`].
    ///
    /// # Errors
    ///
    /// [`RequestError::NotConnected`] when not plugged in (the instrument
    /// should act on it locally), [`RequestError::Full`] when the link is
    /// behind, [`RequestError::Invalid`] for an unknown state or a tempo that
    /// is not a positive number.
    pub fn request_transport(&self, state: u8, bpm: Option<f32>) -> Result<(), RequestError> {
        self.request(Request::transport(state, bpm)?)
    }

    /// Ask the desk for a new tempo and leave its play state alone, so a
    /// tempo change can never undo a start or stop this instrument has not
    /// heard about yet.
    ///
    /// # Errors
    ///
    /// As [`Self::request_transport`]; [`RequestError::Invalid`] for a tempo
    /// that is not a positive number.
    pub fn request_tempo(&self, bpm: f32) -> Result<(), RequestError> {
        self.request(Request::transport(TRANSPORT_UNCHANGED, Some(bpm))?)
    }

    /// Send a note event to the other instruments through the desk: to
    /// `event.target`, or to every other instrument when the target is all
    /// zeros. The link fills in `event.source`.
    ///
    /// # Errors
    ///
    /// As [`Self::request_transport`]; [`RequestError::Invalid`] for an
    /// unknown event type or a channel, note or velocity out of MIDI range.
    pub fn send_note(&self, event: NoteEventMsg) -> Result<(), RequestError> {
        self.request(Request::note(event)?)
    }

    fn request(&self, request: Request) -> Result<(), RequestError> {
        if !self.is_connected() {
            return Err(RequestError::NotConnected);
        }
        match self.requests.try_send(request) {
            Ok(()) => {
                if let Some(join) = self.join.as_ref() {
                    join.thread().unpark();
                }
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(RequestError::Full),
            // The link thread has stopped, so nothing reaches the desk.
            Err(TrySendError::Disconnected(_)) => Err(RequestError::NotConnected),
        }
    }
}

impl HubLink {
    pub(crate) fn shutdown(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(join) = self.join.take() {
            join.thread().unpark();
            if self.finished.recv_timeout(Duration::from_millis(500))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                shutdown::record_stuck();
                return;
            }
            if let Err(payload) = join.join() {
                if !thread::panicking() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
    }
}

impl Drop for HubLink {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl HubLinkAudio {
    /// Whether the desk is playing this instrument.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.state.connected.load(Ordering::Acquire)
    }

    /// The instrument's stream position: the frame the next block handed
    /// to [`Self::send_audio`] starts at. Every rendered frame counts,
    /// plugged in or not, so it is the clock the desk schedules transport
    /// changes on (see [`super::follow`]).
    #[must_use]
    pub const fn stream_frame(&self) -> u64 {
        self.next_frames[0]
    }

    /// For an instrument that renders on a timer and asked the desk to pace
    /// it ([`LinkConfig::pace_lead_frames`]): what it owes the desk at `now`
    /// to be as far ahead as the desk places its stream (see
    /// [`DeskOwes`]). `None` while the desk is not pacing it (not plugged
    /// in, a hub that does not pace, its stream not placed yet, or no word
    /// from the desk for [`PACE_STALE`]): keep its own time. Real-time safe.
    #[must_use]
    pub fn desk_owes(&self, now: Instant) -> Option<DeskOwes> {
        if !self.is_connected() {
            return None;
        }
        let pace = self.state.pace.load()?;
        let age = now
            .saturating_duration_since(self.state.pace.epoch)
            .saturating_sub(pace.arrived);
        if age > PACE_STALE {
            return None;
        }
        // An arrival the clock cannot hold is no pace at all.
        let paced_at = self.state.pace.epoch.checked_add(pace.arrived)?;
        Some(DeskOwes {
            frames: owed(pace, age, self.sample_rate, self.next_frames[0]),
            lead_frames: pace.lead_frames,
            paced_at,
        })
    }

    /// The next message from the hub, if one has arrived. Real-time safe.
    pub fn try_recv(&mut self) -> Option<HubMessage> {
        let message = self.events.try_recv();
        self.update_streams();
        message
    }

    /// Ask the desk to change its transport. Real-time safe. See
    /// [`HubLink::request_transport`].
    ///
    /// # Errors
    ///
    /// As [`HubLink::request_transport`].
    pub fn request_transport(&mut self, state: u8, bpm: Option<f32>) -> Result<(), RequestError> {
        self.request(Request::transport(state, bpm)?)
    }

    /// Ask the desk for a new tempo, leaving its play state alone.
    /// Real-time safe. See [`HubLink::request_tempo`].
    ///
    /// # Errors
    ///
    /// As [`HubLink::request_tempo`].
    pub fn request_tempo(&mut self, bpm: f32) -> Result<(), RequestError> {
        self.request(Request::transport(TRANSPORT_UNCHANGED, Some(bpm))?)
    }

    /// Send a note event through the desk. Real-time safe. See
    /// [`HubLink::send_note`].
    ///
    /// # Errors
    ///
    /// As [`HubLink::send_note`].
    pub fn send_note(&mut self, event: NoteEventMsg) -> Result<(), RequestError> {
        self.request(Request::note(event)?)
    }

    fn request(&mut self, request: Request) -> Result<(), RequestError> {
        if !self.is_connected() {
            return Err(RequestError::NotConnected);
        }
        self.requests
            .try_push(request)
            .map_err(|_| RequestError::Full)?;
        self.waker.unpark();
        Ok(())
    }
}

/// The link thread's state.
struct LinkWorker {
    voice: voice::VoiceWorker,
    config: LinkConfig,
    state: Arc<LinkState>,
    wire: ringbuf::HeapCons<u8>,
    /// Bytes left in an audio frame already started on the socket.
    audio_frame_left: usize,
    messages: ringbuf::HeapProd<ToAudio>,
    callback_requests: ringbuf::HeapCons<Request>,
    ui_requests: Receiver<Request>,
    client: Option<HubIpcClient>,
    /// When the hub stopped taking audio.
    stuck_since: Option<Instant>,
    /// Wait after the next refusal; doubles on each one.
    refusal_delay: Duration,
    /// The desk's counts from connections that have ended.
    health_before: DeskHealthMsg,
    /// The desk's latest counts for this connection.
    health_now: DeskHealthMsg,
}

impl LinkWorker {
    fn run(&mut self, running: &AtomicBool) {
        // Real-time (when asked for) only while plugged in and serving:
        // finding the desk reads a file, registering blocks for up to its
        // timeout, and connecting, disconnecting and saying goodbye allocate
        // and take the refusal lock the UI shares, so all of that happens at
        // the thread's normal priority. Serving allocates only on the way
        // to a disconnection (the reason), after which it demotes.
        let mut priority: Option<RealTime> = if self.client.is_some() {
            self.promote()
        } else {
            None
        };
        let mut next_attempt = Instant::now();
        while running.load(Ordering::Acquire) {
            if self.client.is_none() {
                self.discard_backlog();
                if Instant::now() >= next_attempt {
                    next_attempt = Instant::now() + self.try_connect();
                    if self.client.is_some() {
                        priority = self.promote();
                    }
                }
                thread::park_timeout(IDLE_INTERVAL);
                continue;
            }
            if let Err(why) = self.serve() {
                let why = match Self::demote(priority.take()) {
                    Ok(()) => why,
                    Err(refusal) => format!(
                        "{why}; the desk link could not return to normal priority ({refusal})"
                    ),
                };
                self.disconnect(why);
                next_attempt = Instant::now() + RETRY_INTERVAL;
            }
            let parked_at = Instant::now();
            thread::park_timeout(LINK_TICK);
            if parked_at.elapsed().saturating_add(WAKE_SLACK) < LINK_TICK {
                self.state.woken.fetch_add(1, Ordering::Relaxed);
            } else {
                self.state.ticked.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Err(why) = Self::demote(priority.take()) {
            self.set_refusal(Some(format!(
                "the desk link could not return to normal priority ({why})"
            )));
        }
        self.say_goodbye();
    }

    /// Ask for real-time scheduling, if the instrument wants it.
    fn promote(&self) -> Option<RealTime> {
        if !self.config.real_time {
            return None;
        }
        priority::promote(&self.state, || RealTime::promote(LINK_TICK))
    }

    /// Go back to normal priority, if promoted.
    fn demote(priority: Option<RealTime>) -> Result<(), NotRealTime> {
        priority.map_or(Ok(()), RealTime::demote)
    }

    fn set_refusal(&self, refusal: Option<String>) {
        match self.state.last_refusal.lock() {
            Ok(mut guard) => *guard = refusal,
            Err(poisoned) => *poisoned.into_inner() = refusal,
        }
    }

    /// Send queued requests and audio, read the hub's messages. An error
    /// ends the connection.
    fn serve(&mut self) -> Result<(), String> {
        let Some(client) = self.client.as_mut() else {
            return Ok(());
        };
        // Finish an audio frame before putting a control frame on the wire.
        if self.audio_frame_left > 0 {
            wire::write_wire(
                &mut self.wire,
                client,
                &mut self.audio_frame_left,
                &mut self.stuck_since,
                true,
            )?;
        }
        if self.audio_frame_left == 0 {
            let source = *client.instrument_id();
            while let Some(request) = self.callback_requests.try_pop() {
                send_request(
                    client,
                    request,
                    source,
                    &self.state,
                    &mut self.messages,
                    &mut self.stuck_since,
                )?;
            }
            while let Ok(request) = self.ui_requests.try_recv() {
                send_request(
                    client,
                    request,
                    source,
                    &self.state,
                    &mut self.messages,
                    &mut self.stuck_since,
                )?;
            }
            self.voice
                .send(client, &self.state, &mut self.stuck_since)?;
            let before = client.pending_bytes();
            client
                .flush()
                .map_err(|err| format!("sending to the desk failed: {err}"))?;
            let after = client.pending_bytes();
            if after < before {
                self.stuck_since = None;
            }
            if after > 0 {
                wire::check_stuck(&mut self.stuck_since)?;
            } else {
                wire::write_wire(
                    &mut self.wire,
                    client,
                    &mut self.audio_frame_left,
                    &mut self.stuck_since,
                    false,
                )?;
            }
        }
        self.read_hub()
    }

    fn disconnect(&mut self, why: String) {
        // The reason goes first: anyone who sees the link down can read why.
        self.set_refusal(Some(why));
        self.state.connected.store(false, Ordering::Release);
        self.state.desk_features.store(0, Ordering::Release);
        self.voice.discard(self.client.as_ref(), &self.state);
        for strip in &self.state.strips {
            strip.store(NO_STRIP, Ordering::Release);
        }
        self.state.pace.store(None);
        self.client = None;
        // The desk counts afresh for the next connection.
        self.health_before = DeskHealthMsg {
            placed_afresh: self
                .health_before
                .placed_afresh
                .saturating_add(self.health_now.placed_afresh),
            dropped: self
                .health_before
                .dropped
                .saturating_add(self.health_now.dropped),
        };
        self.health_now = DeskHealthMsg::default();
        self.publish_health();
        self.discard_backlog();
        self.audio_frame_left = 0;
        self.stuck_since = None;
    }

    /// Publish the desk's counts over every connection so far.
    fn publish_health(&self) {
        self.state.placed_afresh.store(
            self.health_before
                .placed_afresh
                .saturating_add(self.health_now.placed_afresh),
            Ordering::Relaxed,
        );
        self.state.desk_dropped.store(
            self.health_before
                .dropped
                .saturating_add(self.health_now.dropped),
            Ordering::Relaxed,
        );
    }

    /// Drop audio and requests the halves queued before they saw the link
    /// go down.
    fn discard_backlog(&mut self) {
        self.voice.discard(self.client.as_ref(), &self.state);
        // Count what `clear` removed: a frame may land after any earlier look.
        if self.wire.clear() > 0 {
            self.state.blocks_dropped.fetch_add(1, Ordering::Relaxed);
        }
        while self.callback_requests.try_pop().is_some() {
            self.state.messages_dropped.fetch_add(1, Ordering::Relaxed);
        }
        while let Ok(request) = self.ui_requests.try_recv() {
            // A stream change is already in the table the next registration
            // is built from: nothing is lost.
            if !matches!(
                request,
                Request::StreamAdd { .. } | Request::StreamRemove { .. } | Request::VoiceFollow(_)
            ) {
                self.state.messages_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Tell the hub the instrument is leaving, and give it a moment to go.
    fn say_goodbye(&mut self) {
        self.state.connected.store(false, Ordering::Release);
        self.state.desk_features.store(0, Ordering::Release);
        self.voice.discard(self.client.as_ref(), &self.state);
        let Some(client) = self.client.as_mut() else {
            return;
        };
        if self.audio_frame_left > 0 {
            if let Err(why) = wire::write_wire(
                &mut self.wire,
                client,
                &mut self.audio_frame_left,
                &mut self.stuck_since,
                true,
            ) {
                self.set_refusal(Some(why));
                self.client = None;
                return;
            }
            if self.audio_frame_left > 0 {
                self.client = None;
                return;
            }
        }
        if client.send_shutdown().is_err() {
            // The connection is already gone or wedged; closing it below
            // tells the hub the same thing.
            self.client = None;
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(100);
        while client.pending_bytes() > 0 && Instant::now() < deadline {
            if client.flush().is_err() {
                break;
            }
            thread::park_timeout(Duration::from_millis(1));
        }
        self.client = None;
    }
}

/// Queue one request for the hub.
fn send_request(
    client: &mut HubIpcClient,
    request: Request,
    source: [u8; 16],
    state: &LinkState,
    messages: &mut ringbuf::HeapProd<ToAudio>,
    stuck_since: &mut Option<Instant>,
) -> Result<(), String> {
    if !client.desk_takes_streams()
        && let Some(update) = stream_update(&request)
    {
        // A desk older than named streams answered the registration without
        // strips and would drop the instrument for a 0x12 or 0x13. The
        // callback still learns the stream (it plays it itself, having no
        // strip); the instrument can read why.
        if let Request::StreamAdd { stream, .. } = request {
            set_stream_refusal(state, stream, OLD_DESK_REFUSAL);
        }
        return messages
            .try_push(update)
            .map_err(|_| "callback stream queue is full".to_string());
    }
    let before = client.pending_bytes();
    let payload_len = match &request {
        Request::Transport { .. } => TransportRequestMsg::WIRE_SIZE,
        Request::Note(_) => NoteEventMsg::WIRE_SIZE,
        Request::StreamAdd { .. } => StreamAddMsg::WIRE_SIZE,
        Request::StreamRemove { .. } => StreamRemoveMsg::WIRE_SIZE,
        Request::VoiceFollow(_) => 1,
    };
    let sent = match request {
        Request::Transport { state, bpm } => client.send_transport_request(state, bpm),
        Request::Note(event) => client.send_note_event(&NoteEventMsg { source, ..event }),
        Request::StreamAdd { stream, spec } => {
            client.send_stream_add(&StreamAddMsg { stream, spec })
        }
        Request::StreamRemove { stream } => client.send_stream_remove(stream),
        Request::VoiceFollow(on) => voice::send_follow(client, state, on),
    };
    match sent {
        Ok(()) => {
            if let Some(update) = stream_update(&request) {
                messages
                    .try_push(update)
                    .map_err(|_| "callback stream queue is full".to_string())?;
            }
            if client.pending_bytes() < before + HEADER_SIZE + payload_len {
                *stuck_since = None;
            }
            Ok(())
        }
        // The hub is not reading; the audio path decides when to give up.
        Err(OutboxError::Stuck)
            if matches!(
                request,
                Request::StreamAdd { .. } | Request::StreamRemove { .. } | Request::VoiceFollow(_)
            ) =>
        {
            Err("stream or voice follow request outbox is full".to_string())
        }
        Err(OutboxError::Stuck) => {
            state.messages_dropped.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        Err(OutboxError::TooLarge) => Err("a request was too large for the desk".to_string()),
        Err(OutboxError::Io(err)) => Err(format!("sending to the desk failed: {err}")),
    }
}

#[cfg(test)]
mod tests;

/// Internal stream bookkeeping is consumed before events reach an instrument.
#[derive(Debug)]
pub(crate) enum ToAudio {
    Hub(HubMessage),
    Stream {
        stream: StreamId,
        channels: Option<u16>,
    },
}
