//! The console: `kazoo-wall` with no argument.
//!
//! It starts a detached daemon when none is playing, then shows the wall
//! live (knobs moving, cables, meters, who is doing what) and lets Tom
//! change everything. It is a client of the daemon's control socket like
//! any seat, as `Tom` with the console role; blocking socket work runs on
//! the [`worker`] threads, so the screen never waits on the socket. When
//! the daemon restarts, the console reconnects on its own and says so.
//!
//! - [`app`] holds the state and says what every key does.
//! - [`draw`] draws the screen.
//! - [`worker`] runs the connections to the daemon.
//! - [`link`] paces reconnection.
//! - [`log`] keeps the live log.
//! - [`layout`] hangs the module panels.
//! - [`knob`] works out knob travel and values.
//! - [`theme`] holds the colours and glyphs.
//! - [`terminal`] takes over the terminal and gives it back.

mod app;
mod draw;
mod dye;
mod knob;
mod layout;
mod link;
mod log;
mod rack;
mod terminal;
mod theme;
mod worker;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as TermEvent, KeyEventKind};
use ratatui::DefaultTerminal;

use kazoo_wall::{daemon, paths};

use app::{App, Exit, unix_now};
use terminal::TerminalGuard;
use worker::{Config, Connection};

/// The seat the console speaks as.
pub const SEAT: &str = "Tom";

/// The console software, as it says hello.
pub const CLIENT: &str = concat!("kazoo-wall console ", env!("CARGO_PKG_VERSION"));

/// How often the console looks at the wall (knob motion and meters).
pub const POLL: Duration = Duration::from_millis(100);

/// Longest wait between frames.
const FRAME: Duration = Duration::from_millis(50);

/// How long a daemon started from here may take to answer.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);

/// The file a daemon started from here writes to.
const DAEMON_LOG: &str = "daemon.log";

/// Open the console: start the daemon if it is not playing, show the wall,
/// and give the terminal back however it ends.
pub fn run() -> ExitCode {
    let socket = paths::socket_path();
    let launch = match launcher() {
        Ok(launch) => launch,
        Err(err) => {
            eprintln!("kazoo-wall: {err}");
            return ExitCode::FAILURE;
        }
    };
    if !daemon::is_running(&socket) {
        eprintln!("kazoo-wall: the wall is not playing; starting it…");
        if let Err(err) = daemon::launch_detached(&launch.0, &socket, &launch.1, LAUNCH_TIMEOUT) {
            eprintln!("kazoo-wall: the wall could not start: {err}");
            return ExitCode::FAILURE;
        }
    }
    let config = Config {
        socket: socket.clone(),
        seat: SEAT.to_string(),
        client: CLIENT.to_string(),
        console: true,
        poll: POLL,
        launch: Some(launch),
    };
    let connection = match Connection::start(&config) {
        Ok(connection) => connection,
        Err(err) => {
            eprintln!("kazoo-wall: the console could not start its connection: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mut app = App::new(SEAT, socket.display().to_string());
    let mut guard = match TerminalGuard::enter() {
        Ok(guard) => guard,
        Err(err) => {
            eprintln!("kazoo-wall: the terminal could not be used: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Some(why) = guard.mouse_refused() {
        app.mouse_refused(why, Instant::now());
    }
    let result = guard.terminal_mut().map_or_else(
        || Err("the terminal was given back before the console started".to_string()),
        |terminal| console(terminal, &mut app, &connection),
    );
    let restored = guard.restore();
    drop(connection);
    if let Err(err) = restored {
        eprintln!("kazoo-wall: could not restore the terminal: {err}");
    }
    match result {
        Ok(Exit::Left) => {
            eprintln!(
                "kazoo-wall: the console is closed and the wall keeps playing \
                 (`kazoo-wall` opens it again, `kazoo-wall stop` stops it)"
            );
            ExitCode::SUCCESS
        }
        Ok(Exit::Stopped) => {
            eprintln!("kazoo-wall: the wall has stopped; its patch is saved");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("kazoo-wall: {err}");
            ExitCode::FAILURE
        }
    }
}

/// This program and the file a daemon started from here writes to.
fn launcher() -> Result<(PathBuf, PathBuf), String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("cannot find this program to start the wall with: {err}"))?;
    let state =
        paths::state_dir().map_err(|err| format!("no place for the wall's state: {err}"))?;
    Ok((exe, daemon_log(&state)))
}

fn daemon_log(state: &Path) -> PathBuf {
    state.join(DAEMON_LOG)
}

/// The console's loop: take in replies, send jobs, draw, and wait for a
/// key until the next frame.
fn console(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    connection: &Connection,
) -> Result<Exit, String> {
    loop {
        let now = Instant::now();
        while let Some(reply) = connection.try_reply()? {
            app.on_reply(reply, now);
        }
        app.tick(now);
        for job in app.take_jobs() {
            connection.send(job)?;
        }
        if let Some(exit) = app.exit() {
            return Ok(exit);
        }
        terminal
            .draw(|frame| draw::draw(frame, app, now, unix_now()))
            .map_err(|err| format!("the screen could not be drawn: {err}"))?;
        let ready = event::poll(FRAME).map_err(|err| format!("the keyboard failed: {err}"))?;
        if ready {
            match event::read().map_err(|err| format!("the keyboard failed: {err}"))? {
                TermEvent::Key(key) if key.kind != KeyEventKind::Release => {
                    app.handle_key(key, Instant::now());
                }
                TermEvent::Mouse(mouse) => app.handle_mouse(mouse, Instant::now()),
                // Resizes redraw on the next frame; focus and pastes change
                // nothing.
                TermEvent::Key(_)
                | TermEvent::Resize(..)
                | TermEvent::FocusGained
                | TermEvent::FocusLost
                | TermEvent::Paste(_) => {}
            }
        }
    }
}
