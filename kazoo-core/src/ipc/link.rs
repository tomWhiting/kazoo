//! An instrument's link to the hub, split so its audio callback stays
//! real-time safe.
//!
//! [`hub_link`] returns two halves:
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
//! An instrument can ask for its link thread to be scheduled as real-time
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
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};

use super::client::{HubIpcClient, HubMessage};
use super::discovery;
use super::outbox::OutboxError;
use super::types::{
    DeskHealthMsg, DeskPaceMsg, NOTE_CC, NOTE_OFF, NOTE_ON, NOTE_PITCH_BEND, NoteEventMsg,
    RegisterMsg, SYNC_NOW, TRANSPORT_PAUSED, TRANSPORT_PLAYING, TRANSPORT_RECORDING,
    TRANSPORT_STOPPED, TRANSPORT_UNCHANGED, TransportSyncMsg,
};
use crate::audio_transport::{
    AudioBlock, AudioBlockConsumer, AudioBlockProducer, AudioRingConfig, AudioRingPopError,
    AudioRingPushError, audio_block_ring,
};
use crate::protocol::{AudioBlockHeader, BlockFlags, BufferId};
use crate::realtime::RealTime;

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

/// Link thread poll interval while looking for a hub.
const IDLE_INTERVAL: Duration = Duration::from_millis(20);

/// Largest block sent to the hub, in frames; bigger callbacks are split.
pub const MAX_LINK_BLOCK_FRAMES: u32 = 4096;

/// Audio blocks buffered between the callback and the link thread.
const AUDIO_BLOCKS: u32 = 64;

/// Hub messages buffered for the callback.
const MESSAGE_BACKLOG: usize = 256;

/// Requests buffered from each half for the link thread.
const REQUEST_BACKLOG: usize = 64;

/// Strip value meaning "not plugged in".
const NO_STRIP: u8 = u8::MAX;

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
    /// busy machine does not hold the audio up on its way to the desk. Off
    /// by default. A refusal is said once on stderr, prefixed with
    /// [`Self::name`], so turn it on only in a program whose stderr is a
    /// log rather than a terminal it draws on.
    pub real_time: bool,
}

impl LinkConfig {
    /// An instrument that finds the hub the standard way.
    #[must_use]
    pub fn new(name: &str, channels: u8, sample_rate: u32, max_block_frames: u32) -> Self {
        Self {
            name: name.to_string(),
            channels,
            sample_rate,
            max_block_frames,
            address: HubAddress::Discover,
            pace_lead_frames: 0,
            real_time: false,
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
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotConnected => "not plugged into the desk",
            Self::Full => "too many requests waiting for the hub",
            Self::Invalid => "invalid request",
        })
    }
}

impl std::error::Error for RequestError {}

/// Something the instrument asks of the hub.
#[derive(Debug, Clone, Copy)]
enum Request {
    Transport { state: u8, bpm: Option<f32> },
    Note(NoteEventMsg),
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
    strip: AtomicU8,
    blocks_sent: AtomicU64,
    blocks_dropped: AtomicU64,
    messages_dropped: AtomicU64,
    connections: AtomicU64,
    last_refusal: std::sync::Mutex<Option<String>>,
    pace: PaceSlot,
    /// [`NOT_ASKED`], [`GRANTED`] or [`REFUSED`]: how the link thread's
    /// last request for real-time scheduling went.
    real_time: AtomicU8,
    /// The desk's count of fresh placements over every connection so far.
    placed_afresh: AtomicU64,
    /// The desk's count of dropped blocks over every connection so far.
    desk_dropped: AtomicU64,
}

/// The link thread has not asked for real-time scheduling.
const NOT_ASKED: u8 = 0;
/// It asked and was granted it.
const GRANTED: u8 = 1;
/// It asked and was refused.
const REFUSED: u8 = 2;

/// Whether a link thread in this process has said it was refused real-time
/// scheduling: a link that reconnects asks again, and once is enough to
/// say so.
static TOLD_REFUSED: AtomicBool = AtomicBool::new(false);

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

/// Counters for the instrument's UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkStatus {
    /// Plugged into the desk.
    pub connected: bool,
    /// Desk strip, when plugged in (0-based).
    pub strip: Option<u8>,
    /// Audio blocks sent to the hub.
    pub blocks_sent: u64,
    /// Audio blocks lost: malformed, the link could not keep up, or the hub
    /// stopped reading.
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

/// The UI half of the link. Dropping it says goodbye to the hub.
#[derive(Debug)]
pub struct HubLink {
    state: Arc<LinkState>,
    requests: SyncSender<Request>,
    running: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

/// The audio-callback half of the link.
pub struct HubLinkAudio {
    audio: AudioBlockProducer,
    messages: ringbuf::HeapCons<HubMessage>,
    requests: ringbuf::HeapProd<Request>,
    waker: Thread,
    state: Arc<LinkState>,
    channels: u16,
    block_frames: u32,
    next_frame: u64,
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

/// Start an instrument's link to the hub.
///
/// # Errors
///
/// Fails only if the link thread cannot be started. Not finding a hub is not
/// an error: the link keeps looking.
pub fn hub_link(config: LinkConfig) -> io::Result<(HubLink, HubLinkAudio)> {
    let channels = u16::from(config.channels.clamp(1, 2));
    let block_frames = config.max_block_frames.clamp(1, MAX_LINK_BLOCK_FRAMES);
    let (audio_tx, audio_rx) = audio_block_ring(AudioRingConfig::new(
        BufferId(0),
        channels,
        block_frames,
        AUDIO_BLOCKS,
    ));
    let (message_tx, message_rx) = HeapRb::<HubMessage>::new(MESSAGE_BACKLOG).split();
    let (callback_tx, callback_rx) = HeapRb::<Request>::new(REQUEST_BACKLOG).split();
    let (ui_tx, ui_rx) = mpsc::sync_channel(REQUEST_BACKLOG);
    let state = Arc::new(LinkState {
        connected: Arc::new(AtomicBool::new(false)),
        strip: AtomicU8::new(NO_STRIP),
        blocks_sent: AtomicU64::new(0),
        blocks_dropped: AtomicU64::new(0),
        messages_dropped: AtomicU64::new(0),
        connections: AtomicU64::new(0),
        last_refusal: std::sync::Mutex::new(None),
        pace: PaceSlot::new(),
        real_time: AtomicU8::new(NOT_ASKED),
        placed_afresh: AtomicU64::new(0),
        desk_dropped: AtomicU64::new(0),
    });
    let sample_rate = config.sample_rate;
    let running = Arc::new(AtomicBool::new(true));
    let mut worker = LinkWorker {
        config,
        state: Arc::clone(&state),
        audio: audio_rx,
        messages: message_tx,
        callback_requests: callback_rx,
        ui_requests: ui_rx,
        scratch: vec![0.0; block_frames as usize * usize::from(channels)],
        client: None,
        pending: None,
        stuck_since: None,
        refusal_delay: RETRY_INTERVAL,
        health_before: DeskHealthMsg::default(),
        health_now: DeskHealthMsg::default(),
    };
    let thread_running = Arc::clone(&running);
    let join = thread::Builder::new()
        .name("kazoo-hub-link".to_string())
        .spawn(move || worker.run(&thread_running))?;
    let waker = join.thread().clone();
    Ok((
        HubLink {
            state: Arc::clone(&state),
            requests: ui_tx,
            running,
            join: Some(join),
        },
        HubLinkAudio {
            audio: audio_tx,
            messages: message_rx,
            requests: callback_tx,
            waker,
            state,
            channels,
            block_frames,
            next_frame: 0,
            sample_rate,
        },
    ))
}

impl HubLink {
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

    /// Whether the link thread was granted real-time scheduling the last
    /// time it plugged in; `None` if it was not asked to be
    /// ([`LinkConfig::real_time`]) or has not plugged in yet.
    #[must_use]
    pub fn real_time(&self) -> Option<bool> {
        match self.state.real_time.load(Ordering::Acquire) {
            GRANTED => Some(true),
            REFUSED => Some(false),
            _ => None,
        }
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
        let strip = self.state.strip.load(Ordering::Acquire);
        let last_refusal = match self.state.last_refusal.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        LinkStatus {
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

impl Drop for HubLink {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(join) = self.join.take() {
            // The link thread may be parked for a whole tick; wake it so it
            // sees `running` and says goodbye now.
            join.thread().unpark();
            if let Err(payload) = join.join() {
                if !thread::panicking() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
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
        self.next_frame
    }

    /// Hand the link a rendered block of `frames` interleaved frames: every
    /// block the instrument renders, so the stream position stays true.
    /// Real-time safe. A block longer than the link's block size is split.
    ///
    /// Returns `true` while the desk is playing this instrument: the caller
    /// should then silence its own output. A block that cannot be queued (the
    /// link is behind, or `samples` does not hold `frames` whole frames) is
    /// counted as dropped, and the result is still `true`, since the desk is
    /// still the instrument's output.
    pub fn send_audio(&mut self, frames: u32, samples: &[f32]) -> bool {
        let start_frame = self.next_frame;
        self.next_frame = self.next_frame.wrapping_add(u64::from(frames));
        if !self.is_connected() {
            return false;
        }
        let channels = usize::from(self.channels);
        if (frames as usize).checked_mul(channels) != Some(samples.len()) {
            self.state.blocks_dropped.fetch_add(1, Ordering::Relaxed);
            return true;
        }
        let mut done = 0_u32;
        while done < frames {
            let count = (frames - done).min(self.block_frames);
            let start = done as usize * channels;
            let end = start + count as usize * channels;
            self.push(
                start_frame.wrapping_add(u64::from(done)),
                count,
                &samples[start..end],
            );
            done += count;
        }
        true
    }

    /// Wake only after publishing the block.
    ///
    /// `Thread::unpark` is constant time: an atomic swap and, only if parked,
    /// a wake system call. It never waits for the link thread. This is the
    /// callback's sole permitted system call; it adds no allocation, lock or
    /// socket I/O.
    fn push(&mut self, start_frame: u64, frames: u32, samples: &[f32]) {
        let pushed = self.audio.push_block(AudioBlock {
            header: AudioBlockHeader {
                start_frame,
                frames,
                channels: self.channels,
                sequence: 0,
                flags: BlockFlags::default(),
            },
            samples,
        });
        match pushed {
            Ok(()) => self.waker.unpark(),
            Err(AudioRingPushError::Full | AudioRingPushError::InvalidSampleCount { .. }) => {
                self.state.blocks_dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
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
            frames: owed(pace, age, self.sample_rate, self.next_frame),
            lead_frames: pace.lead_frames,
            paced_at,
        })
    }

    /// The next message from the hub, if one has arrived. Real-time safe.
    pub fn try_recv(&mut self) -> Option<HubMessage> {
        self.messages.try_pop()
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

/// An audio block in the link's scratch buffer that the hub has not taken.
#[derive(Debug, Clone, Copy)]
struct Pending {
    stream_frame: u64,
    frames: u32,
    samples: usize,
}

/// The link thread's state.
struct LinkWorker {
    config: LinkConfig,
    state: Arc<LinkState>,
    audio: AudioBlockConsumer,
    messages: ringbuf::HeapProd<HubMessage>,
    callback_requests: ringbuf::HeapCons<Request>,
    ui_requests: Receiver<Request>,
    scratch: Vec<f32>,
    client: Option<HubIpcClient>,
    /// The block in `scratch` waiting for room in the outbox.
    pending: Option<Pending>,
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
        let mut priority: Option<RealTime> = None;
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
                self.demote(priority.take());
                self.disconnect(why);
                next_attempt = Instant::now() + RETRY_INTERVAL;
            }
            thread::park_timeout(LINK_TICK);
        }
        self.demote(priority.take());
        self.say_goodbye();
    }

    /// Ask for real-time scheduling, if the instrument wants it.
    fn promote(&self) -> Option<RealTime> {
        if !self.config.real_time {
            return None;
        }
        match RealTime::promote(LINK_TICK) {
            Ok(priority) => {
                self.state.real_time.store(GRANTED, Ordering::Release);
                Some(priority)
            }
            Err(why) => {
                self.state.real_time.store(REFUSED, Ordering::Release);
                if !TOLD_REFUSED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "{}: the desk link runs at normal priority ({why}); on a busy \
                         machine the desk may hear it jump",
                        self.config.name
                    );
                }
                None
            }
        }
    }

    /// Go back to normal priority, if promoted.
    fn demote(&self, priority: Option<RealTime>) {
        if let Some(priority) = priority {
            if let Err(why) = priority.demote() {
                eprintln!(
                    "{}: the desk link could not return to normal priority ({why})",
                    self.config.name
                );
            }
        }
    }

    /// Try to plug in; returns how long to wait before trying again.
    fn try_connect(&mut self) -> Duration {
        let socket = match &self.config.address {
            HubAddress::Discover => match discovery::hub_socket() {
                Ok(socket) => socket,
                Err(err) => {
                    self.set_refusal(Some(format!("cannot find the desk: {err}")));
                    return RETRY_INTERVAL;
                }
            },
            HubAddress::Socket(path) => path.clone(),
        };
        let mut register = RegisterMsg::new(
            &self.config.name,
            self.config.channels,
            self.config.sample_rate,
            self.config.max_block_frames.min(MAX_LINK_BLOCK_FRAMES),
        );
        register.pace_lead_frames = self.config.pace_lead_frames;
        match HubIpcClient::register_at(&socket, &register) {
            Ok(client) => {
                self.plug_in(client);
                RETRY_INTERVAL
            }
            // No desk yet. Keep the last reason: it says why the link is
            // down until something new happens.
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                self.refusal_delay = RETRY_INTERVAL;
                RETRY_INTERVAL
            }
            Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                self.set_refusal(Some(format!(
                    "the desk refused {}: {err}",
                    self.config.name
                )));
                self.refusal_delay = (self.refusal_delay * 2).min(MAX_RETRY_INTERVAL);
                self.refusal_delay
            }
            Err(err) => {
                self.set_refusal(Some(format!(
                    "could not plug into the desk at {}: {err}",
                    socket.display()
                )));
                RETRY_INTERVAL
            }
        }
    }

    fn plug_in(&mut self, client: HubIpcClient) {
        let registration = client.registration();
        // The desk's tempo and play state at once; where in the song this
        // stream is follows as soon as the hub has placed its first block.
        let sync = TransportSyncMsg {
            state: registration.transport_state,
            bpm: registration.bpm,
            at_frame: SYNC_NOW,
            beat: f64::NAN,
        };
        if self
            .messages
            .try_push(HubMessage::TransportSync(sync))
            .is_err()
        {
            self.state.messages_dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.state
            .strip
            .store(client.strip_index(), Ordering::Release);
        self.state.connections.fetch_add(1, Ordering::Relaxed);
        self.set_refusal(None);
        self.refusal_delay = RETRY_INTERVAL;
        self.pending = None;
        self.stuck_since = None;
        self.client = Some(client);
        self.state.connected.store(true, Ordering::Release);
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
        let source = *client.instrument_id();
        while let Some(request) = self.callback_requests.try_pop() {
            send_request(client, request, source, &self.state)?;
        }
        // Stops when empty, or when the UI half is gone and the link is
        // closing.
        while let Ok(request) = self.ui_requests.try_recv() {
            send_request(client, request, source, &self.state)?;
        }
        loop {
            let pending = if let Some(pending) = self.pending {
                pending
            } else {
                match self.audio.pop_block(&mut self.scratch) {
                    Ok(block) => Pending {
                        stream_frame: block.header.start_frame,
                        frames: block.header.frames,
                        samples: block.samples_copied,
                    },
                    Err(AudioRingPopError::Empty) => break,
                    Err(AudioRingPopError::OutputTooSmall { required, .. }) => {
                        self.scratch.resize(required, 0.0);
                        continue;
                    }
                }
            };
            match client.send_audio(
                pending.stream_frame,
                pending.frames,
                &self.scratch[..pending.samples],
            ) {
                Ok(()) => {
                    self.state.blocks_sent.fetch_add(1, Ordering::Relaxed);
                    self.pending = None;
                    self.stuck_since = None;
                }
                Err(OutboxError::Stuck) => {
                    self.pending = Some(pending);
                    let now = Instant::now();
                    let since = *self.stuck_since.get_or_insert(now);
                    if now.duration_since(since) > STUCK_LIMIT {
                        return Err(format!(
                            "the desk stopped taking audio for over {} ms",
                            STUCK_LIMIT.as_millis()
                        ));
                    }
                    break;
                }
                Err(OutboxError::TooLarge) => {
                    return Err(format!(
                        "an audio block of {} frames is too large for the desk",
                        pending.frames
                    ));
                }
                Err(OutboxError::Io(err)) => return Err(format!("sending audio failed: {err}")),
            }
        }
        client
            .flush()
            .map_err(|err| format!("sending to the desk failed: {err}"))?;
        loop {
            match client.try_recv() {
                Ok(Some(HubMessage::Shutdown)) => return Err("the desk closed".to_string()),
                Ok(Some(message)) => {
                    if self.messages.try_push(message).is_err() {
                        self.state.messages_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Ok(None) => {
                    if let Some(pace) = client.take_desk_pace() {
                        self.state.pace.store(Some((pace, Instant::now())));
                    }
                    if let Some(health) = client.take_desk_health() {
                        self.health_now = health;
                        self.publish_health();
                    }
                    return Ok(());
                }
                Err(err) => return Err(format!("the desk connection failed: {err}")),
            }
        }
    }

    fn disconnect(&mut self, why: String) {
        // The reason goes first: anyone who sees the link down can read why.
        self.set_refusal(Some(why));
        self.state.connected.store(false, Ordering::Release);
        self.state.strip.store(NO_STRIP, Ordering::Release);
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
        if self.pending.take().is_some() {
            self.state.blocks_dropped.fetch_add(1, Ordering::Relaxed);
        }
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
        loop {
            match self.audio.pop_block(&mut self.scratch) {
                Ok(_) => {
                    self.state.blocks_dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(AudioRingPopError::Empty) => break,
                Err(AudioRingPopError::OutputTooSmall { required, .. }) => {
                    self.scratch.resize(required, 0.0);
                }
            }
        }
        while self.callback_requests.try_pop().is_some() {
            self.state.messages_dropped.fetch_add(1, Ordering::Relaxed);
        }
        while self.ui_requests.try_recv().is_ok() {
            self.state.messages_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Tell the hub the instrument is leaving, and give it a moment to go.
    fn say_goodbye(&mut self) {
        self.state.connected.store(false, Ordering::Release);
        let Some(client) = self.client.as_mut() else {
            return;
        };
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
) -> Result<(), String> {
    let sent = match request {
        Request::Transport { state, bpm } => client.send_transport_request(state, bpm),
        Request::Note(event) => client.send_note_event(&NoteEventMsg { source, ..event }),
    };
    match sent {
        Ok(()) => Ok(()),
        // The hub is not reading; the audio path decides when to give up.
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
