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

use kazoo_core::ipc::link::{HubAddress, HubLink, LinkConfig, hub_link};

use super::DeskMode;
use super::audio::headless_lead_frames;
use super::audio::{AudioMode, Device, Headless, StreamHealth};
use super::server::{Outgoing, ToControl, line};
use super::wall::Wall;
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

/// Audio glitch and error counts are reported at most this often.
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
        glitches: u64,
        errors: u64,
    },
    Headless(Headless),
}

/// The audio side: where the engine runs, and its listening thread.
struct Backend {
    running: Running,
    listen_stop: Arc<AtomicBool>,
    listen_join: Option<JoinHandle<()>>,
    /// When to try the device again, while standing in for it.
    retry_at: Option<Instant>,
    /// When the device's counters were last reported.
    reported: Instant,
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
}

/// Run the control thread until stopped.
pub fn run(setup: Setup) {
    let (listen_tx, listen_rx) = std::sync::mpsc::channel();
    let (backend, engine, link) = start_audio(
        setup.audio,
        &setup.desk,
        (setup.tempo, 0.0),
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
    if setup.accept.join().is_err() {
        eprintln!("kazoo-wall: the accept thread panicked");
    }
}

/// Start the engine on the device (or the headless loop), with its desk
/// link and listening thread, at `(bpm, beat)`, heard or silent.
/// `failed_before` is when the device last failed, if it did (the failure
/// is then not reported again).
fn start_audio(
    mode: AudioMode,
    desk: &DeskMode,
    (bpm, beat): (f64, f64),
    heard: bool,
    listen_tx: &Sender<Listen>,
    failed_before: Option<Instant>,
) -> (Backend, EngineControl, Option<HubLink>) {
    let bpm = crate::engine::clamp_bpm(bpm);
    if let AudioMode::Device { sample_rate } = mode {
        match Device::open(sample_rate) {
            Ok(device) => {
                let rate = device.sample_rate();
                let link = link_to_desk(desk, rate, Pacing::Device);
                let (hub, link) = split_link(link);
                let (engine, mut control) = engine(engine_config(rate, (bpm, beat), heard), hub);
                let health = Arc::new(StreamHealth::default());
                match device.play(engine, &health) {
                    Ok(stream) => {
                        eprintln!("kazoo-wall: playing on {} at {rate} Hz", device.name());
                        let backend = backend(
                            Running::Device {
                                _stream: stream,
                                health,
                                glitches: 0,
                                errors: 0,
                            },
                            &mut control,
                            listen_tx,
                            None,
                        );
                        return (backend, control, link);
                    }
                    Err(why) => eprintln!("kazoo-wall: {why}; retrying in 2 s"),
                }
            }
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
    let link = link_to_desk(desk, rate, Pacing::Timer);
    let (hub, link) = split_link(link);
    let (engine, mut control) = engine(engine_config(rate, (bpm, beat), heard), hub);
    let retry_at = matches!(mode, AudioMode::Device { .. }).then(|| Instant::now() + DEVICE_RETRY);
    let running = match Headless::start(engine, rate) {
        Ok(headless) => Running::Headless(headless),
        Err(err) => {
            // Without a thread there is nothing to render on: the next
            // housekeeping round sees it finished and tries again.
            eprintln!("kazoo-wall: the render thread could not start: {err}");
            Running::Headless(Headless::stopped())
        }
    };
    (
        backend(running, &mut control, listen_tx, retry_at),
        control,
        link,
    )
}

/// An engine at `rate` carrying on from `(bpm, beat)`, heard or silent
/// from its first frame.
fn engine_config(rate: u32, (bpm, beat): (f64, f64), heard: bool) -> EngineConfig {
    EngineConfig {
        audible: heard,
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
        reported: Instant::now(),
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
    if pacing == Pacing::Timer {
        config.pace_lead_frames = headless_lead_frames(rate);
    }
    match hub_link(config) {
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
            (request, Some(_)) if watcher && request.is_change() => Response::failure(
                id,
                WallError::new(
                    ErrorCode::NotAllowed,
                    format!("a watcher cannot {}; it only watches", request.op()),
                ),
            ),
            (request, Some(seat)) => match self.wall.request(&seat, request, Instant::now()) {
                Ok(outcome) => {
                    if let Some(ticket) = outcome.pending {
                        // Answered when it is ready (see `housekeeping`).
                        self.awaiting.insert(ticket, (conn, id));
                        return;
                    }
                    self.announce(&seat, outcome.change, outcome.event);
                    Response {
                        id,
                        ok: true,
                        result: Some(outcome.result),
                        error: None,
                    }
                }
                Err(err) => Response::failure(id, err),
            },
        };
        self.send(conn, &response);
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
        for (ticket, answer) in self.wall.spoken() {
            let asker = self.awaiting.remove(&ticket);
            let response = match answer {
                Ok(outcome) => {
                    let seat = outcome.change.as_ref().map(|change| change.seat.clone());
                    if let Some(seat) = seat {
                        self.announce(&seat, outcome.change.clone(), outcome.event);
                    }
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

    fn hello(
        &mut self,
        conn: u64,
        id: u64,
        seat: &str,
        client: &str,
        console: bool,
        watcher: bool,
    ) -> Response {
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
        let now = Instant::now();
        self.last_tick = now;
        while let Ok(listen) = self.listen_rx.try_recv() {
            self.wall.heard(listen);
        }
        for event in self.wall.tick(now) {
            self.broadcast(&event, None);
        }
        self.answer_speech();
        self.watch_audio(now);
    }

    /// Rebuild the audio when the device failed, or try the device again
    /// while the headless loop stands in for it.
    fn watch_audio(&mut self, now: Instant) {
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        let report = now.saturating_duration_since(backend.reported) >= REPORT_INTERVAL;
        if report {
            backend.reported = now;
        }
        let rebuild = match &mut backend.running {
            Running::Device {
                health,
                glitches,
                errors,
                ..
            } => {
                if report {
                    report_count(&health.glitches, glitches, "audio glitches");
                    report_count(&health.errors, errors, "audio device errors");
                }
                let failed = health.failed.load(Ordering::Acquire);
                if failed {
                    eprintln!("kazoo-wall: the audio device went away; rebuilding in 2 s");
                }
                failed
            }
            Running::Headless(headless) => {
                let died = headless.finished();
                if died {
                    eprintln!("kazoo-wall: the render loop stopped; restarting it");
                }
                died || backend.retry_at.is_some_and(|at| now >= at)
            }
        };
        if rebuild {
            self.rebuild(now);
        }
    }

    fn rebuild(&mut self, now: Instant) {
        let shared = Arc::clone(self.wall.engine_mut().shared());
        let (bpm, beat) = (shared.bpm(), shared.beat());
        let device_failed = matches!(
            self.backend.as_ref().map(|b| &b.running),
            Some(Running::Device { .. })
        );
        // The old engine (and its half of the desk link) stops first.
        self.backend = None;
        let (backend, engine, link) = if device_failed {
            // Stand in with the headless loop now; the device is tried again
            // in two seconds.
            let rate = FALLBACK_RATE;
            let link = link_to_desk(&self.desk, rate, Pacing::Timer);
            let (hub, link) = split_link(link);
            let (engine, mut control) = engine(engine_config(rate, (bpm, beat), self.heard), hub);
            let running = match Headless::start(engine, rate) {
                Ok(headless) => Running::Headless(headless),
                Err(err) => {
                    eprintln!("kazoo-wall: the render thread could not start: {err}");
                    Running::Headless(Headless::stopped())
                }
            };
            let backend = backend(
                running,
                &mut control,
                &self.listen_tx,
                Some(now + DEVICE_RETRY),
            );
            (backend, control, link)
        } else {
            eprintln!("kazoo-wall: trying the audio device again");
            start_audio(
                self.audio,
                &self.desk,
                (bpm, beat),
                self.heard,
                &self.listen_tx,
                Some(now),
            )
        };
        self.wall.attach(engine, link);
        self.backend = Some(backend);
    }

    /// Stop: finish a recording under way, tell every connection, save,
    /// and let the audio go.
    fn finish(&mut self) {
        let now = Instant::now();
        if let Some(event) = self.wall.end_recording() {
            self.broadcast(&event, None);
        }
        self.wall.save(now);
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
        self.backend = None;
        eprintln!("kazoo-wall: stopped; the patch is saved");
    }
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

/// Report a counter that has grown since `seen`.
fn report_count(counter: &std::sync::atomic::AtomicU64, seen: &mut u64, what: &str) {
    let now = counter.load(Ordering::Relaxed);
    if now != *seen {
        eprintln!("kazoo-wall: {now} {what} so far");
        *seen = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_or_rebuilt_engine_is_heard_as_the_wall_was() {
        for heard in [false, true] {
            let config = engine_config(48_000, (96.0, 12.5), heard);
            assert_eq!(config.audible, heard);
            assert_eq!(config.sample_rate, 48_000);
            assert!((config.bpm - 96.0).abs() < f64::EPSILON);
            assert!((config.beat - 12.5).abs() < f64::EPSILON);
            let (_engine, control) = engine(config, None);
            assert_eq!(control.shared().audible(), heard);
        }
    }
}
