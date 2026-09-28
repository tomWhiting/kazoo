//! Terminal setup and teardown.
//!
//! Every change made to the terminal is undone on the way out, even when
//! setup fails half-way or the event loop returns an error.

use std::io;

use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// The terminal type the app draws on.
pub type Tui = Terminal<CrosstermBackend<io::Stdout>>;

/// Whether the terminal reports key releases, or why it does not.
///
/// Without releases a note-off never arrives from the keyboard, so notes
/// latch; the UI says so and points at the all-notes-off key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyReleases {
    Reported,
    /// The terminal has no keyboard enhancement protocol.
    Unsupported,
    /// Support could not be detected or enabled; the reason is shown.
    Failed(String),
}

impl KeyReleases {
    /// Whether keyboard enhancement flags were pushed and must be popped.
    #[must_use]
    pub const fn pushed(&self) -> bool {
        matches!(self, Self::Reported)
    }
}

/// Enter raw mode and the alternate screen and ask for key-release events.
/// On failure, whatever was already changed is undone before returning.
pub fn setup_terminal() -> color_eyre::Result<(Tui, KeyReleases)> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    if let Err(error) = execute!(stdout, EnterAlternateScreen) {
        return Err(undo_setup(error, false, false));
    }
    let keyboard = match terminal::supports_keyboard_enhancement() {
        Ok(true) => match execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        ) {
            Ok(()) => KeyReleases::Reported,
            Err(error) => KeyReleases::Failed(format!("could not enable key releases: {error}")),
        },
        Ok(false) => KeyReleases::Unsupported,
        Err(error) => KeyReleases::Failed(format!("could not query the keyboard: {error}")),
    };
    let pushed = keyboard.pushed();
    match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => Ok((terminal, keyboard)),
        Err(error) => Err(undo_setup(error, true, pushed)),
    }
}

/// Undo a partial [`setup_terminal`], attaching any cleanup failure to `error`.
fn undo_setup(error: io::Error, alternate: bool, pushed: bool) -> color_eyre::Report {
    let mut stdout = io::stdout();
    let pop = if pushed {
        execute!(stdout, PopKeyboardEnhancementFlags)
    } else {
        Ok(())
    };
    let leave = if alternate {
        execute!(stdout, LeaveAlternateScreen)
    } else {
        Ok(())
    };
    let raw = terminal::disable_raw_mode();
    let report = color_eyre::Report::new(error).wrap_err("could not set up the terminal");
    match pop.and(leave).and(raw) {
        Ok(()) => report,
        Err(cleanup_error) => report.wrap_err(format!(
            "and could not restore it afterwards: {cleanup_error}"
        )),
    }
}

/// Undo every terminal change. Every step is attempted even if an earlier one
/// fails; the first failure is returned.
pub fn restore_terminal(terminal: &mut Tui, keyboard: &KeyReleases) -> io::Result<()> {
    let pop = if keyboard.pushed() {
        execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags)
    } else {
        Ok(())
    };
    let raw = terminal::disable_raw_mode();
    let leave = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let cursor = terminal.show_cursor();
    pop.and(raw).and(leave).and(cursor)
}

/// Combine the event loop's result with the terminal restore result. The
/// loop error wins; a restore failure alongside it is still reported.
pub fn finish(
    app_name: &str,
    result: color_eyre::Result<()>,
    restored: io::Result<()>,
) -> color_eyre::Result<()> {
    match (result, restored) {
        (Ok(()), restored) => restored.map_err(Into::into),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(restore_error)) => Err(error.wrap_err(format!(
            "{app_name}: could not fully restore the terminal either: {restore_error}"
        ))),
    }
}
