//! The instrument hub: where kazoo's instruments plug into the desk.
//!
//! Instruments (kazoo-808, kazoo-mini, kazoo-cs80, kazoo-dx, kazoo-arp) find
//! the hub through the socket and PID file in kazoo-core's discovery module,
//! register, and stream their audio to it. Each one gets the next free strip:
//! its audio is stamped onto the desk's studio clock and handed to the audio
//! callback through the [`patchbay`](crate::patchbay), so it plays through
//! that strip's trim, EQ, fader and pan like any other source.
//!
//! The hub also carries the studio transport: the desk's tempo and play state
//! go to every instrument when they change and when one joins, and an
//! instrument may ask to change them. Notes one instrument sends (an
//! arpeggiator, say) are routed to their target, or to every other instrument
//! when untargeted.
//!
//! Everything here runs on one polling thread. It never touches the audio
//! callback except through the patchbay's lock-free queues and the shared
//! state's atomics.

mod instrument;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use kazoo_core::audio_transport::{AudioRingConfig, audio_block_ring};
use kazoo_core::ipc::discovery;
use kazoo_core::ipc::protocol::{FrameBuffer, MAX_PAYLOAD_SIZE};
use kazoo_core::ipc::types::{
    MSG_NOTE_EVENT, MSG_REFUSED, MSG_REGISTER, MSG_REGISTERED, MSG_SHUTDOWN, MSG_TRANSPORT_SYNC,
    PROTOCOL_VERSION, RegisterMsg, RegisteredMsg, SYNC_NOW, TRANSPORT_PAUSED, TRANSPORT_PLAYING,
    TRANSPORT_RECORDING, TRANSPORT_STOPPED, TransportSyncMsg,
};
use kazoo_core::protocol::BufferId;

use crate::engine::{MAX_SOURCE_RING_SAMPLES, MixerEngineError, short_name};
use crate::patchbay::{PatchCommand, PatchEvent, PatchbayHub};
use crate::shared::{DEFAULT_BPM, DESK_CHANNELS, SharedState, Transport};
use crate::song::SongAnchor;
use crate::studio_clock::{DeskClock, DeskTimer, StudioStamp};
use crate::worker::join_worker;

use instrument::{ConnectionError, Inbound, Instrument, NoteMessage, Registration};
use kazoo_core::ipc::outbox::OutboxError;

/// Time between polls of the sockets.
const POLL_INTERVAL: Duration = Duration::from_micros(500);

/// How long a new connection has to register.
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(2);

/// Messages read from one instrument per poll, so one busy instrument cannot
/// starve the others.
const MESSAGES_PER_POLL: usize = 32;

/// Nominal block size of an instrument's ring. Blocks of any size up to
/// [`instrument::MAX_MESSAGE_FRAMES`] fit; this only sets how many are held.
///
/// Small, so an instrument with a small callback still gets a deep ring: the
/// ring holds [`MAX_SOURCE_RING_SAMPLES`] samples in at most this many
/// frames' worth of blocks (256 stereo blocks).
const RING_BLOCK_FRAMES: u32 = 128;

/// Notices waiting for the desk before new ones are counted as dropped.
const NOTICE_BACKLOG: usize = 64;

/// Patch commands the hub may have awaiting the callback's answer.
pub const PATCH_CAPACITY: usize = 2 * DESK_CHANNELS;

/// How the hub is set up.
#[derive(Debug, Clone)]
pub struct HubConfig {
    /// Socket the hub listens on.
    pub socket: PathBuf,
    /// Write the PID file instruments use to find the hub. Off for a hub on
    /// a private socket (tests).
    pub advertise: bool,
    /// The desk's sample rate; instruments must match it.
    pub sample_rate: u32,
    /// Strips already in use by other sources, which instruments never get.
    pub reserved: [bool; DESK_CHANNELS],
}

impl HubConfig {
    /// A hub on the standard socket, advertised for instruments to find.
    #[must_use]
    pub fn standard(sample_rate: u32, reserved: [bool; DESK_CHANNELS]) -> Self {
        Self {
            socket: discovery::default_socket_path(),
            advertise: true,
            sample_rate,
            reserved,
        }
    }
}

/// Why the hub did not start.
#[derive(Debug)]
pub enum HubStartError {
    /// Another hub is already serving instruments on the socket.
    AnotherHub {
        /// Its process id, when its PID file names this socket.
        pid: Option<u32>,
        /// The socket.
        socket: PathBuf,
    },
    /// The socket, PID file or thread could not be set up.
    Io(io::Error),
}

impl fmt::Display for HubStartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AnotherHub { pid, socket } => {
                write!(f, "another kazoo hub")?;
                if let Some(pid) = pid {
                    write!(f, " (process {pid})")?;
                }
                write!(
                    f,
                    " is already serving instruments on {}; close it first",
                    socket.display()
                )
            }
            Self::Io(err) => write!(f, "could not start the instrument hub: {err}"),
        }
    }
}

impl std::error::Error for HubStartError {}

impl From<io::Error> for HubStartError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

/// Something the desk should tell the engineer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubNotice {
    /// An instrument plugged in.
    Joined {
        /// Desk strip.
        slot: usize,
        /// Instrument name.
        name: String,
    },
    /// An instrument left.
    Left {
        /// Desk strip it had.
        slot: usize,
        /// Instrument name.
        name: String,
        /// Why.
        reason: LeaveReason,
    },
    /// An instrument could not plug in.
    Refused {
        /// Instrument name, if it got as far as saying.
        name: String,
        /// Why.
        reason: RefuseReason,
        /// Whether the instrument was told why.
        told: bool,
    },
    /// The desk's patching did something it never should: a bug.
    Fault(String),
}

/// Why an instrument left the desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaveReason {
    /// It said goodbye.
    Goodbye,
    /// Its connection closed or failed.
    Disconnected(String),
    /// It sent something the hub could not accept.
    Protocol(String),
    /// It stopped reading what the hub sent.
    Stuck,
    /// The desk's engine would not take its audio.
    Engine(MixerEngineError),
}

/// Why an instrument could not plug in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefuseReason {
    /// It runs at a different sample rate from the desk.
    SampleRate {
        /// The instrument's rate.
        instrument: u32,
        /// The desk's rate.
        desk: u32,
    },
    /// It asked for a channel count other than mono or stereo.
    Channels(u8),
    /// Every strip is taken.
    DeskFull,
    /// It did not register properly.
    Handshake(String),
}

impl fmt::Display for HubNotice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Joined { slot, name } => write!(f, "{name} plugged into strip {}", slot + 1),
            Self::Left { slot, name, reason } => {
                write!(f, "{name} left strip {}", slot + 1)?;
                match reason {
                    LeaveReason::Goodbye => Ok(()),
                    LeaveReason::Disconnected(why) => write!(f, " (connection lost: {why})"),
                    LeaveReason::Protocol(why) => write!(f, " (bad message: {why})"),
                    LeaveReason::Stuck => write!(f, " (stopped responding)"),
                    LeaveReason::Engine(err) => write!(f, " (desk refused it: {err:?})"),
                }
            }
            Self::Refused { name, reason, told } => {
                let name = if name.is_empty() {
                    "an instrument"
                } else {
                    name
                };
                write!(f, "{name} could not plug in: {reason}")?;
                if !told {
                    write!(f, " (and could not be told why)")?;
                }
                Ok(())
            }
            Self::Fault(what) => write!(f, "desk fault: {what}"),
        }
    }
}

impl fmt::Display for RefuseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SampleRate { instrument, desk } => write!(
                f,
                "it runs at {instrument} Hz, the desk at {desk} Hz (use the same device)"
            ),
            Self::Channels(n) => write!(f, "{n} channels (mono or stereo only)"),
            Self::DeskFull => write!(f, "every strip is taken"),
            Self::Handshake(why) => write!(f, "{why}"),
        }
    }
}

/// The hub thread is no longer running (it only stops early by panicking).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubStopped;

/// Running counters, for the desk.
#[derive(Debug, Default)]
struct HubStats {
    instruments: AtomicU64,
    blocks: AtomicU64,
    dropped_blocks: AtomicU64,
    resyncs: AtomicU64,
    lost_notices: AtomicU64,
}

/// A copy of the hub's counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubSnapshot {
    /// Instruments plugged in now.
    pub instruments: u64,
    /// Audio blocks received.
    pub blocks: u64,
    /// Audio blocks lost because a strip's ring was full.
    pub dropped_blocks: u64,
    /// Times an instrument's stream was re-placed on the studio clock.
    pub resyncs: u64,
    /// Notices dropped because the desk was not reading them.
    pub lost_notices: u64,
}

/// The running hub. Dropping it disconnects every instrument.
#[derive(Debug)]
pub struct Hub {
    running: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    notices: Receiver<HubNotice>,
    stats: Arc<HubStats>,
    socket: PathBuf,
}

impl Hub {
    /// Start serving instruments.
    ///
    /// # Errors
    ///
    /// [`HubStartError::AnotherHub`] if an advertised hub is already running,
    /// or [`HubStartError::Io`] if the socket, PID file or thread fails.
    pub fn start(
        config: HubConfig,
        shared: Arc<SharedState>,
        patchbay: PatchbayHub,
    ) -> Result<Self, HubStartError> {
        let listener = match discovery::claim_socket(&config.socket) {
            Ok(listener) => listener,
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
                let pid = match discovery::read_pid_file() {
                    Ok(Some(record)) if record.socket == config.socket => Some(record.pid),
                    Ok(_) => None,
                    Err(read_err) => {
                        return Err(HubStartError::Io(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            format!(
                                "another kazoo hub is serving on {}, and the hub PID file \
                                 could not be read: {read_err}",
                                config.socket.display()
                            ),
                        )));
                    }
                };
                return Err(HubStartError::AnotherHub {
                    pid,
                    socket: config.socket,
                });
            }
            Err(err) => return Err(err.into()),
        };
        listener.set_nonblocking(true)?;
        if config.advertise {
            discovery::write_pid_file(&config.socket)?;
        }

        let running = Arc::new(AtomicBool::new(true));
        let stats = Arc::new(HubStats::default());
        let (notice_tx, notices) = sync_channel(NOTICE_BACKLOG);
        let socket = config.socket.clone();
        let mut server = Server::new(listener, config, shared, patchbay, notice_tx, &stats);
        let thread_running = Arc::clone(&running);
        let join = thread::Builder::new()
            .name("kazoo-mix-hub".to_string())
            .spawn(move || server.run(&thread_running))?;

        Ok(Self {
            running,
            join: Some(join),
            notices,
            stats,
            socket,
        })
    }

    /// The next notice for the desk, if any.
    ///
    /// # Errors
    ///
    /// [`HubStopped`] once the hub thread has stopped and every notice it
    /// sent has been read.
    pub fn next_notice(&self) -> Result<Option<HubNotice>, HubStopped> {
        match self.notices.try_recv() {
            Ok(notice) => Ok(Some(notice)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(HubStopped),
        }
    }

    /// Whether the hub thread is still serving. It only stops if it panicked;
    /// dropping the hub then re-raises that panic.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.join.as_ref().is_some_and(|join| !join.is_finished())
    }

    /// The hub's counters.
    #[must_use]
    pub fn snapshot(&self) -> HubSnapshot {
        HubSnapshot {
            instruments: self.stats.instruments.load(Ordering::Relaxed),
            blocks: self.stats.blocks.load(Ordering::Relaxed),
            dropped_blocks: self.stats.dropped_blocks.load(Ordering::Relaxed),
            resyncs: self.stats.resyncs.load(Ordering::Relaxed),
            lost_notices: self.stats.lost_notices.load(Ordering::Relaxed),
        }
    }

    /// The socket instruments connect to.
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(join) = self.join.take() {
            join_worker("instrument hub", join);
        }
    }
}

/// A connection that has not registered yet.
#[derive(Debug)]
struct Pending {
    stream: UnixStream,
    read_buf: FrameBuffer,
    deadline: Instant,
}

/// Who holds a desk strip, as far as the hub knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strip {
    /// Free for an instrument.
    Free,
    /// Held by a source the hub does not manage.
    Reserved,
    /// Held by a connected instrument.
    Busy,
    /// Being unplugged: free once the callback confirms.
    Releasing,
}

/// State owned by the hub thread.
struct Server {
    listener: UnixListener,
    config: HubConfig,
    shared: Arc<SharedState>,
    patchbay: PatchbayHub,
    notices: SyncSender<HubNotice>,
    stats: Arc<HubStats>,
    pending: Vec<Pending>,
    instruments: Vec<Instrument>,
    strips: [Strip; DESK_CHANNELS],
    patches: VecDeque<PatchCommand>,
    /// The song anchor in force and those scheduled after it.
    song: VecDeque<SongAnchor>,
    timer: DeskTimer,
    /// The last refusal told to the desk, so a retrying instrument does not
    /// repeat it over every other notice.
    last_refusal: Option<HubNotice>,
}

impl Server {
    fn new(
        listener: UnixListener,
        config: HubConfig,
        shared: Arc<SharedState>,
        patchbay: PatchbayHub,
        notices: SyncSender<HubNotice>,
        stats: &Arc<HubStats>,
    ) -> Self {
        let strips = config.reserved.map(|reserved| {
            if reserved {
                Strip::Reserved
            } else {
                Strip::Free
            }
        });
        let timer = DeskTimer::new(config.sample_rate);
        Self {
            listener,
            config,
            shared,
            patchbay,
            notices,
            stats: Arc::clone(stats),
            pending: Vec::new(),
            instruments: Vec::with_capacity(DESK_CHANNELS),
            strips,
            patches: VecDeque::with_capacity(PATCH_CAPACITY),
            song: VecDeque::from([SongAnchor::stopped(f64::from(DEFAULT_BPM))]),
            timer,
            last_refusal: None,
        }
    }

    fn run(&mut self, running: &AtomicBool) {
        while running.load(Ordering::Acquire) {
            self.poll_once();
            thread::sleep(POLL_INTERVAL);
        }
        self.shut_down();
    }

    /// One pass over everything the hub serves.
    fn poll_once(&mut self) {
        // Answers first: a strip the desk has emptied is free before anyone
        // new registers.
        self.collect_patch_events();
        self.accept();
        self.register_pending();
        self.read_instruments();
        self.follow_song();
        self.pace_instruments();
        self.flush_instruments();
        self.send_patches();
        self.stats
            .instruments
            .store(self.instruments.len() as u64, Ordering::Relaxed);
    }

    fn clock(&mut self) -> DeskClock {
        self.timer.read(&self.shared)
    }

    fn notify(&mut self, notice: HubNotice) {
        if matches!(notice, HubNotice::Refused { .. }) {
            if self.last_refusal.as_ref() == Some(&notice) {
                return;
            }
            self.last_refusal = Some(notice.clone());
        } else {
            self.last_refusal = None;
        }
        match self.notices.try_send(notice) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.stats.lost_notices.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn accept(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => match stream.set_nonblocking(true) {
                    Ok(()) => self.pending.push(Pending {
                        stream,
                        read_buf: FrameBuffer::new(),
                        deadline: Instant::now() + REGISTRATION_TIMEOUT,
                    }),
                    Err(err) => self.notify(HubNotice::Refused {
                        name: String::new(),
                        reason: RefuseReason::Handshake(format!("socket setup failed: {err}")),
                        told: false,
                    }),
                },
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
                Err(err) => {
                    self.notify(HubNotice::Refused {
                        name: String::new(),
                        reason: RefuseReason::Handshake(format!("accept failed: {err}")),
                        told: false,
                    });
                    return;
                }
            }
        }
    }

    fn register_pending(&mut self) {
        let now = Instant::now();
        let mut index = 0;
        while index < self.pending.len() {
            let pending = &mut self.pending[index];
            match pending.read_buf.try_read_frame(&mut &pending.stream) {
                Ok(None) if now < pending.deadline => index += 1,
                Ok(None) => {
                    let pending = self.pending.swap_remove(index);
                    self.refuse(
                        &pending.stream,
                        String::new(),
                        RefuseReason::Handshake("it connected but never registered".to_string()),
                    );
                }
                Ok(Some(header)) => {
                    let pending = self.pending.swap_remove(index);
                    let len = header.payload_len as usize;
                    if header.msg_type == MSG_REGISTER && len == RegisterMsg::V1_WIRE_SIZE {
                        self.refuse(
                            &pending.stream,
                            String::new(),
                            RefuseReason::Handshake(format!(
                                "it speaks kazoo protocol 1, the desk speaks {PROTOCOL_VERSION}: \
                                 rebuild it"
                            )),
                        );
                        continue;
                    }
                    if header.msg_type == MSG_REGISTER
                        && len > RegisterMsg::WIRE_SIZE
                        && len < RegisterMsg::PACED_WIRE_SIZE
                    {
                        self.refuse(
                            &pending.stream,
                            String::new(),
                            RefuseReason::Handshake(format!(
                                "its registration was {len} bytes: {} or at least {}",
                                RegisterMsg::WIRE_SIZE,
                                RegisterMsg::PACED_WIRE_SIZE
                            )),
                        );
                        continue;
                    }
                    if header.msg_type != MSG_REGISTER || len < RegisterMsg::WIRE_SIZE {
                        self.refuse(
                            &pending.stream,
                            String::new(),
                            RefuseReason::Handshake(format!(
                                "first message was 0x{:02X} with {len} bytes, not a registration",
                                header.msg_type
                            )),
                        );
                        continue;
                    }
                    let register = RegisterMsg::decode(&pending.read_buf.payload()[..len]);
                    self.register(pending, &register);
                }
                // Closed before sending anything: something checking the
                // hub is there (a starting hub, or `hub_listening`), not an
                // instrument.
                Err(err)
                    if err.kind() == io::ErrorKind::UnexpectedEof
                        && !pending.read_buf.read_in_progress() =>
                {
                    self.pending.swap_remove(index);
                }
                Err(err) => {
                    self.pending.swap_remove(index);
                    self.notify(HubNotice::Refused {
                        name: String::new(),
                        reason: RefuseReason::Handshake(format!(
                            "connection failed while registering: {err}"
                        )),
                        told: false,
                    });
                }
            }
        }
    }

    /// Tell a connection why it cannot plug in, and the desk that it could
    /// not. The connection closes when `stream` is dropped.
    fn refuse(&mut self, stream: &UnixStream, name: String, reason: RefuseReason) {
        let text = reason.to_string();
        let mut end = text.len().min(MAX_PAYLOAD_SIZE);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut frame = FrameBuffer::new();
        frame.payload_mut()[..end].copy_from_slice(&text.as_bytes()[..end]);
        // A fresh connection's send buffer is empty, so the short refusal
        // goes out whole or not at all.
        let told = frame
            .write_frame(MSG_REFUSED, 0, end, &mut &*stream)
            .is_ok();
        self.notify(HubNotice::Refused { name, reason, told });
    }

    fn register(&mut self, pending: Pending, register: &RegisterMsg) {
        let name = register.name_str().to_string();
        if register.protocol != PROTOCOL_VERSION {
            let reason = RefuseReason::Handshake(format!(
                "it speaks kazoo protocol {}, the desk speaks {PROTOCOL_VERSION}: rebuild it",
                register.protocol
            ));
            self.refuse(&pending.stream, name, reason);
            return;
        }
        if register.sample_rate != self.config.sample_rate {
            let reason = RefuseReason::SampleRate {
                instrument: register.sample_rate,
                desk: self.config.sample_rate,
            };
            self.refuse(&pending.stream, name, reason);
            return;
        }
        let channels = match register.channel_count {
            1 => 1_u16,
            2 => 2,
            other => {
                self.refuse(&pending.stream, name, RefuseReason::Channels(other));
                return;
            }
        };
        let Some(slot) = self.strips.iter().position(|strip| *strip == Strip::Free) else {
            self.refuse(&pending.stream, name, RefuseReason::DeskFull);
            return;
        };

        let (producer, consumer) = audio_block_ring(strip_ring(slot, channels));

        let clock = self.clock();
        let transport = self.shared.transport();
        let registered = RegisteredMsg {
            strip_index: u8::try_from(slot).unwrap_or(u8::MAX),
            hub_sample_rate: self.config.sample_rate,
            hub_buffer_size: clock.device_frames,
            transport_state: transport_state(transport),
            bpm: transport.bpm,
            position: clock.studio_now,
        };
        let mut payload = [0_u8; RegisteredMsg::WIRE_SIZE];
        registered.encode(&mut payload);

        let mut instrument = Instrument::new(
            pending.stream,
            Registration {
                id: register.instrument_id,
                name: name.clone(),
                slot,
                channels,
                // At most a second of extra lead: more is a mistake, and
                // would hold every transport change back as long.
                pace_lead_frames: register.pace_lead_frames.min(self.config.sample_rate),
            },
            pending.read_buf,
            producer,
        );
        let greeted = instrument
            .send(MSG_REGISTERED, &payload)
            .and_then(|()| instrument.flush());
        if let Err(err) = greeted {
            self.notify(HubNotice::Refused {
                name,
                reason: RefuseReason::Handshake(format!(
                    "could not answer its registration: {}",
                    describe_outbox_error(&err)
                )),
                told: false,
            });
            return;
        }

        self.strips[slot] = Strip::Busy;
        self.patches.push_back(PatchCommand::Attach {
            slot,
            name: short_name(display_name(&name)),
            consumer,
        });
        self.instruments.push(instrument);
        self.notify(HubNotice::Joined { slot, name });
    }

    fn read_instruments(&mut self) {
        let clock = self.clock();
        let mut notes: Vec<([u8; 16], NoteMessage)> = Vec::new();
        let mut leaving: Vec<(usize, LeaveReason)> = Vec::new();
        for (index, instrument) in self.instruments.iter_mut().enumerate() {
            for _ in 0..MESSAGES_PER_POLL {
                match instrument.poll(clock) {
                    Ok(None) => break,
                    Ok(Some(Inbound::Audio { resynced })) => {
                        self.stats.blocks.fetch_add(1, Ordering::Relaxed);
                        if resynced {
                            self.stats.resyncs.fetch_add(1, Ordering::Relaxed);
                            // Its stream moved on the studio clock: where the
                            // song is, in its frames, moved with it.
                            instrument.needs_song = true;
                        }
                    }
                    Ok(Some(Inbound::AudioDropped)) => {
                        self.stats.dropped_blocks.fetch_add(1, Ordering::Relaxed);
                    }
                    Ok(Some(Inbound::Note(note))) => notes.push((instrument.id, note)),
                    Ok(Some(Inbound::Transport { state, bpm })) => {
                        apply_transport_request(&self.shared, state, bpm);
                    }
                    Ok(Some(Inbound::Goodbye)) => {
                        leaving.push((index, LeaveReason::Goodbye));
                        break;
                    }
                    Err(ConnectionError::Io(err)) => {
                        leaving.push((index, LeaveReason::Disconnected(err.to_string())));
                        break;
                    }
                    Err(ConnectionError::Protocol(err)) => {
                        leaving.push((index, LeaveReason::Protocol(err.to_string())));
                        break;
                    }
                }
            }
        }
        for (source, note) in notes {
            self.route_note(source, &note, &mut leaving);
        }
        self.remove(leaving);
    }

    fn route_note(
        &mut self,
        source: [u8; 16],
        note: &NoteMessage,
        leaving: &mut Vec<(usize, LeaveReason)>,
    ) {
        let broadcast = note.target == [0; 16];
        for (index, instrument) in self.instruments.iter_mut().enumerate() {
            let wanted = if broadcast {
                instrument.id != source
            } else {
                instrument.id == note.target
            };
            if wanted {
                if let Err(err) = instrument.send(MSG_NOTE_EVENT, &note.payload) {
                    leaving.push((index, leave_reason(&err)));
                }
            }
        }
    }

    /// Pass every transport change the callback scheduled to every
    /// instrument, as the frame of its own stream the change lands on, and
    /// tell an instrument newly placed (or re-placed) on the studio clock
    /// where the song is. Then tell the callback how far ahead to schedule,
    /// so the slowest instrument still hears of a change in time.
    fn follow_song(&mut self) {
        let clock = self.clock();
        let mut fresh = 0;
        while let Some(anchor) = self.patchbay.next_song() {
            self.song.push_back(anchor);
            fresh += 1;
        }
        // Keep the anchor in force and the ones still to land.
        while self.song.len() > 1 && self.song[1].frame <= clock.studio_now {
            self.song.pop_front();
        }
        let fresh = fresh.min(self.song.len());
        let sample_rate = self.config.sample_rate;
        let mut leaving = Vec::new();
        for (index, instrument) in self.instruments.iter_mut().enumerate() {
            let catch_up = instrument.needs_song && instrument.stamp().next_frame().is_some();
            let skip = if catch_up {
                instrument.needs_song = false;
                0
            } else {
                self.song.len() - fresh
            };
            for anchor in self.song.iter().skip(skip) {
                let sync = song_sync(anchor, instrument.stamp(), sample_rate);
                let mut payload = [0_u8; TransportSyncMsg::WIRE_SIZE];
                sync.encode(&mut payload);
                if let Err(err) = instrument.send(MSG_TRANSPORT_SYNC, &payload) {
                    leaving.push((index, leave_reason(&err)));
                    break;
                }
            }
        }
        self.remove(leaving);

        let need = self
            .instruments
            .iter()
            .map(|instrument| instrument.stamp().schedule_need(clock))
            .max()
            .unwrap_or(0);
        self.shared
            .set_schedule_ahead(u32::try_from(need).unwrap_or(u32::MAX));
    }

    /// Tell every instrument that renders on a timer where the desk is
    /// playing its stream.
    fn pace_instruments(&mut self) {
        let clock = self.clock();
        let now = Instant::now();
        let mut leaving = Vec::new();
        for (index, instrument) in self.instruments.iter_mut().enumerate() {
            if let Err(err) = instrument.pace(clock, now) {
                leaving.push((index, leave_reason(&err)));
            }
        }
        self.remove(leaving);
    }

    fn flush_instruments(&mut self) {
        let mut leaving = Vec::new();
        for (index, instrument) in self.instruments.iter_mut().enumerate() {
            if let Err(err) = instrument.flush() {
                leaving.push((index, leave_reason(&err)));
            }
        }
        self.remove(leaving);
    }

    /// Disconnect instruments, highest index first so the rest stay valid.
    fn remove(&mut self, mut leaving: Vec<(usize, LeaveReason)>) {
        leaving.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
        leaving.dedup_by_key(|(index, _)| *index);
        for (index, reason) in leaving {
            let instrument = self.instruments.swap_remove(index);
            self.strips[instrument.slot] = Strip::Releasing;
            self.patches.push_back(PatchCommand::Detach {
                slot: instrument.slot,
            });
            self.notify(HubNotice::Left {
                slot: instrument.slot,
                name: instrument.name,
                reason,
            });
        }
    }

    fn collect_patch_events(&mut self) {
        while let Some(event) = self.patchbay.next_event() {
            match event {
                PatchEvent::Attached { previous: None, .. } => {}
                // The hub only plugs into strips it knows are empty. What
                // the strip held is dropped here, off the audio thread.
                PatchEvent::Attached {
                    slot,
                    previous: Some(_),
                } => self.notify(HubNotice::Fault(format!(
                    "strip {} still had a source when an instrument was plugged in",
                    slot + 1
                ))),
                PatchEvent::Refused { slot, error, .. } => {
                    self.strips[slot] = Strip::Free;
                    if let Some(index) = self.instruments.iter().position(|i| i.slot == slot) {
                        let instrument = self.instruments.swap_remove(index);
                        self.notify(HubNotice::Left {
                            slot,
                            name: instrument.name,
                            reason: LeaveReason::Engine(error),
                        });
                    }
                }
                PatchEvent::Detached { slot, consumer } => {
                    if let Err(err) = consumer {
                        self.notify(HubNotice::Fault(format!(
                            "unplugging strip {} failed: {err:?}",
                            slot + 1
                        )));
                    }
                    if self.strips.get(slot) == Some(&Strip::Releasing) {
                        self.strips[slot] = Strip::Free;
                    }
                }
            }
        }
    }

    fn send_patches(&mut self) {
        while let Some(command) = self.patches.pop_front() {
            if let Err(command) = self.patchbay.send(command) {
                // Answers are outstanding; try again next poll.
                self.patches.push_front(command);
                return;
            }
        }
    }

    /// Tell every instrument the hub is closing, and give the messages a
    /// moment to go out.
    fn shut_down(&mut self) {
        // An instrument that cannot take the goodbye is not waited for: its
        // connection closing below tells it the same thing.
        let told: Vec<bool> = self
            .instruments
            .iter_mut()
            .map(|instrument| instrument.send(MSG_SHUTDOWN, &[]).is_ok())
            .collect();
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline
            && self
                .instruments
                .iter_mut()
                .zip(&told)
                .any(|(instrument, told)| {
                    *told && instrument.flush().is_ok() && instrument.outbox.waiting() > 0
                })
        {
            thread::sleep(POLL_INTERVAL);
        }
        self.instruments.clear();
        if let Err(err) = std::fs::remove_file(&self.config.socket) {
            if err.kind() != io::ErrorKind::NotFound {
                eprintln!(
                    "kazoo-mix: could not remove the hub socket {}: {err}",
                    self.config.socket.display()
                );
            }
        }
        if self.config.advertise {
            if let Err(err) = discovery::remove_own_pid_file() {
                eprintln!("kazoo-mix: could not remove the hub PID file: {err}");
            }
        }
    }
}

const fn transport_state(transport: Transport) -> u8 {
    if transport.playing {
        TRANSPORT_PLAYING
    } else {
        TRANSPORT_STOPPED
    }
}

/// The ring an instrument's audio reaches strip `slot` through: as many
/// [`RING_BLOCK_FRAMES`] blocks as fit the strip's limit.
pub(crate) fn strip_ring(slot: usize, channels: u16) -> AudioRingConfig {
    let ring_samples = RING_BLOCK_FRAMES as usize * usize::from(channels.max(1));
    let blocks = u32::try_from(MAX_SOURCE_RING_SAMPLES / ring_samples).unwrap_or(u32::MAX);
    let buffer_id = BufferId(u32::try_from(slot).unwrap_or(u32::MAX));
    AudioRingConfig::new(buffer_id, channels, RING_BLOCK_FRAMES, blocks)
}

/// A song anchor as a sync for one instrument: on the frame of its stream
/// where the anchor lands (or its next frame, if that is later), or "at
/// once, position to follow" while its stream is not yet on the clock.
fn song_sync(anchor: &SongAnchor, stamp: &StudioStamp, sample_rate: u32) -> TransportSyncMsg {
    let state = if anchor.playing {
        TRANSPORT_PLAYING
    } else {
        TRANSPORT_STOPPED
    };
    // Tempos are 20-300 BPM: f32 holds them exactly enough.
    let bpm = anchor.bpm as f32;
    let placed = stamp.next_frame().and_then(|next| {
        let at = anchor.frame.max(next);
        stamp.stream_at(at).map(|stream| (at, stream))
    });
    match placed {
        Some((at, stream)) => TransportSyncMsg {
            state,
            bpm,
            at_frame: stream,
            beat: anchor.beat_at(at, f64::from(sample_rate)),
        },
        None => TransportSyncMsg {
            state,
            bpm,
            at_frame: SYNC_NOW,
            beat: f64::NAN,
        },
    }
}

fn apply_transport_request(shared: &SharedState, state: u8, bpm: Option<f32>) {
    if let Some(bpm) = bpm {
        shared.set_tempo(bpm);
    }
    match state {
        TRANSPORT_PLAYING | TRANSPORT_RECORDING => shared.set_playing(true),
        TRANSPORT_STOPPED | TRANSPORT_PAUSED => shared.set_playing(false),
        // A tempo-only request, or a state this desk does not know: the
        // play state stays as it is and the tempo above still applies.
        _ => {}
    }
}

fn leave_reason(err: &OutboxError) -> LeaveReason {
    match err {
        OutboxError::Stuck => LeaveReason::Stuck,
        OutboxError::TooLarge => LeaveReason::Protocol("oversized message".to_string()),
        OutboxError::Io(err) => LeaveReason::Disconnected(err.to_string()),
    }
}

fn describe_outbox_error(err: &OutboxError) -> String {
    match err {
        OutboxError::Stuck => "it is not reading".to_string(),
        OutboxError::TooLarge => "message too large".to_string(),
        OutboxError::Io(err) => err.to_string(),
    }
}

/// The name shown on a strip: the instrument's name without the shared
/// `kazoo-` prefix.
fn display_name(name: &str) -> &str {
    name.strip_prefix("kazoo-").unwrap_or(name)
}
