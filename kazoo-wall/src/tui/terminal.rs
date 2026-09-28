//! The terminal's raw mode, alternate screen and mouse capture, restored
//! however the console ends: a normal exit, an error, or a panic (the panic
//! hook restores the terminal before the panic message prints, so it is
//! readable). A terminal that refuses the mouse still gets the whole
//! console from the keyboard.

use std::io;
use std::sync::Once;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use ratatui::DefaultTerminal;

/// Raw mode and the alternate screen, left when dropped.
#[derive(Debug)]
pub struct TerminalGuard {
    terminal: Option<DefaultTerminal>,
    mouse_refused: Option<String>,
}

impl TerminalGuard {
    /// Take over the terminal, with a panic hook that gives it back.
    ///
    /// # Errors
    ///
    /// Fails if the terminal cannot enter raw mode or the alternate
    /// screen; the terminal is restored before returning.
    pub fn enter() -> io::Result<Self> {
        install_panic_restore_hook();
        match ratatui::try_init() {
            Ok(terminal) => {
                let mouse_refused = match execute!(io::stdout(), EnableMouseCapture) {
                    Ok(()) => None,
                    Err(err) => Some(err.to_string()),
                };
                Ok(Self {
                    terminal: Some(terminal),
                    mouse_refused,
                })
            }
            Err(err) => {
                if let Err(restore) = ratatui::try_restore() {
                    eprintln!("kazoo-wall: could not restore the terminal: {restore}");
                }
                Err(err)
            }
        }
    }

    /// Why the terminal refused mouse capture, if it did.
    #[must_use]
    pub fn mouse_refused(&self) -> Option<&str> {
        self.mouse_refused.as_deref()
    }

    /// The terminal, until it is restored.
    pub const fn terminal_mut(&mut self) -> Option<&mut DefaultTerminal> {
        self.terminal.as_mut()
    }

    /// Give the terminal back. Safe to call more than once.
    ///
    /// # Errors
    ///
    /// Fails if raw mode or the alternate screen cannot be left.
    pub fn restore(&mut self) -> io::Result<()> {
        if self.terminal.take().is_some() {
            return restore_terminal();
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if let Err(err) = self.restore() {
            eprintln!("kazoo-wall: could not restore the terminal: {err}");
        }
    }
}

/// Restore the terminal before any panic message prints; installed once.
fn install_panic_restore_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if let Err(err) = restore_terminal() {
                eprintln!("kazoo-wall: could not restore the terminal: {err}");
            }
            default_hook(info);
        }));
    });
}

/// Release the mouse and leave raw mode and the alternate screen, trying
/// both even if the first fails.
fn restore_terminal() -> io::Result<()> {
    let mouse = execute!(io::stdout(), DisableMouseCapture);
    let screen = ratatui::try_restore();
    screen?;
    mouse
}
