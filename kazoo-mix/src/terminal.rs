//! Robust terminal lifecycle helpers for `kazoo-mix`.
//!
//! Terminal state must be restored even if the app exits through an error or a
//! panic. Keep this local to `kazoo-mix`; `kazoo-core` stays UI-free.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use color_eyre::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Concrete terminal backend used by kazoo-mix.
pub type MixTerminal = Terminal<CrosstermBackend<io::Stdout>>;

/// Idempotent RAII guard for raw-mode / alternate-screen terminal state.
#[derive(Debug)]
pub struct TerminalGuard {
    terminal: Option<MixTerminal>,
    mouse: bool,
    restored: AtomicBool,
}

impl TerminalGuard {
    /// Enter the alternate-screen terminal with mouse capture and install a
    /// panic hook that restores the terminal.
    ///
    /// A terminal that refuses mouse capture still gets a fully
    /// keyboard-driven desk; [`Self::mouse_enabled`] says which, and the desk
    /// shows it in the header.
    #[must_use]
    pub fn enter() -> Self {
        install_panic_restore_hook();
        let terminal = ratatui::init();
        let mouse = execute!(io::stdout(), EnableMouseCapture).is_ok();
        Self {
            terminal: Some(terminal),
            mouse,
            restored: AtomicBool::new(false),
        }
    }

    /// Whether the terminal accepted mouse capture.
    #[must_use]
    pub const fn mouse_enabled(&self) -> bool {
        self.mouse
    }

    /// Mutable terminal access while the guard is active; `None` once the
    /// terminal has been restored.
    pub const fn terminal_mut(&mut self) -> Option<&mut MixTerminal> {
        self.terminal.as_mut()
    }

    /// Restore terminal state. Safe to call multiple times.
    ///
    /// Both steps are always attempted; the first failure is returned.
    pub fn restore(&mut self) -> Result<()> {
        if !self.restored.swap(true, Ordering::AcqRel) {
            self.terminal.take();
            return restore_terminal();
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Err(err) = self.restore() {
            eprintln!("kazoo-mix: could not restore the terminal: {err}");
        }
    }
}

/// Release mouse capture and leave the alternate screen, attempting both
/// even if the first fails.
fn restore_terminal() -> Result<()> {
    let mouse = execute!(io::stdout(), DisableMouseCapture);
    let screen = ratatui::try_restore();
    screen?;
    mouse?;
    Ok(())
}

fn install_panic_restore_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Err(err) = restore_terminal() {
            eprintln!("kazoo-mix: could not restore the terminal: {err}");
        }
        default_hook(info);
    }));
}
