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
//! [`Panics::ThisThread`]: the hook then neither restores the terminal for a
//! panic on another thread nor reports it, from entering until the terminal
//! is wholly put back. The program must catch it on that thread and say it.

use std::cell::Cell;
use std::fmt;
use std::io::{self, IsTerminal, Stdout};
use std::ops::{Deref, DerefMut};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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
/// The entry whose thread alone puts the terminal back on a panic, when the
/// program asked for [`Panics::ThisThread`]; 0 for none. Each such entry
/// takes a fresh token, so a thread that owned an earlier entry owns
/// nothing now.
static OWNER: AtomicU64 = AtomicU64::new(0);
/// The last token handed out.
static TOKENS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The token of the entry this thread made, 0 if none.
    static OWNS: Cell<u64> = const { Cell::new(0) };
}

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
    /// Only a panic on the thread that entered the terminal does. For a panic
    /// on another thread the hook neither restores nor reports, from entering
    /// until the terminal is wholly put back: the program must catch it on
    /// that thread and say it.
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
    enable_raw_mode().map_err(Error::RawMode)?;
    own(options.panics);
    ACTIVE.store(true, Ordering::Release);
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        return Err(undone(Error::AlternateScreen(error)));
    }
    let releases = if options.key_release_events {
        ask_for_releases(&mut stdout)
    } else {
        KeyReleases::NotAsked
    };
    // Marked before it is asked for: a capture that fails part way may have
    // turned some of its modes on, and turning off modes never turned on is
    // harmless.
    MOUSE.store(options.mouse, Ordering::Release);
    let (mouse, mouse_refused) = if options.mouse {
        match capture_mouse(&mut stdout) {
            Ok(()) => (true, None),
            Err(error) => (false, Some(error.to_string())),
        }
    } else {
        (false, None)
    };
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
    // Only now is the screen wholly back: until here another thread's panic
    // would still write over it.
    OWNER.store(0, Ordering::Release);
    left.and(raw).and(cursor)
}

/// Take the panics for this thread when `panics` asks for that, or give
/// them to any thread.
fn own(panics: Panics) {
    let token = match panics {
        Panics::AnyThread => 0,
        Panics::ThisThread => TOKENS.fetch_add(1, Ordering::AcqRel) + 1,
    };
    // A thread being torn down has no locals left: it owns nothing.
    if OWNS.try_with(|owns| owns.set(token)).is_err() {
        OWNER.store(0, Ordering::Release);
        return;
    }
    OWNER.store(token, Ordering::Release);
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

/// Whether a panic on the current thread is another thread's, while a
/// program that catches those itself holds the terminal.
fn elsewhere() -> bool {
    let owner = OWNER.load(Ordering::Acquire);
    owner != 0 && OWNS.try_with(Cell::get).unwrap_or(0) != owner
}

/// What the hook did with a panic.
#[derive(Debug, PartialEq, Eq)]
enum Panicked {
    /// Another thread's, left to the program.
    LeftToTheProgram,
    /// The terminal was put back (or was not entered), and the panic goes on
    /// to be reported; the restore's failure, if it failed.
    Reported(Option<String>),
}

/// Decide a panic: another thread's is left alone, and any other puts the
/// terminal back with `restore` before it is reported.
fn on_panic(elsewhere: bool, restore: impl FnOnce() -> io::Result<()>) -> Panicked {
    if elsewhere {
        Panicked::LeftToTheProgram
    } else {
        Panicked::Reported(restore().err().map(|error| error.to_string()))
    }
}

/// Restore the terminal before a panic is reported, then report it as the
/// hook before would; a panic on another thread of a program that asked for
/// [`Panics::ThisThread`] is left to that program. Installed once per
/// process.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| match on_panic(elsewhere(), restore) {
            Panicked::LeftToTheProgram => {}
            Panicked::Reported(failed) => {
                if let Some(error) = failed {
                    eprintln!("kazoo: could not restore the terminal: {error}");
                }
                previous(info);
            }
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::thread;

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
    fn leaving_lets_go_of_what_it_took_once_and_always_leaves_the_screen() {
        MOUSE.store(true, Ordering::Release);
        PUSHED.store(true, Ordering::Release);
        let mut out = Vec::new();
        leave_into(&mut out).expect("written to memory");
        let written = String::from_utf8(out).expect("escape codes are text");
        assert!(written.contains("?1000l"), "{written:?}");
        assert!(written.contains("?1003l"), "{written:?}");
        assert!(written.contains("?1049l"), "{written:?}");
        // The keyboard flags popped: CSI < 1 u.
        assert!(written.contains("\u{1b}[<1u"), "{written:?}");
        let mut again = Vec::new();
        leave_into(&mut again).expect("written to memory");
        let again = String::from_utf8(again).expect("escape codes are text");
        assert!(
            !again.contains("?1000l"),
            "the mouse let go once: {again:?}"
        );
        assert!(!again.contains("[<1u"), "the flags popped once: {again:?}");
        assert!(
            again.contains("?1049l"),
            "the screen left each time: {again:?}"
        );
    }

    #[test]
    fn another_threads_panic_is_the_programs_only_while_it_holds_the_terminal() {
        assert!(!elsewhere(), "nothing is owned");
        own(Panics::ThisThread);
        assert!(!elsewhere(), "this thread entered");
        let other = thread::spawn(elsewhere).join().expect("the thread ran");
        assert!(other, "another thread's panic is the program's");

        // A second entry by another thread owns it now; this one no longer.
        thread::spawn(|| own(Panics::ThisThread))
            .join()
            .expect("the thread ran");
        assert!(elsewhere(), "an earlier owner owns nothing");

        // Put back wholly: every thread's panic restores and is reported.
        own(Panics::ThisThread);
        OWNER.store(0, Ordering::Release);
        let other = thread::spawn(elsewhere).join().expect("the thread ran");
        assert!(!other, "after leaving, any thread's panic is reported");

        own(Panics::AnyThread);
        let other = thread::spawn(elsewhere).join().expect("the thread ran");
        assert!(!other, "any thread's panic restores");
    }

    #[test]
    fn a_panic_is_left_restored_or_reported_with_its_failure() {
        let restores = AtomicUsize::new(0);
        let fine = || {
            restores.fetch_add(1, Ordering::Relaxed);
            Ok(())
        };
        assert_eq!(on_panic(true, fine), Panicked::LeftToTheProgram);
        assert_eq!(
            restores.load(Ordering::Relaxed),
            0,
            "left alone: no restore"
        );
        assert_eq!(on_panic(false, fine), Panicked::Reported(None));
        assert_eq!(restores.load(Ordering::Relaxed), 1);
        let failing = || Err(io::Error::other("the tty is gone"));
        assert_eq!(
            on_panic(false, failing),
            Panicked::Reported(Some("the tty is gone".to_owned()))
        );
    }
}
