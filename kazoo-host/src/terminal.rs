//! The terminal every kazoo program draws on.
//!
//! [`enter_with`] puts the terminal into raw mode on the alternate screen,
//! asks for key-release events and mouse capture when the program wants
//! them, and installs (once per process) a panic hook that puts the
//! terminal back before the panic is reported. [`Terminal`] puts it back
//! when dropped, and [`Terminal::leave`] does so and says if it failed.
//! Whichever comes first does the work; the others find nothing to do, so
//! a panic during the restore, or a drop after a leave, never runs it
//! twice.

use std::fmt;
use std::io::{self, IsTerminal, Stdout};
use std::ops::{Deref, DerefMut};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::Show;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::backend::CrosstermBackend;

/// The backend every program draws through.
pub type Backend = CrosstermBackend<Stdout>;

/// Whether the terminal is in the state [`enter_with`] set up: the one
/// guard every way out goes through.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Whether keyboard enhancement flags were pushed and must be popped.
static PUSHED: AtomicBool = AtomicBool::new(false);
/// Whether mouse capture was enabled and must be disabled.
static MOUSE: AtomicBool = AtomicBool::new(false);

/// What a program asks of the terminal beyond raw mode and the alternate
/// screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Capture the mouse.
    pub mouse: bool,
    /// Ask for key-release events, which instruments need for note-offs.
    pub key_release_events: bool,
}

/// Whether the terminal reports key releases, or why it does not.
///
/// Without releases a note-off never arrives from the keyboard, so notes
/// latch; instruments say so on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyReleases {
    /// The program did not ask for them.
    NotAsked,
    /// The terminal reports them.
    Reported,
    /// The terminal has no keyboard enhancement protocol.
    Unsupported,
    /// Support could not be detected or enabled; the reason is kept.
    Failed(String),
}

/// What failed while entering the terminal, with the error underneath.
#[derive(Debug)]
pub enum Error {
    /// Standard output is not a terminal.
    NoTty,
    /// Raw mode could not be entered.
    RawMode(io::Error),
    /// The alternate screen could not be entered.
    AlternateScreen(io::Error),
    /// The drawing backend could not be set up.
    Backend(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTty => f.write_str("no tty: standard output is not a terminal"),
            Self::RawMode(error) => write!(f, "could not enter raw mode: {error}"),
            Self::AlternateScreen(error) => {
                write!(f, "could not enter the alternate screen: {error}")
            }
            Self::Backend(error) => write!(f, "could not set up the terminal backend: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoTty => None,
            Self::RawMode(error) | Self::AlternateScreen(error) | Self::Backend(error) => {
                Some(error)
            }
        }
    }
}

/// The terminal a program draws on. It derefs to the ratatui terminal, and
/// puts the real terminal back when dropped.
#[derive(Debug)]
pub struct Terminal {
    inner: ratatui::Terminal<Backend>,
    releases: KeyReleases,
    mouse: bool,
}

impl Terminal {
    /// Whether key releases are reported, or why not.
    #[must_use]
    pub const fn key_releases(&self) -> &KeyReleases {
        &self.releases
    }

    /// Whether the mouse was asked for and captured.
    #[must_use]
    pub const fn mouse_captured(&self) -> bool {
        self.mouse
    }

    /// Put the terminal back now and say whether every step worked.
    ///
    /// # Errors
    ///
    /// The first step of the restore that failed; every step is attempted.
    pub fn leave(self) -> io::Result<()> {
        restore()
    }
}

impl Deref for Terminal {
    type Target = ratatui::Terminal<Backend>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Terminal {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        if let Err(error) = restore() {
            eprintln!("kazoo: could not fully restore the terminal: {error}");
        }
    }
}

/// [`enter_with`] with no mouse and no key-release events.
///
/// # Errors
///
/// As [`enter_with`].
pub fn enter() -> Result<Terminal, Error> {
    enter_with(Options::default())
}

/// Enter raw mode and the alternate screen, with what `options` asks for.
///
/// A terminal that refuses key releases or the mouse is still entered;
/// [`Terminal::key_releases`] and [`Terminal::mouse_captured`] say what was
/// granted. Whatever was set up before a failure is undone first.
///
/// # Errors
///
/// [`Error::NoTty`] when standard output is not a terminal, and otherwise
/// the step that failed.
pub fn enter_with(options: Options) -> Result<Terminal, Error> {
    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        return Err(Error::NoTty);
    }
    install_panic_hook();
    enable_raw_mode().map_err(Error::RawMode)?;
    ACTIVE.store(true, Ordering::Release);
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        return Err(undone(Error::AlternateScreen(error)));
    }
    let releases = if options.key_release_events {
        ask_for_releases(&mut stdout)
    } else {
        KeyReleases::NotAsked
    };
    let mouse = options.mouse && execute!(stdout, EnableMouseCapture).is_ok();
    MOUSE.store(mouse, Ordering::Release);
    match ratatui::Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(inner) => Ok(Terminal {
            inner,
            releases,
            mouse,
        }),
        Err(error) => Err(undone(Error::Backend(error))),
    }
}

fn ask_for_releases(stdout: &mut Stdout) -> KeyReleases {
    match supports_keyboard_enhancement() {
        Ok(true) => match execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        ) {
            Ok(()) => {
                PUSHED.store(true, Ordering::Release);
                KeyReleases::Reported
            }
            Err(error) => KeyReleases::Failed(format!("could not enable key releases: {error}")),
        },
        Ok(false) => KeyReleases::Unsupported,
        Err(error) => KeyReleases::Failed(format!("could not query the keyboard: {error}")),
    }
}

/// Undo a partial entry, keeping the entry's error and noting a restore
/// failure on standard error.
fn undone(error: Error) -> Error {
    if let Err(restore_error) = restore() {
        eprintln!("kazoo: {error}, and restoring the terminal failed too: {restore_error}");
    }
    error
}

/// Put the terminal back if it is still entered. Only the first caller
/// does the work.
///
/// # Errors
///
/// The first step that failed; every step is attempted.
pub fn restore() -> io::Result<()> {
    restore_once(&ACTIVE, undo)
}

/// Run `undo` if `active` was set, clearing it first, so of any number of
/// callers (a drop, a leave, the panic hook) exactly one restores.
fn restore_once(active: &AtomicBool, undo: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    if active.swap(false, Ordering::AcqRel) {
        undo()
    } else {
        Ok(())
    }
}

fn undo() -> io::Result<()> {
    let mut stdout = io::stdout();
    let pop = if PUSHED.swap(false, Ordering::AcqRel) {
        execute!(stdout, PopKeyboardEnhancementFlags)
    } else {
        Ok(())
    };
    let mouse = if MOUSE.swap(false, Ordering::AcqRel) {
        execute!(stdout, DisableMouseCapture)
    } else {
        Ok(())
    };
    let screen = execute!(stdout, LeaveAlternateScreen);
    let raw = disable_raw_mode();
    let cursor = execute!(stdout, Show);
    pop.and(mouse).and(screen).and(raw).and(cursor)
}

/// Restore the terminal before a panic is reported, then report it as the
/// hook before would. Installed once per process.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Err(error) = restore() {
                eprintln!("kazoo: could not restore the terminal: {error}");
            }
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn restore_runs_once_even_if_asked_twice() {
        let active = AtomicBool::new(true);
        let runs = AtomicUsize::new(0);
        let count = || {
            runs.fetch_add(1, Ordering::Relaxed);
            Ok(())
        };
        restore_once(&active, count).expect("first restore");
        restore_once(&active, count).expect("second restore");
        assert_eq!(runs.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn enter_without_a_tty_is_an_error_in_words() {
        assert!(
            !io::stdout().is_terminal(),
            "the test runner gives tests a pipe, not a terminal"
        );
        let error = enter().expect_err("a pipe is not a terminal");
        assert!(error.to_string().contains("tty"), "{error}");
    }

    #[test]
    fn options_default_to_no_mouse_and_no_release_events() {
        assert_eq!(
            Options::default(),
            Options {
                mouse: false,
                key_release_events: false,
            }
        );
    }
}
