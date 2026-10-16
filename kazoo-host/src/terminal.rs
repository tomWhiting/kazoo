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
//!
//! A program that catches the panics of its other threads itself asks for
//! [`Panics::ThisThread`]: a panic elsewhere then neither restores the
//! terminal nor writes over the screen, and the program says it.

use std::fmt;
use std::io::{self, IsTerminal, Stdout};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once, PoisonError};
use std::thread::{self, ThreadId};

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
/// The thread whose panics alone put the terminal back, when the program
/// asked for [`Panics::ThisThread`].
static OWNER: Mutex<Option<ThreadId>> = Mutex::new(None);

/// What a program asks of the terminal beyond raw mode and the alternate
/// screen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Capture the mouse.
    pub mouse: bool,
    /// Ask for key-release events, which instruments need for note-offs.
    pub key_release_events: bool,
    /// Whose panics put the terminal back.
    pub panics: Panics,
}

/// Whose panics put the terminal back while it is entered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Panics {
    /// A panic on any thread puts the terminal back and is reported.
    #[default]
    AnyThread,
    /// Only a panic on the thread that entered the terminal does. A panic on
    /// another thread is neither restored for nor reported over the screen:
    /// the program catches it on that thread and says it.
    ThisThread,
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
    mouse_refused: Option<String>,
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

    /// Why the terminal refused mouse capture, if it did.
    #[must_use]
    pub fn mouse_refused(&self) -> Option<&str> {
        self.mouse_refused.as_deref()
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
    *OWNER.lock().unwrap_or_else(PoisonError::into_inner) = match options.panics {
        Panics::AnyThread => None,
        Panics::ThisThread => Some(thread::current().id()),
    };
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
    let (mouse, mouse_refused) = if options.mouse {
        match capture_mouse(&mut stdout) {
            Ok(()) => (true, None),
            Err(error) => (false, Some(error.to_string())),
        }
    } else {
        (false, None)
    };
    MOUSE.store(mouse, Ordering::Release);
    match ratatui::Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(inner) => Ok(Terminal {
            inner,
            releases,
            mouse,
            mouse_refused,
        }),
        Err(error) => Err(undone(Error::Backend(error))),
    }
}

fn capture_mouse(stdout: &mut impl io::Write) -> io::Result<()> {
    execute!(stdout, EnableMouseCapture)
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
    let left = leave_into(&mut stdout);
    let raw = disable_raw_mode();
    let cursor = execute!(stdout, Show);
    left.and(raw).and(cursor)
}

/// Write what leaves the screen: the keyboard flags popped and the mouse
/// let go if they were taken, then the alternate screen left. Every step is
/// attempted; the first failure is returned.
fn leave_into(out: &mut impl io::Write) -> io::Result<()> {
    let pop = if PUSHED.swap(false, Ordering::AcqRel) {
        execute!(out, PopKeyboardEnhancementFlags)
    } else {
        Ok(())
    };
    let mouse = if MOUSE.swap(false, Ordering::AcqRel) {
        execute!(out, DisableMouseCapture)
    } else {
        Ok(())
    };
    let screen = execute!(out, LeaveAlternateScreen);
    pop.and(mouse).and(screen)
}

/// Whether a panic on the current thread is another thread's, while the
/// terminal is entered for a program that catches those itself.
fn elsewhere() -> bool {
    ACTIVE.load(Ordering::Acquire)
        && OWNER
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|owner| owner != thread::current().id())
}

/// Restore the terminal before a panic is reported, then report it as the
/// hook before would; a panic on another thread of a program that asked for
/// [`Panics::ThisThread`] is left to that program. Installed once per
/// process.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if elsewhere() {
                return;
            }
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
    fn mouse_capture_failure_keeps_the_reason() {
        #[derive(Debug)]
        struct Refused;
        impl io::Write for Refused {
            fn write(&mut self, _bytes: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("mouse capture refused"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let error = capture_mouse(&mut Refused).expect_err("mouse capture is refused");
        assert!(error.to_string().contains("mouse capture refused"));
    }

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
                panics: Panics::AnyThread,
            }
        );
    }

    #[test]
    fn leaving_lets_go_of_the_mouse_it_took() {
        MOUSE.store(true, Ordering::Release);
        let mut out = Vec::new();
        leave_into(&mut out).expect("written to memory");
        let written = String::from_utf8(out).expect("escape codes are text");
        assert!(written.contains("?1000l"), "{written:?}");
        assert!(written.contains("?1003l"), "{written:?}");
        assert!(written.contains("?1049l"), "{written:?}");
        let mut again = Vec::new();
        leave_into(&mut again).expect("written to memory");
        let again = String::from_utf8(again).expect("escape codes are text");
        assert!(!again.contains("?1000l"), "let go once: {again:?}");
    }

    #[test]
    fn another_threads_panic_is_the_programs_only_when_asked() {
        assert!(!elsewhere(), "nothing is entered");
        ACTIVE.store(true, Ordering::Release);
        *OWNER.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread::current().id());
        assert!(!elsewhere(), "this thread entered");
        let other = thread::spawn(elsewhere).join().expect("the thread ran");
        assert!(other, "another thread's panic is the program's");
        *OWNER.lock().unwrap_or_else(PoisonError::into_inner) = None;
        let other = thread::spawn(elsewhere).join().expect("the thread ran");
        assert!(!other, "any thread's panic restores");
        ACTIVE.store(false, Ordering::Release);
    }
}
