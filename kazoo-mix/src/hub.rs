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
//! Everything here runs on one thread that sleeps in `poll(2)` on its sockets
//! and wakes when bytes arrive, or every [`HUB_TICK`] for what has no socket.
//! It never touches the audio
//! callback except through the patchbay's lock-free queues and the shared
//! state's atomics. The thread stamps each block onto the studio clock as it
//! reads it, so a block it reads late is placed afresh (heard as a jump):
//! it asks to be scheduled as real-time for its whole life (see
//! [`kazoo_core::realtime`]), and [`Hub::real_time`] says whether it was.
//! A poll allocates nothing; an instrument joining, leaving or being
//! refused does (its ring, its name, the notice), which is rare and brief.
//! Each instrument is told how the desk has been taking its audio (see
//! [`kazoo_core::ipc::types::DeskHealthMsg`]), so a jump can be seen at the
//! instrument as well as heard.

mod instrument;
mod network;
mod refusals;
pub(crate) mod tempo;
use tempo::initial_song;
#[cfg(test)]
pub(crate) mod tests;

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::io::Errno;

use kazoo_core::audio_transport::{AudioRingConfig, audio_block_ring};
use kazoo_core::ipc::discovery;
use kazoo_core::ipc::protocol::{FrameBuffer, FrameHeader, HEADER_SIZE, encode_header};
use kazoo_core::ipc::stream::HubStream;
use kazoo_core::ipc::types::{
    MSG_NOTE_EVENT, MSG_REFUSED, MSG_REGISTER, MSG_REGISTERED, MSG_SHUTDOWN, MSG_TRANSPORT_SYNC,
    PROTOCOL_VERSION, RegisterMsg, RegisteredMsg, SYNC_NOW, TRANSPORT_PAUSED, TRANSPORT_PLAYING,
    TRANSPORT_RECORDING, TRANSPORT_STOPPED, TransportSyncMsg,
};
use kazoo_core::protocol::BufferId;
use kazoo_core::realtime::{NotRealTime, RealTime};

use crate::engine::{MAX_SOURCE_RING_SAMPLES, MixerEngineError, short_name};
use crate::patchbay::{PatchCommand, PatchEvent, PatchbayHub};
use crate::shared::{DESK_CHANNELS, SharedState, Transport};
use crate::song::SongAnchor;
use crate::studio_clock::{DeskClock, DeskTimer, StudioStamp};
use crate::worker::join_worker;

use instrument::{ConnectionError, Inbound, Instrument, NoteMessage, Registration};
use kazoo_core::ipc::outbox::OutboxError;

/// Longest wait for work without socket readiness.
///
/// Patchbay answers, song anchors, shared transport, pace and health are served
/// every two milliseconds: a tenth of the studio scheduling slack. This keeps
/// anchors inside it, with four times fewer idle wakes than the old interval.
/// Also the real-time scheduling period.
const HUB_TICK: Duration = Duration::from_millis(2);

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

/// Most reason text sent to a connection that cannot register.
const REFUSAL_BYTES: usize = 512;

/// Patch commands the hub may have awaiting the callback's answer.
pub const PATCH_CAPACITY: usize = 2 * DESK_CHANNELS;

/// How the hub is set up.
#[derive(Debug, Clone)]
pub struct HubConfig {
    /// Socket the hub listens on.
    pub socket: PathBuf,
    /// Optional network listener; absent by default.
    pub listen: Option<SocketAddr>,
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
            listen: None,
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
    /// An instrument plays into the desk over the network, and the desk
    /// holds its audio back this long to ride out the network's delays:
    /// said when it plugs in, and each time it came late and that grew.
    NetworkLead {
        /// Desk strip.
        slot: usize,
        /// Instrument name.
        name: String,
        /// The lead for the network, beyond the usual.
        lead: Duration,
    },
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
    /// Its block cannot fit twice in the strip ring.
    BlockTooLarge {
        /// Registered frames per block.
        frames: u32,
        /// Most frames that fit twice at its channel count.
        most: u32,
        /// Its channel count, which sets `most`.
        channels: u16,
    },
    /// Every strip is taken.
    DeskFull,
    /// It did not register properly.
    Handshake(String),
}

impl fmt::Display for HubNotice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Joined { slot, name } => write!(f, "{name} plugged into strip {}", slot + 1),
            Self::NetworkLead { slot, name, lead } => write!(
                f,
                "{name} on strip {} plays over the network: the desk holds it {} ms behind to ride out the network's delays",
                slot + 1,
                lead.as_millis()
            ),
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
            Self::SampleRate { instrument, desk } => {
                f.write_str(&kazoo_core::ipc::link::rate_refusal(*instrument, *desk))
            }
            Self::Channels(n) => write!(f, "{n} channels (mono or stereo only)"),
            Self::BlockTooLarge {
                frames,
                most,
                channels,
            } => write!(
                f,
                "blocks of {frames} frames (at most {most} at {channels} channels)"
            ),
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
    refusals: refusals::Refusals,
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
    listen: Option<SocketAddr>,
    /// Whether the hub thread is scheduled as real-time, once it has asked.
    real_time: Arc<OnceLock<Result<(), NotRealTime>>>,
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
            Err(err) => {
                return Err(HubStartError::Io(io::Error::new(
                    err.kind(),
                    format!(
                        "cannot claim the desk's socket {}: {err}",
                        config.socket.display()
                    ),
                )));
            }
        };
        listener.set_nonblocking(true)?;
        let (network, listen) = network::bind_claimed(config.listen, &config.socket)?;
        if config.advertise {
            discovery::write_pid_file(&config.socket)?;
        }

        let running = Arc::new(AtomicBool::new(true));
        let stats = Arc::new(HubStats::default());
        let (notice_tx, notices) = sync_channel(NOTICE_BACKLOG);
        let socket = config.socket.clone();
        let mut server = Server::new(
            listener, network, config, shared, patchbay, notice_tx, &stats,
        );
        let thread_running = Arc::clone(&running);
        let real_time = Arc::new(OnceLock::new());
        let promoted = Arc::clone(&real_time);
        let join = thread::Builder::new()
            .name("kazoo-mix-hub".to_string())
            .spawn(move || {
                // Kept on this thread's stack for its whole life, including
                // socket readiness waits between serving passes.
                let (priority, outcome) = match RealTime::promote(HUB_TICK) {
                    Ok(priority) => (Some(priority), Ok(())),
                    // Kept for the desk to show, with why (see
                    // `Hub::real_time`).
                    Err(why) => (None, Err(why)),
                };
                promoted.get_or_init(|| outcome);
                server.run(&thread_running);
                if let Some(priority) = priority {
                    if let Err(why) = priority.demote() {
                        // The thread ends next, which ends its priority too.
                        server.notify(HubNotice::Fault(format!(
                            "the hub could not return to normal priority: {why}"
                        )));
                    }
                }
            })?;

        Ok(Self {
            running,
            join: Some(join),
            notices,
            stats,
            socket,
            listen,
            real_time,
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

    /// Whether the hub thread is scheduled as real-time, and why not when
    /// it is not; `None` until the thread has asked, just after it starts.
    #[must_use]
    pub fn real_time(&self) -> Option<&Result<(), NotRealTime>> {
        self.real_time.get()
    }

    #[must_use]
    pub fn refused_lines(&self, now: Instant) -> Vec<String> {
        self.stats.refusals.lines(now)
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
    stream: HubStream,
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
    network: Option<TcpListener>,
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
    /// Notes read this poll, kept between polls so a poll allocates
    /// nothing.
    notes: Vec<([u8; 16], NoteMessage)>,
    /// Instruments leaving this pass, kept for the same reason.
    leaving: Vec<(usize, LeaveReason)>,
    /// Instruments whose network lead the desk gives changed this poll,
    /// and what it is now (one each at most): kept between polls so a poll
    /// allocates nothing.
    grown: Vec<(usize, Duration)>,
}

/// Space for both listeners, every instrument, and twice as many pending streams.
const MAX_POLLED: usize = 2 + DESK_CHANNELS + 2 * DESK_CHANNELS;

impl Server {
    fn new(
        listener: UnixListener,
        network: Option<TcpListener>,
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
        let song = VecDeque::from([initial_song(&shared)]);
        Self {
            listener,
            network,
            config,
            shared,
            patchbay,
            notices,
            stats: Arc::clone(stats),
            pending: Vec::new(),
            instruments: Vec::with_capacity(DESK_CHANNELS),
            strips,
            patches: VecDeque::with_capacity(PATCH_CAPACITY),
            song,
            timer,
            last_refusal: None,
            notes: Vec::with_capacity(DESK_CHANNELS * MESSAGES_PER_POLL),
            leaving: Vec::with_capacity(2 * DESK_CHANNELS),
            grown: Vec::with_capacity(DESK_CHANNELS),
        }
    }

    fn run(&mut self, running: &AtomicBool) {
        let mut poll_fault_reported = false;
        while running.load(Ordering::Acquire) {
            self.poll_once();
            if let Err(error) = self.wait() {
                if !poll_fault_reported {
                    self.notify(HubNotice::Fault(format!(
                        "waiting on hub sockets failed: {error}"
                    )));
                    poll_fault_reported = true;
                }
                thread::park_timeout(HUB_TICK);
            }
        }
        self.shut_down();
    }

    /// Borrow live sockets for one readiness wait, without allocating.
    fn wait(&self) -> Result<(), Errno> {
        let mut fds: [PollFd<'_>; MAX_POLLED] =
            std::array::from_fn(|_| PollFd::new(&self.listener, PollFlags::IN));
        let mut n = 1;
        if let Some(network) = &self.network {
            fds[n] = PollFd::new(network, PollFlags::IN);
            n += 1;
        }
        for instrument in &self.instruments {
            fds[n] = PollFd::new(&instrument.stream, PollFlags::IN);
            n += 1;
        }
        for pending in self.pending.iter().take(MAX_POLLED - n) {
            fds[n] = PollFd::new(&pending.stream, PollFlags::IN);
            n += 1;
        }
        let timeout = Timespec::try_from(HUB_TICK).map_err(|_| Errno::INVAL)?;
        match poll(&mut fds[..n], Some(&timeout)) {
            Ok(_) | Err(Errno::INTR) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// One pass over everything the hub serves.
    fn poll_once(&mut self) {
        // Answers first: a strip the desk has emptied is free before anyone
        // new registers.
        self.collect_patch_events();
        self.accept();
        self.accept_tcp();
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
        self.stats.refusals.record(&notice, Instant::now());
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
                        stream: HubStream::Unix(stream),
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
    fn refuse(&mut self, stream: &HubStream, name: String, reason: RefuseReason) {
        let text = reason.to_string();
        let mut end = text.len().min(REFUSAL_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut frame = [0_u8; HEADER_SIZE + REFUSAL_BYTES];
        encode_header(
            &FrameHeader {
                msg_type: MSG_REFUSED,
                payload_len: end as u32,
                seq: 0,
            },
            &mut frame[..HEADER_SIZE],
        );
        frame[HEADER_SIZE..HEADER_SIZE + end].copy_from_slice(&text.as_bytes()[..end]);
        // A fresh connection's send buffer is empty, so the short refusal
        // goes out whole or not at all.
        let told = (&*stream).write_all(&frame[..HEADER_SIZE + end]).is_ok();
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
        let most = (MAX_SOURCE_RING_SAMPLES / (usize::from(channels) * 2)) as u32;
        if register.buffer_size > most {
            self.refuse(
                &pending.stream,
                name,
                RefuseReason::BlockTooLarge {
                    frames: register.buffer_size,
                    most,
                    channels,
                },
            );
            return;
        }
        let Some(slot) = self.strips.iter().position(|strip| *strip == Strip::Free) else {
            self.refuse(&pending.stream, name, RefuseReason::DeskFull);
            return;
        };

        let (producer, consumer) = audio_block_ring(strip_ring(slot, channels));

        let clock = self.clock();
        let transport = self.shared.transport();
        let registered = RegisteredMsg {
            strips: Vec::new(),
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
                block_frames: register
                    .buffer_size
                    .clamp(1, instrument::MAX_MESSAGE_FRAMES as u32),
                pace_lead_frames: register.pace_lead_frames,
                ring_frames: ring_frames(strip_ring(slot, channels)),
                sample_rate: self.config.sample_rate,
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
        let network_lead = instrument.network_news(clock);
        self.instruments.push(instrument);
        self.joined(slot, name, network_lead);
    }

    /// Say an instrument plugged into strip `slot`, and, when it plays over
    /// the network, the lead the desk gives it for that.
    fn joined(&mut self, slot: usize, name: String, network_lead: Option<Duration>) {
        self.notify(HubNotice::Joined {
            slot,
            name: name.clone(),
        });
        if let Some(lead) = network_lead {
            self.notify(HubNotice::NetworkLead { slot, name, lead });
        }
    }

    fn read_instruments(&mut self) {
        let clock = self.clock();
        let mut notes = std::mem::take(&mut self.notes);
        let mut leaving = std::mem::take(&mut self.leaving);
        let mut grown = std::mem::take(&mut self.grown);
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
            if let Some(lead) = instrument.network_news(clock) {
                grown.push((index, lead));
            }
        }
        for (source, note) in &notes {
            self.route_note(*source, note, &mut leaving);
        }
        notes.clear();
        self.notes = notes;
        for &(index, lead) in &grown {
            let (slot, name) = {
                let instrument = &self.instruments[index];
                (instrument.slot, instrument.name.clone())
            };
            self.notify(HubNotice::NetworkLead { slot, name, lead });
        }
        grown.clear();
        self.grown = grown;
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
        let mut leaving = std::mem::take(&mut self.leaving);
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
    /// playing its stream, and every instrument how the desk has been taking
    /// its audio when that has changed.
    fn pace_instruments(&mut self) {
        let clock = self.clock();
        let now = Instant::now();
        let mut leaving = std::mem::take(&mut self.leaving);
        for (index, instrument) in self.instruments.iter_mut().enumerate() {
            if let Err(err) = instrument
                .pace(clock, now)
                .and_then(|()| instrument.report_health(now))
            {
                leaving.push((index, leave_reason(&err)));
            }
        }
        self.remove(leaving);
    }

    fn flush_instruments(&mut self) {
        let mut leaving = std::mem::take(&mut self.leaving);
        for (index, instrument) in self.instruments.iter_mut().enumerate() {
            if let Err(err) = instrument.flush() {
                leaving.push((index, leave_reason(&err)));
            }
        }
        self.remove(leaving);
    }

    /// Disconnect instruments, highest index first so the rest stay valid,
    /// and keep `leaving`, emptied, for the next pass.
    fn remove(&mut self, mut leaving: Vec<(usize, LeaveReason)>) {
        // Ascending, so taking from the end goes highest index first; the
        // first reason given for an instrument is the one kept.
        leaving.sort_by_key(|(index, _)| *index);
        leaving.dedup_by_key(|(index, _)| *index);
        while let Some((index, reason)) = leaving.pop() {
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
        self.leaving = leaving;
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
            thread::park_timeout(Duration::from_millis(1));
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

/// Most extra lead an instrument may be given on a strip whose ring holds
/// `ring_frames`, against the desk's `clock` at `sample_rate`: the whole
/// lead held to half what the ring holds, so the other half takes the
/// audio an instrument sends in a burst after a stall, with the largest
/// block an instrument may send counted in the lead. And never more than a
/// second, which would hold every transport change back as long.
fn most_extra_lead(ring_frames: u64, clock: DeskClock, sample_rate: u32, block_frames: u32) -> u32 {
    let base =
        u64::from(clock.device_frames) + u64::from(block_frames) + u64::from(clock.margin_frames);
    let most = (ring_frames / 2).saturating_sub(base);
    // Held below the rate, a u32: the conversion is exact.
    most.min(u64::from(sample_rate)) as u32
}

/// Frames `ring` holds.
const fn ring_frames(ring: AudioRingConfig) -> u64 {
    // Widened before multiplying: exact.
    ring.capacity_blocks as u64 * ring.block_frames as u64
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
