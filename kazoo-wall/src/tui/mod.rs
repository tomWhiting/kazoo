//! The console: `kazoo wall` with no argument.
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
//! - [`kazoo_host::terminal`] takes over the terminal and gives it back.

mod app;
pub mod clock;
mod draw;
mod dye;
mod knob;
mod layout;
mod link;
mod log;
mod rack;
mod theme;
mod worker;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event as TermEvent, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::daemon::{self, ServeCommand};
use crate::paths;

use app::{App, Exit, unix_now};
use worker::{Config, Connection};

/// The seat the console speaks as.
pub const SEAT: &str = "Tom";

/// The console software, as it says hello.
pub const CLIENT: &str = concat!("kazoo-wall console ", env!("CARGO_PKG_VERSION"));

/// How often the console looks at the wall (knob motion and meters).
pub const POLL: Duration = Duration::from_millis(100);

/// Longest wait between frames: sixty a second, for the scopes.
const FRAME: Duration = Duration::from_millis(16);

/// How long a daemon started from here may take to answer.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);

/// The file a daemon started from here writes to.
const DAEMON_LOG: &str = "daemon.log";

/// Open the console: start the daemon if it is not playing, show the wall,
/// and give the terminal back however it ends.
pub fn run(serve: ServeCommand) -> color_eyre::Result<()> {
    let socket = paths::socket_path();
    let launch = launcher(serve).map_err(|err| color_eyre::eyre::eyre!("{err}"))?;
    if !daemon::is_running(&socket) {
        eprintln!("kazoo-wall: the wall is not playing; starting it…");
        daemon::launch_detached(&launch.0, &socket, &launch.1, LAUNCH_TIMEOUT)
            .map_err(|err| color_eyre::eyre::eyre!("the wall could not start: {err}"))?;
    }
    let config = Config {
        socket: socket.clone(),
        seat: SEAT.to_string(),
        client: CLIENT.to_string(),
        console: true,
        poll: POLL,
        launch: Some(launch),
    };
    let connection = Connection::start(&config).map_err(|err| {
        color_eyre::eyre::eyre!("the console could not start its connection: {err}")
    })?;
    let mut app = App::new(SEAT, socket.display().to_string());
    // A settings file that cannot be read is said in the log, and the
    // console starts with the defaults (and does not write over it unless
    // a setting changes).
    let (prefs_file, refused) = match app::prefs_path() {
        Ok(file) => {
            let refused = match app::load_prefs(&file) {
                Ok(prefs) => {
                    app.apply_prefs(prefs);
                    None
                }
                Err(why) => Some(why),
            };
            (Some(file), refused)
        }
        Err(why) => (None, Some(why)),
    };
    let mut guard = kazoo_host::terminal::enter_with(kazoo_host::terminal::Options {
        mouse: true,
        key_release_events: false,
    })
    .map_err(|err| color_eyre::eyre::eyre!("the terminal could not be used: {err}"))?;
    if let Some(why) = guard.mouse_refused() {
        app.mouse_refused(why, Instant::now());
    }
    if let Some(why) = refused {
        app.prefs_trouble(why, Instant::now());
    }
    let result = console(&mut guard, &mut app, &connection, prefs_file.as_deref());
    let restored = guard.leave();
    drop(connection);
    match (result, restored) {
        (Err(err), Err(restore)) => Err(color_eyre::eyre::eyre!(
            "{err}; could not restore the terminal: {restore}"
        )),
        (Err(err), Ok(())) => Err(color_eyre::eyre::eyre!("{err}")),
        (Ok(_), Err(err)) => Err(color_eyre::eyre::eyre!(
            "could not restore the terminal: {err}"
        )),
        (Ok(exit), Ok(())) => {
            match exit {
                Exit::Left => eprintln!(
                    "kazoo-wall: the console is closed and the wall keeps playing \
                     (`kazoo wall` opens it again, `kazoo wall stop` stops it)"
                ),
                Exit::Stopped => eprintln!("kazoo-wall: the wall has stopped; its patch is saved"),
            }
            Ok(())
        }
    }
}

/// The supplied command and the file a daemon started from here writes to.
fn launcher(serve: ServeCommand) -> Result<(ServeCommand, PathBuf), String> {
    let state =
        paths::state_dir().map_err(|err| format!("no place for the wall's state: {err}"))?;
    Ok((serve, daemon_log(&state)))
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
    prefs_file: Option<&Path>,
) -> Result<Exit, String> {
    let mut remembered = app.prefs();
    loop {
        if let Some(file) = prefs_file {
            let prefs = app.prefs();
            if prefs != remembered {
                remembered = prefs;
                if let Err(why) = app::save_prefs(file, &remembered) {
                    app.prefs_trouble(why, Instant::now());
                }
            }
        }
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
