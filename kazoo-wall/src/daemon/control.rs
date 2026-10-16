//! The control thread: it owns the wall, the connections and the audio
//! backend, and is the only thread that changes any of them.
//!
//! It answers each request in arrival order, pushes events to subscribed
//! connections, and between messages does the housekeeping: listening,
//! meters, faults, recording, saving, and keeping the audio running
//! (rebuilding a failed device stream every two seconds, forever, with the
//! headless loop keeping the wall's clock and desk feed going meanwhile).
//! A recording under way when the daemon stops is finished before it goes.

use std::collections::HashMap;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use kazoo_core::host::link::connect_config_or_direct;
use kazoo_core::ipc::link::{HubAddress, HubLink, LinkConfig};

mod desk_rate;
mod link_diagnostics;
#[cfg(test)]
mod tempo_tests;

use super::DeskMode;
use super::audio::headless_lead_frames;
use super::audio::{AudioMode, Device, Headless, StreamHealth};
use super::server::{Outgoing, ToControl, line};
use super::wall::{Outcome, Wall};
use crate::engine::{EngineConfig, EngineControl, MAX_CHUNK_FRAMES, engine};
use crate::listen;
use crate::patch::printable;
use crate::protocol::{
    Change, DAEMON, ErrorCode, Event, HelloResult, Listen, MonitorResult, Request, RequestLine,
    Response, ShutdownResult, SubscribeResult, WallError, valid_name,
};

/// Most connections at once.
pub const MAX_CONNECTIONS: usize = 64;

/// Longest wait between housekeeping rounds.
const TICK: Duration = Duration::from_millis(20);

/// Wait between attempts to reopen the audio device.
pub const DEVICE_RETRY: Duration = Duration::from_secs(2);

/// Sample rate of the headless loop when it stands in for a device.
const FALLBACK_RATE: u32 = 48_000;

/// Audio counts (device glitches and errors, the render loop falling past
/// the desk's lead, the desk's own counts) are reported at most this often,
/// and whenever the audio is rebuilt.
const REPORT_INTERVAL: Duration = Duration::from_secs(10);

/// The desk strip's name.
pub const DESK_NAME: &str = "kazoo-wall";

/// One connection.
#[derive(Debug)]
struct Conn {
    seat: Option<String>,
    console: bool,
    /// Watchers see everything, are never listed or announced, and may
    /// change nothing.
    watcher: bool,
    subscribed: bool,
    out: SyncSender<Outgoing>,
    stream: UnixStream,
    writer: Option<JoinHandle<()>>,
}

/// Where the engine is running.
enum Running {
    Device {
        // Held for its lifetime: dropping it stops the sound.
        _stream: cpal::Stream,
        health: Arc<StreamHealth>,
    },
    Headless(Headless),
}

/// What the running backend and desk link have counted so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counts {
    /// Device underruns and overruns.
    glitches: u64,
    /// Other device errors.
    errors: u64,
    /// Times the render loop fell past the desk's lead.
    past_lead: u64,
    /// Times the desk placed the wall's stream afresh.
    placed_afresh: u64,
    /// Blocks the desk dropped because the strip's ring was full.
    desk_dropped: u64,
}

/// One count kept across rebuilds of the audio: every rebuild starts a new
/// backend and desk link, counting from zero.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tally {
    /// What the backends and links that have gone reached.
    finished: u64,
    /// The whole count as last reported.
    told: u64,
}

impl Tally {
    /// The whole count, given what the running backend or link has reached.
    const fn total(self, running: u64) -> u64 {
        self.finished.saturating_add(running)
    }

    /// The line to log if the whole count has changed since it was last
    /// reported.
    fn report(&mut self, running: u64, what: &str) -> Option<String> {
        let total = self.total(running);
        (total != self.told).then(|| {
            self.told = total;
            format!("kazoo-wall: {total} {what} so far")
        })
    }

    /// The running backend or link is going: keep what it reached.
    const fn finish(&mut self, running: u64) {
        self.finished = self.total(running);
    }
}

/// The audio counts across rebuilds, and when they were last reported.
#[derive(Debug, Clone, Copy)]
struct AudioCounts {
    reported: Instant,
    glitches: Tally,
    errors: Tally,
    past_lead: Tally,
    placed_afresh: Tally,
    desk_dropped: Tally,
}

impl AudioCounts {
    fn new(now: Instant) -> Self {
        Self {
            reported: now,
            glitches: Tally::default(),
            errors: Tally::default(),
            past_lead: Tally::default(),
            placed_afresh: Tally::default(),
            desk_dropped: Tally::default(),
        }
    }

    /// Every count that has changed since it was last reported, given what
    /// the running backend and link have counted, as lines for the log.
    fn report(&mut self, running: Counts, now: Instant) -> Vec<String> {
        self.reported = now;
        [
            self.glitches.report(running.glitches, "audio glitches"),
            self.errors.report(running.errors, "audio device errors"),
            self.past_lead
                .report(running.past_lead, "render-loop stalls past the desk's lead"),
            self.placed_afresh.report(
                running.placed_afresh,
                "fresh placements of the wall's stream by the desk (a block late, each heard as \
                 a jump)",
            ),
            self.desk_dropped.report(
                running.desk_dropped,
                "wall blocks the desk dropped (its strip was full)",
            ),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// Whether the counts are due to be reported at `now`.
    fn due(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.reported) >= REPORT_INTERVAL
    }

    /// The running backend and link are going: keep what they counted.
    const fn finish(&mut self, running: Counts) {
        self.glitches.finish(running.glitches);
        self.errors.finish(running.errors);
        self.past_lead.finish(running.past_lead);
        self.placed_afresh.finish(running.placed_afresh);
        self.desk_dropped.finish(running.desk_dropped);
    }
}

/// The audio side: where the engine runs, and its listening thread.
struct Backend {
    running: Running,
    listen_stop: Arc<AtomicBool>,
    listen_join: Option<JoinHandle<()>>,
    /// When to try the device again, while standing in for it.
    retry_at: Option<Instant>,
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.listen_stop.store(true, Ordering::Release);
        if let Some(join) = self.listen_join.take() {
            if join.join().is_err() {
                eprintln!("kazoo-wall: the listening thread panicked");
            }
        }
    }
}

/// Everything the control thread needs to start.
pub struct Setup {
    /// The audio mode.
    pub audio: AudioMode,
    /// The desk link mode.
    pub desk: DeskMode,
    /// The tempo to start at: the saved patch's.
    pub tempo: f64,
    /// Requests from the socket threads.
    pub inbox: Receiver<ToControl>,
    /// Set to stop.
    pub stop: Arc<AtomicBool>,
    /// The accept thread, joined on the way out.
    pub accept: JoinHandle<()>,
    /// Builds the wall, given its first engine and desk link.
    pub build: Box<dyn FnOnce(EngineControl, Option<HubLink>) -> Wall + Send>,
}

impl std::fmt::Debug for Setup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setup")
            .field("audio", &self.audio)
            .field("desk", &self.desk)
            .finish_non_exhaustive()
    }
}

/// The control thread's state.
struct Control {
    wall: Wall,
    conns: HashMap<u64, Conn>,
    audio: AudioMode,
    desk: DeskMode,
    backend: Option<Backend>,
    listen_tx: Sender<Listen>,
    listen_rx: Receiver<Listen>,
    stop: Arc<AtomicBool>,
    stopping: bool,
    /// Whether the wall is heard. It starts silent, playing on, until a
    /// console turns the monitor on; a rebuilt engine keeps the choice.
    heard: bool,
    last_tick: Instant,
    /// `speak` requests waiting for their words: ticket → (connection,
    /// request id).
    awaiting: HashMap<u64, (u64, u64)>,
    /// The audio counts, kept across rebuilds of the audio.
    counts: AudioCounts,
    /// Whether the scheduling refusal has already been logged.
    told_link_refused: bool,
    desk_refusals: link_diagnostics::DeskRefusals,
    /// The process-wide desk link shutdown counts already logged.
    reported_link_shutdowns: link_diagnostics::LinkShutdowns,
    /// Following the rate of a desk the wall joins (KZ-D21).
    desk_rate: desk_rate::Watch,
}

/// Run the control thread until stopped.
pub fn run(setup: Setup) {
    let (listen_tx, listen_rx) = std::sync::mpsc::channel();
    let (backend, engine, link) = start_audio(
        setup.audio,
        &setup.desk,
        (setup.tempo, 0.0, 0.0),
        false,
        &listen_tx,
        None,
    );
    let wall = (setup.build)(engine, link);
    let mut control = Control {
        wall,
        conns: HashMap::new(),
        audio: setup.audio,
        desk: setup.desk,
        backend: Some(backend),
        listen_tx,
        listen_rx,
        stop: setup.stop,
        stopping: false,
        heard: false,
        last_tick: Instant::now(),
        awaiting: HashMap::new(),
        counts: AudioCounts::new(Instant::now()),
        told_link_refused: false,
        desk_refusals: link_diagnostics::DeskRefusals::default(),
        reported_link_shutdowns: link_diagnostics::LinkShutdowns::default(),
        desk_rate: desk_rate::Watch::default(),
    };
    while !control.stopping {
        match setup.inbox.recv_timeout(TICK) {
            Ok(message) => control.handle(message),
            Err(RecvTimeoutError::Timeout) => control.housekeeping(),
            Err(RecvTimeoutError::Disconnected) => control.stopping = true,
        }
        if control.last_tick.elapsed() >= TICK {
            control.housekeeping();
        }
        if control.stop.load(Ordering::Acquire) {
            control.stopping = true;
        }
    }
    control.stop.store(true, Ordering::Release);
    control.finish();
    let mut reported_link_shutdowns = control.reported_link_shutdowns;
    drop(control);
    link_diagnostics::report_shutdowns(
        &mut reported_link_shutdowns,
        link_diagnostics::LinkShutdowns::now(),
        |line| eprintln!("{line}"),
    );
    if setup.accept.join().is_err() {
        eprintln!("kazoo-wall: the accept thread panicked");
    }
}

/// Start the engine on the device (or the headless loop), with its desk
/// link and listening thread, at `(bpm, beat)` with the wall playing from
/// `origin`, heard or silent.
/// `failed_before` is when the device last failed, if it did (the failure
/// is then not reported again).
fn start_audio(
    mode: AudioMode,
    desk: &DeskMode,
    (bpm, beat, origin): (f64, f64, f64),
    heard: bool,
    listen_tx: &Sender<Listen>,
    failed_before: Option<Instant>,
) -> (Backend, EngineControl, Option<HubLink>) {
    let bpm = crate::engine::clamp_bpm(bpm);
    if let AudioMode::Device { sample_rate } = mode {
        match Device::open(sample_rate) {
            Ok(device) => match play_on(&device, desk, (bpm, beat, origin), heard, listen_tx) {
                Ok(started) => return started,
                Err(why) => eprintln!("kazoo-wall: {why}; retrying in 2 s"),
            },
            Err(why) => {
                if failed_before.is_none() {
                    eprintln!("kazoo-wall: {why}; retrying every 2 s");
                }
            }
        }
    }
    let rate = match mode {
        AudioMode::Headless { sample_rate } => sample_rate.max(8_000),
        AudioMode::Device { sample_rate } => sample_rate.unwrap_or(FALLBACK_RATE).max(8_000),
    };
    let retry_at = matches!(mode, AudioMode::Device { .. }).then(|| Instant::now() + DEVICE_RETRY);
    start_headless(desk, rate, (bpm, beat, origin), heard, listen_tx, retry_at)
}

/// The engine on `device` at the rate it is set to, with its desk link and
/// listening thread, at `(bpm, beat)` with the wall playing from `origin`,
/// heard or silent; why not, when the device will not play.
fn play_on(
    device: &Device,
    desk: &DeskMode,
    at: (f64, f64, f64),
    heard: bool,
    listen_tx: &Sender<Listen>,
) -> Result<(Backend, EngineControl, Option<HubLink>), String> {
    let rate = device.sample_rate();
    let link = link_to_desk(desk, rate, Pacing::Device);
    let (hub, link) = split_link(link);
    let (engine, mut control) = engine(engine_config(rate, at, heard), hub);
    let health = Arc::new(StreamHealth::default());
    let stream = device.play(engine, &health)?;
    eprintln!("kazoo-wall: playing on {} at {rate} Hz", device.name());
    let backend = backend(
        Running::Device {
            _stream: stream,
            health,
        },
        &mut control,
        listen_tx,
        None,
    );
    Ok((backend, control, link))
}

/// The headless loop at `rate`, with its desk link and listening thread,
/// at `(bpm, beat)` with the wall playing from `origin`, heard or silent;
/// the device is tried again at `retry_at`, if one is given.
fn start_headless(
    desk: &DeskMode,
    rate: u32,
    at: (f64, f64, f64),
    heard: bool,
    listen_tx: &Sender<Listen>,
    retry_at: Option<Instant>,
) -> (Backend, EngineControl, Option<HubLink>) {
    let link = link_to_desk(desk, rate, Pacing::Timer);
    let (hub, link) = split_link(link);
    let (engine, mut control) = engine(engine_config(rate, at, heard), hub);
    let (running, retry_at) = match Headless::start(engine, rate) {
        Ok(headless) => (Running::Headless(headless), retry_at),
        Err(err) => {
            // Without a thread there is nothing to render on: it is tried
            // again once the retry is up, never on every round.
            eprintln!("kazoo-wall: the render thread could not start: {err}; retrying in 2 s");
            let retry = Instant::now() + DEVICE_RETRY;
            (
                Running::Headless(Headless::stopped()),
                Some(retry_at.map_or(retry, |at| at.max(retry))),
            )
        }
    };
    (
        backend(running, &mut control, listen_tx, retry_at),
        control,
        link,
    )
}

/// An engine at `rate` carrying on from `(bpm, beat)` and the wall's
/// `origin`, heard or silent from its first frame.
fn engine_config(rate: u32, (bpm, beat, origin): (f64, f64, f64), heard: bool) -> EngineConfig {
    EngineConfig {
        audible: heard,
        origin,
        ..EngineConfig::new(rate, bpm, beat)
    }
}

fn backend(
    running: Running,
    control: &mut EngineControl,
    listen_tx: &Sender<Listen>,
    retry_at: Option<Instant>,
) -> Backend {
    let listen_stop = Arc::new(AtomicBool::new(false));
    let listen_join = control.take_listen().and_then(|ring| {
        match listen::spawn(
            ring,
            control.sample_rate(),
            listen_tx.clone(),
            Arc::clone(&listen_stop),
        ) {
            Ok(join) => Some(join),
            Err(err) => {
                eprintln!("kazoo-wall: listening could not start: {err}");
                None
            }
        }
    });
    Backend {
        running,
        listen_stop,
        listen_join,
        retry_at,
    }
}

/// What clocks the wall's rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pacing {
    /// The audio device's callbacks.
    Device,
    /// The headless loop's timer, which asks the desk to pace it.
    Timer,
}

/// The desk link for a stream at `rate`, if the wall joins the desk.
fn link_to_desk(
    desk: &DeskMode,
    rate: u32,
    pacing: Pacing,
) -> Option<(HubLink, kazoo_core::ipc::link::HubLinkAudio)> {
    let address = match desk {
        DeskMode::Off => return None,
        DeskMode::Discover => HubAddress::Discover,
        DeskMode::Socket(path) => HubAddress::Socket(path.clone()),
    };
    // MAX_CHUNK_FRAMES is 4096: fits.
    let mut config = LinkConfig::new(DESK_NAME, 2, rate, MAX_CHUNK_FRAMES as u32);
    config.address = address;
    // Whichever clocks the wall, its blocks go to the desk through the link
    // thread: a busy machine must not hold them up on the way. The daemon's
    // stderr is its log, where a refusal is said.
    config.real_time = true;
    if pacing == Pacing::Timer {
        config.pace_lead_frames = headless_lead_frames(rate);
    }
    match connect_config_or_direct(config) {
        Ok(link) => Some(link),
        Err(err) => {
            eprintln!("kazoo-wall: the desk link could not start: {err}; playing on my own");
            None
        }
    }
}

fn split_link(
    link: Option<(HubLink, kazoo_core::ipc::link::HubLinkAudio)>,
) -> (Option<kazoo_core::ipc::link::HubLinkAudio>, Option<HubLink>) {
    match link {
        Some((link, audio)) => (Some(audio), Some(link)),
        None => (None, None),
    }
}

impl Control {
    fn handle(&mut self, message: ToControl) {
        match message {
            ToControl::Opened {
                conn,
                out,
                stream,
                writer,
            } => self.opened(conn, out, stream, writer),
            ToControl::Line { conn, line } => match line {
                Ok(request) => self.request(conn, &request),
                Err(response) => self.send(conn, &response),
            },
            ToControl::TooLong { conn } => {
                let response = Response::failure(
                    0,
                    WallError::new(
                        ErrorCode::BadRequest,
                        format!(
                            "a line longer than {} bytes; closing the connection",
                            crate::protocol::MAX_LINE
                        ),
                    ),
                );
                self.send(conn, &response);
                self.close(conn);
            }
            ToControl::Closed { conn } => self.close(conn),
        }
    }

    fn opened(
        &mut self,
        conn: u64,
        out: SyncSender<Outgoing>,
        stream: UnixStream,
        writer: JoinHandle<()>,
    ) {
        if self.conns.len() >= MAX_CONNECTIONS {
            let response = Response::failure(
                0,
                WallError::new(
                    ErrorCode::Full,
                    format!("the wall takes {MAX_CONNECTIONS} connections at once"),
                ),
            );
            if out.try_send(Outgoing::Line(line(&response))).is_err()
                || out.try_send(Outgoing::Close).is_err()
            {
                shut(&stream);
            }
            return;
        }
        self.conns.insert(
            conn,
            Conn {
                seat: None,
                console: false,
                watcher: false,
                subscribed: false,
                out,
                stream,
                writer: Some(writer),
            },
        );
    }

    fn request(&mut self, conn: u64, line: &RequestLine) {
        let Some(state) = self.conns.get(&conn) else {
            return;
        };
        let id = line.id;
        let seat = state.seat.clone();
        let console = state.console;
        let watcher = state.watcher;
        let mut comment = None;
        if seat.is_some() {
            let refused = refusal(&line.request, console, watcher)
                .map_or_else(|| self.wall.check_wall(line.wall, &line.request), Err)
                .and_then(|()| commented(line));
            match refused {
                Ok(checked) => comment = checked,
                Err(refused) => {
                    self.send(conn, &Response::failure(id, refused));
                    return;
                }
            }
        }
        let response = match (&line.request, seat) {
            (
                Request::Hello {
                    seat,
                    client,
                    console,
                    watcher,
                },
                None,
            ) => self.hello(conn, id, seat, client, *console, *watcher),
            (Request::Hello { .. }, Some(_)) => Response::failure(
                id,
                WallError::new(
                    ErrorCode::NotAllowed,
                    "this connection has said hello already",
                ),
            ),
            (_, None) => {
                Response::failure(id, WallError::new(ErrorCode::NotHello, "say hello first"))
            }
            (Request::Subscribe, Some(_)) => {
                if let Some(state) = self.conns.get_mut(&conn) {
                    state.subscribed = true;
                }
                respond(
                    id,
                    &SubscribeResult {
                        seats: self.wall.seats(),
                    },
                )
            }
            (Request::Monitor { on }, Some(seat)) => {
                if console {
                    self.heard = *on;
                    self.wall.engine_mut().shared().set_audible(*on);
                    eprintln!(
                        "kazoo-wall: {seat} {} the wall",
                        if *on { "is hearing" } else { "silenced" }
                    );
                    respond(id, &MonitorResult { on: *on })
                } else {
                    Response::failure(
                        id,
                        WallError::new(
                            ErrorCode::NotAllowed,
                            "only a console can make the wall heard or silent; seats cannot",
                        ),
                    )
                }
            }
            (Request::Shutdown, Some(seat)) => {
                if console {
                    eprintln!("kazoo-wall: {seat} asked the wall to stop");
                    self.stopping = true;
                    respond(id, &ShutdownResult { stopping: true })
                } else {
                    Response::failure(
                        id,
                        WallError::new(
                            ErrorCode::NotAllowed,
                            "only a console can stop the wall; seats cannot",
                        ),
                    )
                }
            }
            (request, Some(seat)) => {
                match self.ask_wall((conn, id), (&seat, console), request, comment.as_deref()) {
                    Some(response) => response,
                    // Answered when it is ready (see `housekeeping`).
                    None => return,
                }
            }
        };
        self.send(conn, &response);
    }

    /// Ask the wall `request` (line `id` on `conn`) for `seat`, a console
    /// or not, with the sender's `comment` (checked already) on its change,
    /// and tell everyone what it changed. The response; none for a request
    /// answered later (its ticket is kept).
    fn ask_wall(
        &mut self,
        (conn, id): (u64, u64),
        (seat, console): (&str, bool),
        request: &Request,
        comment: Option<&str>,
    ) -> Option<Response> {
        match self
            .wall
            .request_commented((seat, console), request, comment, Instant::now())
        {
            Ok(outcome) => {
                if let Some(ticket) = outcome.pending {
                    self.awaiting.insert(ticket, (conn, id));
                    return None;
                }
                self.announce(seat, outcome.change, outcome.event);
                Some(Response {
                    id,
                    ok: true,
                    result: Some(outcome.result),
                    error: None,
                })
            }
            Err(err) => Some(Response::failure(id, err)),
        }
    }

    /// Tell everyone about a request's outcome: its change to every other
    /// seat, then its event to everyone: the fingerprints a change moved
    /// (the actor cannot work out the flow for itself), or the rack's rows
    /// after a move.
    fn announce(&mut self, seat: &str, change: Option<Change>, event: Option<Event>) {
        if let Some(change) = change {
            let event = Event::Change {
                change: Box::new(change),
            };
            self.broadcast(&event, Some(seat));
        }
        if let Some(event) = event {
            self.broadcast(&event, None);
        }
    }

    /// Answer the `speak` requests whose words are ready.
    fn answer_speech(&mut self) {
        let answers = self.wall.spoken();
        self.answer_later(answers);
    }

    fn answer_tree(&mut self, now: Instant) {
        let answers = self.wall.tree_answers(now);
        self.answer_later(answers);
    }

    fn answer_later(&mut self, answers: Vec<(u64, Result<Outcome, WallError>)>) {
        for (ticket, answer) in answers {
            let asker = self.awaiting.remove(&ticket);
            let response = match answer {
                Ok(outcome) => {
                    let seat = outcome.change.as_ref().map(|change| change.seat.clone());
                    self.announce(
                        seat.as_deref().unwrap_or("kazoo-wall"),
                        outcome.change.clone(),
                        outcome.event,
                    );
                    asker.map(|(_, id)| Response {
                        id,
                        ok: true,
                        result: Some(outcome.result),
                        error: None,
                    })
                }
                Err(err) => asker.map(|(_, id)| Response::failure(id, err)),
            };
            if let (Some((conn, _)), Some(response)) = (asker, response) {
                self.send(conn, &response);
            }
        }
    }

    /// Answer saves once their loops have logged their snapshots.
    fn answer_saves(&mut self) {
        for (ticket, answer) in self.wall.saved() {
            let asker = self.awaiting.remove(&ticket);
            let response = match answer {
                Ok(outcome) => {
                    self.announce("kazoo-wall", outcome.change, outcome.event);
                    asker.map(|(_, id)| Response {
                        id,
                        ok: true,
                        result: Some(outcome.result),
                        error: None,
                    })
                }
                Err(error) => asker.map(|(_, id)| Response::failure(id, error)),
            };
            if let (Some((conn, _)), Some(response)) = (asker, response) {
                self.send(conn, &response);
            }
        }
    }

    fn hello(
        &mut self,
        conn: u64,
        id: u64,
        seat: &str,
        client: &str,
        console: bool,
        watcher: bool,
    ) -> Response {
        if seat.eq_ignore_ascii_case("desk") || seat.eq_ignore_ascii_case("kazoo-wall") {
            return Response::failure(
                id,
                WallError::new(
                    ErrorCode::BadName,
                    format!("'{seat}' is a reserved seat name; choose another name."),
                ),
            );
        }
        if !valid_name(seat) {
            return Response::failure(
                id,
                WallError::new(
                    ErrorCode::BadName,
                    format!(
                        "'{}' is not a valid seat name: use 1 to 24 of A-Z a-z 0-9 space _ . -",
                        printable(seat)
                    ),
                ),
            );
        }
        if let Some(state) = self.conns.get_mut(&conn) {
            state.seat = Some(seat.to_string());
            state.console = console;
            state.watcher = watcher;
        }
        if watcher {
            eprintln!("kazoo-wall: {seat} is watching ({})", printable(client));
        } else if self.wall.seat_joined(seat) {
            eprintln!("kazoo-wall: {seat} joined ({})", printable(client));
            let event = Event::Seat {
                seat: seat.to_string(),
                joined: true,
                seq: Some(self.wall.revision()),
            };
            self.broadcast(&event, Some(seat));
        }
        respond(
            id,
            &HelloResult {
                daemon: DAEMON.to_string(),
                seat: seat.to_string(),
                console,
                seats: self.wall.seats(),
                revision: self.wall.revision(),
                watcher,
            },
        )
    }

    /// Queue `response` for `conn`; a connection that cannot take it is
    /// closed.
    fn send(&mut self, conn: u64, response: &Response) {
        let bytes = line(response);
        let Some(state) = self.conns.get(&conn) else {
            return;
        };
        match state.out.try_send(Outgoing::Line(bytes)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                eprintln!("kazoo-wall: connection {conn} is not reading; closing it");
                shut(&state.stream);
                self.close(conn);
            }
            Err(TrySendError::Disconnected(_)) => self.close(conn),
        }
    }

    /// Push `event` to every subscribed connection, except those of seat
    /// `except` (a watcher is never a seat's own connection, so it hears
    /// everything, whatever its name).
    fn broadcast(&mut self, event: &Event, except: Option<&str>) {
        let bytes = line(event);
        let mut stuck = Vec::new();
        for (conn, state) in &self.conns {
            let theirs = !state.watcher && except.is_some() && state.seat.as_deref() == except;
            let to_them = state.subscribed && state.seat.is_some() && !theirs;
            if to_them && state.out.try_send(Outgoing::Line(bytes.clone())).is_err() {
                stuck.push(*conn);
            }
        }
        for conn in stuck {
            eprintln!("kazoo-wall: connection {conn} is not reading its events; closing it");
            if let Some(state) = self.conns.get(&conn) {
                shut(&state.stream);
            }
            self.close(conn);
        }
    }

    fn close(&mut self, conn: u64) {
        let Some(state) = self.conns.remove(&conn) else {
            return;
        };
        if state.out.try_send(Outgoing::Close).is_err() {
            shut(&state.stream);
        }
        let seat = state.seat.filter(|_| !state.watcher);
        if let Some(seat) = seat {
            if self.wall.seat_left(&seat) {
                eprintln!("kazoo-wall: {seat} left");
                let event = Event::Seat {
                    seat: seat.clone(),
                    joined: false,
                    seq: Some(self.wall.revision()),
                };
                self.broadcast(&event, Some(&seat));
            }
        }
    }

    fn housekeeping(&mut self) {
        if let Some(link) = self.wall.desk_link() {
            self.desk_refusals
                .report(&link.status(), |line| eprintln!("{line}"));
        }
        link_diagnostics::report_refusal(
            &mut self.told_link_refused,
            self.wall.desk_link().and_then(HubLink::real_time),
            |line| eprintln!("{line}"),
        );
        link_diagnostics::report_shutdowns(
            &mut self.reported_link_shutdowns,
            link_diagnostics::LinkShutdowns::now(),
            |line| eprintln!("{line}"),
        );
        let now = Instant::now();
        self.last_tick = now;
        while let Ok(listen) = self.listen_rx.try_recv() {
            self.wall.heard(listen);
        }
        for event in self.wall.tick(now) {
            self.broadcast(&event, None);
        }
        self.answer_speech();
        self.answer_tree(now);
        self.answer_saves();
        self.watch_audio(now);
    }

    /// Rebuild the audio when the device failed, or try the device again
    /// while the headless loop stands in for it.
    fn watch_audio(&mut self, now: Instant) {
        if self.counts.due(now) {
            self.report_counts(now);
        }
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        let rebuild = match &mut backend.running {
            Running::Device { health, .. } => {
                let failed = health.failed.load(Ordering::Acquire);
                if failed {
                    eprintln!("kazoo-wall: the audio device went away; rebuilding in 2 s");
                }
                failed
            }
            Running::Headless(headless) => {
                // A render loop that stopped starts again at once, unless a
                // retry is set (one that could not start waits for it).
                let due = backend.retry_at.is_some_and(|at| now >= at);
                let died = headless.finished() && (backend.retry_at.is_none() || due);
                if died {
                    eprintln!("kazoo-wall: the render loop stopped; restarting it");
                }
                died || due
            }
        };
        if rebuild {
            self.rebuild(now);
            return;
        }
        self.follow_desk_rate(now);
    }

    /// What the running backend and desk link have counted.
    fn running_counts(&self) -> Counts {
        let mut counts = self.desk_counts();
        match self.backend.as_ref().map(|backend| &backend.running) {
            Some(Running::Device { health, .. }) => {
                counts.glitches = health.glitches.load(Ordering::Relaxed);
                counts.errors = health.errors.load(Ordering::Relaxed);
            }
            Some(Running::Headless(headless)) => counts.past_lead = headless.times_past_lead(),
            None => {}
        }
        counts
    }

    /// What the desk has said of the wall's stream over the running link.
    fn desk_counts(&self) -> Counts {
        let desk = self
            .wall
            .desk_link()
            .map(HubLink::desk_health)
            .unwrap_or_default();
        Counts {
            placed_afresh: desk.placed_afresh,
            desk_dropped: desk.dropped,
            ..Counts::default()
        }
    }

    /// Log every audio count that has changed since it was last logged.
    fn report_counts(&mut self, now: Instant) {
        let running = self.running_counts();
        for line in self.counts.report(running, now) {
            eprintln!("{line}");
        }
    }

    /// Report the counts of the backend and link about to go, and keep
    /// them: the next ones count from zero.
    fn finish_counts(&mut self, now: Instant) {
        let running = self.running_counts();
        self.finish_with(running, now);
    }

    /// Report `running`, the counts of the backend and link about to go,
    /// and keep them.
    fn finish_with(&mut self, running: Counts, now: Instant) {
        for line in self.counts.report(running, now) {
            eprintln!("{line}");
        }
        self.counts.finish(running);
    }

    fn rebuild(&mut self, now: Instant) {
        let device_failed = matches!(
            self.backend.as_ref().map(|b| &b.running),
            Some(Running::Device { .. })
        );
        let following = self.desk_rate.following();
        self.replace_audio(now, |control, at| {
            if let Some(rate) = following {
                // The render loop rendering for a desk stopped: it starts
                // again at the desk's rate, never back on the device while
                // the desk is up.
                start_headless(
                    &control.desk,
                    rate,
                    at,
                    control.heard,
                    &control.listen_tx,
                    None,
                )
            } else if device_failed {
                // Stand in with the headless loop now; the device is tried
                // again in two seconds.
                start_headless(
                    &control.desk,
                    FALLBACK_RATE,
                    at,
                    control.heard,
                    &control.listen_tx,
                    Some(now + DEVICE_RETRY),
                )
            } else {
                eprintln!("kazoo-wall: trying the audio device again");
                start_audio(
                    control.audio,
                    &control.desk,
                    at,
                    control.heard,
                    &control.listen_tx,
                    Some(now),
                )
            }
        });
    }

    /// Stop the audio and start it again with `start`, which is given the
    /// wall's `(bpm, beat, origin)` to carry on from (the wall playing
    /// carries on from the bar it landed on).
    fn replace_audio(
        &mut self,
        now: Instant,
        start: impl FnOnce(&mut Self, (f64, f64, f64)) -> (Backend, EngineControl, Option<HubLink>),
    ) {
        let shared = Arc::clone(self.wall.engine_mut().shared());
        let at = (shared.bpm(), shared.beat(), shared.origin());
        // The old backend's counts are read as it goes; the old desk link
        // serves until the new one takes its place, so what the desk says
        // in the meantime is read last of all.
        let device = self.running_counts();
        // The old engine (and its half of the desk link) stops first.
        self.backend = None;
        let (backend, engine, link) = start(self, at);
        let desk = self.desk_counts();
        self.finish_with(
            Counts {
                placed_afresh: desk.placed_afresh,
                desk_dropped: desk.desk_dropped,
                ..device
            },
            now,
        );
        self.wall.attach(engine, link, now);
        self.backend = Some(backend);
    }

    /// Stop: finish a recording under way, tell every connection, save,
    /// and let the audio go.
    fn finish(&mut self) {
        let now = Instant::now();
        if let Some(event) = self.wall.end_recording() {
            self.broadcast(&event, None);
        }
        // What the log is owed is tried once more; what it still cannot
        // take is said lost (never numbered or told, so never reused).
        for event in self.wall.close(now) {
            self.broadcast(&event, None);
        }
        let answers = self.wall.settled_desk_go_answers();
        self.answer_later(answers);
        let conns: Vec<u64> = self.conns.keys().copied().collect();
        let mut writers = Vec::new();
        for conn in conns {
            if let Some(state) = self.conns.get_mut(&conn) {
                writers.extend(state.writer.take());
            }
            self.close(conn);
        }
        for writer in writers {
            // Writes time out after two seconds: this never hangs.
            if writer.join().is_err() {
                eprintln!("kazoo-wall: a connection's writer panicked");
            }
        }
        self.finish_counts(now);
        self.backend = None;
        eprintln!("kazoo-wall: stopped; the patch is saved");
    }
}

/// The comment `line` carries, trimmed, if it has one: only a request that
/// takes one may (see [`Request::takes_comment`]), held to the rules of
/// [`crate::protocol::RequestLine::comment`].
fn commented(line: &RequestLine) -> Result<Option<String>, WallError> {
    let Some(comment) = line.comment.as_deref() else {
        return Ok(None);
    };
    if !line.request.takes_comment() {
        let why = if matches!(line.request, Request::Batch { .. }) {
            "a batch says why with its note, and each step with its own comments".to_string()
        } else {
            format!(
                "a comment goes on a change of its own (a turn, patch, unpatch, add, remove, \
                 chain add, undo, tempo, loop, record, load, go or new), not on {}",
                line.request.op()
            )
        };
        return Err(WallError::new(ErrorCode::BadRequest, why));
    }
    crate::protocol::batch::comment(comment).map(|kept| Some(kept.to_string()))
}

/// Why a connection that has said hello may not make `request`, if it may
/// not: a watcher changes nothing; and the saves are Tom's safety net, so
/// only a console may destroy one, by deleting it or saving over it.
fn refusal(request: &Request, console: bool, watcher: bool) -> Option<WallError> {
    let why = match request {
        _ if watcher && request.is_change() => {
            format!("a watcher cannot {}; it only watches", request.op())
        }
        Request::DeleteSave { .. } if !console => {
            "only a console can delete a save; seats cannot".to_string()
        }
        Request::Save {
            overwrite: true, ..
        } if !console => "only a console can replace a save; save under a new name".to_string(),
        _ => return None,
    };
    Some(WallError::new(ErrorCode::NotAllowed, why))
}

fn respond(id: u64, result: &impl serde::Serialize) -> Response {
    Response::success(id, result).unwrap_or_else(|err| Response::failure(id, err))
}

fn shut(stream: &UnixStream) {
    if let Err(err) = stream.shutdown(Shutdown::Both) {
        if err.kind() != std::io::ErrorKind::NotConnected {
            eprintln!("kazoo-wall: closing a connection failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tally_reports_growth_once_and_keeps_counting_across_rebuilds() {
        let mut tally = Tally::default();
        assert_eq!(tally.report(0, "things"), None);
        assert_eq!(
            tally.report(2, "things").as_deref(),
            Some("kazoo-wall: 2 things so far")
        );
        assert_eq!(tally.report(2, "things"), None);
        // The backend goes at 3; the next counts from zero.
        tally.finish(3);
        assert_eq!(
            tally.report(0, "things").as_deref(),
            Some("kazoo-wall: 3 things so far")
        );
        assert_eq!(tally.report(0, "things"), None);
        assert_eq!(
            tally.report(1, "things").as_deref(),
            Some("kazoo-wall: 4 things so far")
        );
    }

    #[test]
    fn audio_counts_report_what_changed_and_survive_a_rebuild() {
        let start = Instant::now();
        let mut counts = AudioCounts::new(start);
        assert!(!counts.due(start));
        assert!(counts.due(start + REPORT_INTERVAL));
        assert!(counts.report(Counts::default(), start).is_empty());
        let running = Counts {
            past_lead: 2,
            placed_afresh: 1,
            ..Counts::default()
        };
        let lines = counts.report(running, start);
        assert_eq!(
            lines,
            [
                "kazoo-wall: 2 render-loop stalls past the desk's lead so far",
                "kazoo-wall: 1 fresh placements of the wall's stream by the desk (a block late, each \
                 heard as a jump) so far",
            ]
        );
        // A device retry every two seconds rebuilds the audio: nothing is
        // lost, and nothing is said again until something changes.
        counts.finish(running);
        for _ in 0..5 {
            assert!(counts.report(Counts::default(), start).is_empty());
            counts.finish(Counts::default());
        }
        let lines = counts.report(
            Counts {
                desk_dropped: 1,
                ..Counts::default()
            },
            start,
        );
        assert_eq!(
            lines,
            ["kazoo-wall: 1 wall blocks the desk dropped (its strip was full) so far"]
        );
    }

    #[test]
    fn a_built_or_rebuilt_engine_is_heard_as_the_wall_was() {
        for heard in [false, true] {
            let config = engine_config(48_000, (96.0, 12.5, 8.0), heard);
            assert_eq!(config.audible, heard);
            assert_eq!(config.sample_rate, 48_000);
            assert!((config.bpm - 96.0).abs() < f64::EPSILON);
            assert!((config.beat - 12.5).abs() < f64::EPSILON);
            let (_engine, control) = engine(config, None);
            assert_eq!(control.shared().audible(), heard);
            // The wall playing carries on from the bar it landed on.
            assert_eq!(control.shared().origin().to_bits(), 8.0_f64.to_bits());
        }
    }
}

#[cfg(test)]
mod moments_tests {
    use super::{commented, refusal};
    use crate::protocol::{ErrorCode, Request, RequestLine};

    fn line(value: serde_json::Value) -> RequestLine {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn a_comment_goes_only_on_a_change_of_its_own_and_is_kept_trimmed() {
        let turn = line(serde_json::json!({
            "id": 1, "op": "turn", "module": "vcf1", "knob": "cutoff", "value": 300.0,
            "comment": "  brighter  ",
        }));
        assert_eq!(commented(&turn).unwrap().as_deref(), Some("brighter"));
        let plain = line(serde_json::json!({"id": 2, "op": "look"}));
        assert_eq!(commented(&plain).unwrap(), None);
        let look = line(serde_json::json!({"id": 3, "op": "look", "comment": "why"}));
        let refused = commented(&look).unwrap_err();
        assert_eq!(refused.code, ErrorCode::BadRequest);
        assert!(
            refused.message.contains("not on look"),
            "{}",
            refused.message
        );
        let batch = line(serde_json::json!({
            "id": 4, "op": "batch", "note": "n",
            "changes": [{"op": "tempo", "bpm": 90.0}], "comment": "why",
        }));
        assert!(
            commented(&batch).unwrap_err().message.contains("its note"),
            "a batch says why with its note"
        );
        for broken in ["", "   ", "two\nlines", &"x".repeat(201)] {
            let mut turn = turn.clone();
            turn.comment = Some(broken.to_string());
            assert_eq!(
                commented(&turn).unwrap_err().code,
                ErrorCode::BadRequest,
                "{broken:?}"
            );
        }
        let mut longest = turn;
        longest.comment = Some("é".repeat(200));
        assert!(commented(&longest).is_ok(), "200 characters, not bytes");
    }

    #[test]
    fn watchers_read_moments_and_seats_may_go() {
        assert!(refusal(&Request::Tree { recent: None }, false, true).is_none());
        for request in [
            Request::Mark {
                name: "dawn".into(),
            },
            Request::Go {
                moment: "dawn".into(),
            },
        ] {
            assert_eq!(
                refusal(&request, false, true).unwrap().code,
                ErrorCode::NotAllowed
            );
            assert!(refusal(&request, false, false).is_none());
        }
    }
}
