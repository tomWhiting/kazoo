//! Keyboard-to-note mapping and input handling.
//!
//! QWERTY keyboard mapped to chromatic notes.
//! Navigation: Tab between sections, arrows within, +/- adjust.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::app::App;

// ---------------------------------------------------------------------------
// Input event handling
// ---------------------------------------------------------------------------

/// Handle a key event. Returns true if the event was consumed.
pub fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    // Quit
    if code == KeyCode::Char('q') && modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return true;
    }
    if code == KeyCode::Esc {
        app.should_quit = true;
        return true;
    }

    // Section navigation
    if code == KeyCode::Tab {
        if modifiers.contains(KeyModifiers::SHIFT) {
            app.prev_section();
        } else {
            app.next_section();
        }
        return true;
    }

    // Parameter navigation (arrow keys only — j/k are musical keys)
    match code {
        KeyCode::Down => {
            app.next_param();
            return true;
        }
        KeyCode::Up => {
            app.prev_param();
            return true;
        }
        _ => {}
    }

    // Parameter adjustment
    match code {
        KeyCode::Char('+' | '=') | KeyCode::Right => {
            app.adjust_param(1.0);
            return true;
        }
        KeyCode::Char('-' | '_') | KeyCode::Left => {
            app.adjust_param(-1.0);
            return true;
        }
        _ => {}
    }

    false
}
