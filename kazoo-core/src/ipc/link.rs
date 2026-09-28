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
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};

use super::client::{HubIpcClient, HubMessage};
use super::discovery;
use super::outbox::OutboxError;
use super::types::{
    DeskPaceMsg, NOTE_CC, NOTE_OFF, NOTE_ON, NOTE_PITCH_BEND, NoteEventMsg, RegisterMsg, SYNC_NOW,
    TRANSPORT_PAUSED, TRANSPORT_PLAYING, TRANSPORT_RECORDING, TRANSPORT_STOPPED,
    TRANSPORT_UNCHANGED, TransportSyncMsg,
};
use crate::audio_transport::{
    AudioBlock, AudioBlockConsumer, AudioBlockProducer, AudioRingConfig, AudioRingPopError,
    AudioRingPushError, audio_block_ring,
};
use crate::protocol::{AudioBlockHeader, BlockFlags, BufferId};

/// How often the link looks for a hub while none is answering.
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

/// Longest wait between attempts after the desk refuses the instrument.
const MAX_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// How long the hub may stop reading before the link gives up on it.
const STUCK_LIMIT: Duration = Duration::from_millis(500);

/// Link thread poll interval while connected.
const POLL_INTERVAL: Duration = Duration::from_micros(500);

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
    };
    let thread_running = Arc::clone(&running);
    let join = thread::Builder::new()
        .name("kazoo-hub-link".to_string())
        .spawn(move || worker.run(&thread_running))?;
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
            Ok(()) => Ok(()),
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
            Ok(()) => {}
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
            .map_err(|_| RequestError::Full)
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
}

impl LinkWorker {
    fn run(&mut self, running: &AtomicBool) {
        let mut next_attempt = Instant::now();
        while running.load(Ordering::Acquire) {
            if self.client.is_none() {
                self.discard_backlog();
                if Instant::now() >= next_attempt {
                    next_attempt = Instant::now() + self.try_connect();
                }
                thread::sleep(IDLE_INTERVAL);
                continue;
            }
            if let Err(why) = self.serve() {
                self.disconnect(why);
                next_attempt = Instant::now() + RETRY_INTERVAL;
            }
            thread::sleep(POLL_INTERVAL);
        }
        self.say_goodbye();
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
        if self.pending.take().is_some() {
            self.state.blocks_dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.stuck_since = None;
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
            thread::sleep(POLL_INTERVAL);
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
mod tests {
    use super::*;
    use crate::ipc::protocol::AUDIO_PAYLOAD_HEADER;
    use crate::ipc::protocol::FrameBuffer;
    use crate::ipc::types::{
        MSG_AUDIO, MSG_DESK_PACE, MSG_NOTE_EVENT, MSG_REFUSED, MSG_REGISTER, MSG_REGISTERED,
        MSG_SHUTDOWN, MSG_TRANSPORT_REQUEST, MSG_TRANSPORT_SYNC, RegisterMsg, RegisteredMsg,
        TransportRequestMsg,
    };
    use std::os::unix::net::{UnixListener, UnixStream};

    fn socket() -> PathBuf {
        use std::sync::atomic::AtomicU32;
        static NEXT: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir().join(format!(
            "kzl-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn wait(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// Accept one instrument on `listener` and answer its registration.
    fn accept(listener: &UnixListener) -> (UnixStream, FrameBuffer) {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = FrameBuffer::new();
        let header = buf.read_frame(&mut stream).unwrap();
        assert_eq!(header.msg_type, MSG_REGISTER);
        let register = RegisterMsg::decode(&buf.payload()[..header.payload_len as usize]);
        assert_eq!(register.name_str(), "kazoo-test");
        let reply = RegisteredMsg {
            strip_index: 3,
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
    }

    fn config(path: &std::path::Path) -> LinkConfig {
        LinkConfig {
            address: HubAddress::Socket(path.to_path_buf()),
            ..LinkConfig::new("kazoo-test", 2, 48_000, 256)
        }
    }

    #[test]
    fn audio_is_local_until_the_hub_appears_then_goes_to_the_desk() {
        let path = socket();
        let (link, mut audio) = hub_link(config(&path)).unwrap();
        let block = [0.5_f32; 512];
        // No hub yet: the instrument keeps its own output.
        assert!(!audio.send_audio(256, &block));

        let listener = UnixListener::bind(&path).unwrap();
        let (mut stream, mut buf) = accept(&listener);
        wait("the link to connect", || link.is_connected());
        assert_eq!(link.status().strip, Some(3));

        assert!(audio.send_audio(256, &block));
        let header = buf.read_frame(&mut stream).unwrap();
        assert_eq!(header.msg_type, MSG_AUDIO);
        assert_eq!(header.payload_len as usize, AUDIO_PAYLOAD_HEADER + 512 * 4);
        wait("the send to be counted", || link.status().blocks_sent == 1);
        drop(link);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn hub_messages_reach_the_callback_and_a_closing_hub_is_noticed() {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        let (link, mut audio) = hub_link(config(&path)).unwrap();
        let (mut stream, mut buf) = accept(&listener);
        wait("the link to connect", || link.is_connected());

        // Joining hands the callback the desk's transport straight away.
        let joined = audio.try_recv();
        assert!(
            matches!(joined, Some(HubMessage::TransportSync(s)) if s.state == 0 && s.bpm.to_bits() == 120.0_f32.to_bits() && s.at_frame == SYNC_NOW),
            "{joined:?}"
        );

        let sync = TransportSyncMsg {
            state: 1,
            bpm: 90.0,
            at_frame: 4_800,
            beat: 2.0,
        };
        sync.encode(buf.payload_mut());
        buf.write_frame(
            MSG_TRANSPORT_SYNC,
            1,
            TransportSyncMsg::WIRE_SIZE,
            &mut stream,
        )
        .unwrap();
        let mut received = None;
        wait("the sync to arrive", || {
            received = audio.try_recv();
            received.is_some()
        });
        assert!(matches!(received, Some(HubMessage::TransportSync(s)) if s.state == 1));

        buf.write_frame(MSG_SHUTDOWN, 2, 0, &mut stream).unwrap();
        wait("the link to notice the hub closed", || !link.is_connected());
        assert!(!audio.send_audio(256, &[0.0; 512]));
        assert!(
            link.status()
                .last_refusal
                .is_some_and(|why| why.contains("closed"))
        );
        drop(link);
        std::fs::remove_file(&path).unwrap();
    }

    /// Tell the link where the desk is playing its stream.
    fn pace(stream: &mut UnixStream, buf: &mut FrameBuffer, playing_stream: u64, lead: u32) {
        let pace = DeskPaceMsg {
            playing_stream,
            lead_frames: lead,
        };
        pace.encode(buf.payload_mut());
        buf.write_frame(MSG_DESK_PACE, 0, DeskPaceMsg::WIRE_SIZE, stream)
            .unwrap();
    }

    #[test]
    fn a_paced_link_asks_for_its_lead_and_owes_the_desk_what_it_is_behind() {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        let (link, mut audio) = hub_link(LinkConfig {
            pace_lead_frames: 1_920,
            ..config(&path)
        })
        .unwrap();
        // Not plugged in: nothing is owed, and the instrument keeps time.
        assert_eq!(audio.desk_owes(Instant::now()), None);

        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = FrameBuffer::new();
        let header = buf.read_frame(&mut stream).unwrap();
        let register = RegisterMsg::decode(&buf.payload()[..header.payload_len as usize]);
        assert_eq!(register.pace_lead_frames, 1_920);
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
        wait("the link to connect", || link.is_connected());
        // Plugged in, but the desk has not paced it yet.
        assert_eq!(audio.desk_owes(Instant::now()), None);

        // 1 024 frames rendered; the desk plays frame 100 of them with a
        // lead of 2 000: it wants up to 2 100, so 1 076 are owed.
        assert!(audio.send_audio(1_024, &[0.0; 2_048]));
        pace(&mut stream, &mut buf, 100, 2_000);
        let mut owed = None;
        wait("the pace to arrive", || {
            owed = audio.desk_owes(Instant::now()).map(|owes| owes.frames);
            owed.is_some()
        });
        let arrived = Instant::now();
        let now_owed = audio.desk_owes(arrived).unwrap().frames;
        // Plus the frames the desk played since the pace came (well under
        // 50 ms of them, even on a busy machine).
        assert!((1_076..1_076 + 2_400).contains(&owed.unwrap()), "{owed:?}");
        assert!(now_owed >= owed.unwrap(), "{now_owed} after {owed:?}");
        // The desk's clock runs on between paces: 10 ms later, 480 more.
        let later = audio
            .desk_owes(arrived + Duration::from_millis(10))
            .unwrap()
            .frames;
        assert!(
            (later - now_owed).abs_diff(480) <= 1,
            "{later} after {now_owed}"
        );
        // Ahead of the desk: owed goes negative.
        assert!(audio.send_audio(4_096, &vec![0.0; 8_192]));
        let ahead = audio.desk_owes(arrived).unwrap().frames;
        assert_eq!(ahead, now_owed - 4_096);
        let reckoning = audio.desk_owes(arrived).unwrap();
        assert_eq!(reckoning.lead_frames, 2_000);
        assert!(reckoning.paced_at <= arrived);
        // A pace gone stale is not followed.
        assert_eq!(
            audio.desk_owes(arrived + PACE_STALE + Duration::from_millis(50)),
            None
        );

        // Unplugged: forgotten.
        buf.write_frame(MSG_SHUTDOWN, 1, 0, &mut stream).unwrap();
        wait("the link to notice the hub closed", || !link.is_connected());
        assert_eq!(audio.desk_owes(Instant::now()), None);
        drop(link);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn what_is_owed_is_a_signed_distance_across_the_wrap() {
        // The desk plays 1 000 frames before the stream's frame 0, with a
        // lead of 2 000: it wants up to frame 1 000 (plus 48 a millisecond
        // on). Frames wrap, and the distance stays signed either side.
        let pace = Pace {
            playing_stream: 0_u64.wrapping_sub(1_000),
            lead_frames: 2_000,
            arrived: Duration::ZERO,
        };
        assert_eq!(owed(pace, Duration::ZERO, 48_000, 0), 1_000);
        assert_eq!(owed(pace, Duration::ZERO, 48_000, 1_256), -256);
        assert_eq!(owed(pace, Duration::from_millis(1), 48_000, 1_000), 48);
        // Near the top of the counter and past it.
        let pace = Pace {
            playing_stream: u64::MAX - 10,
            lead_frames: 100,
            arrived: Duration::ZERO,
        };
        assert_eq!(owed(pace, Duration::ZERO, 48_000, u64::MAX - 10), 100);
        assert_eq!(owed(pace, Duration::ZERO, 48_000, 200), -111);
    }

    #[test]
    fn a_pace_or_an_unknown_message_does_not_hold_up_the_next() {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        let (link, mut audio) = hub_link(config(&path)).unwrap();
        let (mut stream, mut buf) = accept(&listener);
        wait("the link to connect", || link.is_connected());
        wait("the joining sync", || audio.try_recv().is_some());
        pace(&mut stream, &mut buf, 0, 1_000);
        buf.write_frame(0x7E, 1, 0, &mut stream).unwrap();
        let sync = TransportSyncMsg {
            state: 1,
            bpm: 100.0,
            at_frame: SYNC_NOW,
            beat: 0.0,
        };
        sync.encode(buf.payload_mut());
        buf.write_frame(
            MSG_TRANSPORT_SYNC,
            2,
            TransportSyncMsg::WIRE_SIZE,
            &mut stream,
        )
        .unwrap();
        let mut received = None;
        wait("the sync behind them", || {
            received = audio.try_recv();
            received.is_some()
        });
        assert!(matches!(received, Some(HubMessage::TransportSync(s)) if s.state == 1));
        drop(link);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn dropping_the_link_says_goodbye() {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        let (link, _audio) = hub_link(config(&path)).unwrap();
        let (mut stream, mut buf) = accept(&listener);
        wait("the link to connect", || link.is_connected());
        drop(link);
        let header = buf.read_frame(&mut stream).unwrap();
        assert_eq!(header.msg_type, MSG_SHUTDOWN);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn oversized_callbacks_are_split_and_malformed_ones_dropped() {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        let (link, mut audio) = hub_link(config(&path)).unwrap();
        let (mut stream, mut buf) = accept(&listener);
        wait("the link to connect", || link.is_connected());

        // 600 frames through a 256-frame link: 256, 256, 88.
        assert!(audio.send_audio(600, &[0.25; 1200]));
        for frames in [256, 256, 88] {
            let header = buf.read_frame(&mut stream).unwrap();
            assert_eq!(header.msg_type, MSG_AUDIO);
            assert_eq!(
                header.payload_len as usize,
                AUDIO_PAYLOAD_HEADER + frames * 2 * 4
            );
        }
        // Three frames of stereo need six samples, not five.
        assert!(audio.send_audio(3, &[0.0; 5]));
        wait("the counters", || {
            let status = link.status();
            status.blocks_sent == 3 && status.blocks_dropped == 1
        });
        drop(link);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn requests_from_either_half_reach_the_desk() {
        let path = socket();
        let (link, mut audio) = hub_link(config(&path)).unwrap();
        // Not plugged in: the instrument is told to act on its own.
        assert_eq!(
            audio.request_transport(TRANSPORT_PLAYING, None),
            Err(RequestError::NotConnected)
        );
        assert_eq!(
            link.request_transport(TRANSPORT_PLAYING, None),
            Err(RequestError::NotConnected)
        );

        let listener = UnixListener::bind(&path).unwrap();
        let (mut stream, mut buf) = accept(&listener);
        wait("the link to connect", || link.is_connected());
        assert_eq!(link.request_transport(9, None), Err(RequestError::Invalid));
        assert_eq!(
            audio.request_transport(TRANSPORT_PLAYING, Some(f32::NAN)),
            Err(RequestError::Invalid)
        );

        audio
            .request_transport(TRANSPORT_PLAYING, Some(96.0))
            .unwrap();
        let header = buf.read_frame(&mut stream).unwrap();
        assert_eq!(header.msg_type, MSG_TRANSPORT_REQUEST);
        let request = TransportRequestMsg::decode(buf.payload());
        assert_eq!(request.requested_state, TRANSPORT_PLAYING);
        assert_eq!(request.has_bpm, 1);
        assert_eq!(request.requested_bpm.to_bits(), 96.0_f32.to_bits());

        // A tempo change leaves the desk's play state alone; leaving it
        // alone with no tempo to set is not a request at all.
        assert_eq!(
            link.request_transport(TRANSPORT_UNCHANGED, None),
            Err(RequestError::Invalid)
        );
        assert_eq!(audio.request_tempo(-3.0), Err(RequestError::Invalid));
        link.request_tempo(132.0).unwrap();
        let header = buf.read_frame(&mut stream).unwrap();
        assert_eq!(header.msg_type, MSG_TRANSPORT_REQUEST);
        let request = TransportRequestMsg::decode(buf.payload());
        assert_eq!(request.requested_state, TRANSPORT_UNCHANGED);
        assert_eq!(request.has_bpm, 1);
        assert_eq!(request.requested_bpm.to_bits(), 132.0_f32.to_bits());

        let note = NoteEventMsg {
            source: [0; 16],
            target: [0; 16],
            event_type: NOTE_ON,
            channel: 0,
            note: 60,
            velocity: 100,
        };
        assert_eq!(
            link.send_note(NoteEventMsg { note: 200, ..note }),
            Err(RequestError::Invalid)
        );
        link.send_note(note).unwrap();
        let header = buf.read_frame(&mut stream).unwrap();
        assert_eq!(header.msg_type, MSG_NOTE_EVENT);
        let sent = NoteEventMsg::decode(buf.payload());
        assert_eq!(sent.note, 60);
        assert_ne!(sent.source, [0; 16], "the link signs notes with its id");
        drop(link);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_refusal_is_reported_and_retries_back_off() {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        let (link, _audio) = hub_link(config(&path)).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = FrameBuffer::new();
        buf.read_frame(&mut stream).unwrap();
        let reason = b"every strip is taken";
        buf.payload_mut()[..reason.len()].copy_from_slice(reason);
        buf.write_frame(MSG_REFUSED, 0, reason.len(), &mut stream)
            .unwrap();
        drop(stream);
        wait("the refusal to be reported", || {
            link.status()
                .last_refusal
                .is_some_and(|why| why.contains("every strip is taken"))
        });
        assert!(!link.is_connected());

        // The next try waits two seconds, not the usual one.
        listener.set_nonblocking(true).unwrap();
        let refused_at = Instant::now();
        let retried_at = loop {
            match listener.accept() {
                Ok(_) => break Instant::now(),
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    assert!(refused_at.elapsed() < Duration::from_secs(5), "no retry");
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("accept failed: {err}"),
            }
        };
        assert!(
            retried_at.duration_since(refused_at) > Duration::from_millis(1_500),
            "retried after {:?}",
            retried_at.duration_since(refused_at)
        );
        drop(link);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_desk_that_stops_reading_is_unplugged() {
        let path = socket();
        let listener = UnixListener::bind(&path).unwrap();
        let (link, mut audio) = hub_link(config(&path)).unwrap();
        // Registered, then never read from again.
        let (_stream, _buf) = accept(&listener);
        wait("the link to connect", || link.is_connected());
        let block = [0.1_f32; 512];
        let deadline = Instant::now() + Duration::from_secs(10);
        while link.is_connected() {
            assert!(Instant::now() < deadline, "the link never gave up");
            audio.send_audio(256, &block);
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            link.status()
                .last_refusal
                .is_some_and(|why| why.contains("stopped taking audio")),
            "{:?}",
            link.status()
        );
        drop(link);
        std::fs::remove_file(&path).unwrap();
    }
}
