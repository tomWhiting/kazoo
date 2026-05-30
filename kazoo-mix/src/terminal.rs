//! Robust terminal lifecycle helpers for `kazoo-mix`.
//!
//! Terminal state must be restored even if the app exits through an error or a
//! panic. Keep this local to `kazoo-mix`; `kazoo-core` stays UI-free.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use color_eyre::Result;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

/// Concrete terminal backend used by kazoo-mix.
pub type MixTerminal = Terminal<CrosstermBackend<io::Stdout>>;

/// Idempotent RAII guard for raw-mode / alternate-screen terminal state.
#[derive(Debug)]
pub struct TerminalGuard {
    terminal: Option<MixTerminal>,
    restored: AtomicBool,
}

impl TerminalGuard {
    /// Enter the alternate-screen terminal and install a best-effort panic hook.
    #[must_use]
    pub fn enter() -> Self {
        install_panic_restore_hook();
        let terminal = ratatui::init();
        Self {
            terminal: Some(terminal),
            restored: AtomicBool::new(false),
        }
    }

    /// Mutable terminal access while the guard is active.
    pub const fn terminal_mut(&mut self) -> &mut MixTerminal {
        self.terminal.as_mut().expect("terminal already restored")
    }

    /// Restore terminal state. Safe to call multiple times.
    pub fn restore(&mut self) -> Result<()> {
        if !self.restored.swap(true, Ordering::AcqRel) {
            self.terminal.take();
            ratatui::try_restore()?;
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn install_panic_restore_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = ratatui::try_restore();
        default_hook(info);
    }));
}
