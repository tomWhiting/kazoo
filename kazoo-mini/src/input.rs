//! Keyboard-to-note mapping and input handling.
//!
//! QWERTY keyboard mapped to chromatic notes.
//! Navigation: Tab between sections, arrows within, +/- adjust.

use crossterm::event::{KeyCode, KeyModifiers};

use crate::app::App;

// ---------------------------------------------------------------------------
// QWERTY-to-MIDI note mapping
// ---------------------------------------------------------------------------

/// Map a QWERTY key to a MIDI note number.
///
/// Layout (two-row chromatic, starting at C3):
/// ```text
///  W E   T Y U   O P
/// A S D F G H J K L ;
/// ```
/// Bottom row = white keys (C3 to E4).
/// Top row = black keys (sharps/flats).
/// Navigation uses arrow keys only (no j/k) to keep the full chromatic
/// keyboard available.
#[must_use]
pub const fn key_to_midi_note(code: KeyCode) -> Option<u8> {
    // Base octave: C3 = MIDI 48
    match code {
        // Bottom row: white keys C3 through E4
        KeyCode::Char('a') => Some(48),       // C3
        KeyCode::Char('s') => Some(50),       // D3
        KeyCode::Char('d') => Some(52),       // E3
        KeyCode::Char('f') => Some(53),       // F3
        KeyCode::Char('g') => Some(55),       // G3
        KeyCode::Char('h') => Some(57),       // A3
        KeyCode::Char('j') => Some(59),       // B3
        KeyCode::Char('k' | 'z') => Some(60), // C4
        KeyCode::Char('l' | 'x') => Some(62), // D4
        KeyCode::Char(';' | 'c') => Some(64), // E4
        // Top row: black keys
        KeyCode::Char('w') => Some(49), // C#3
        KeyCode::Char('e') => Some(51), // D#3
        KeyCode::Char('t') => Some(54), // F#3
        KeyCode::Char('y') => Some(56), // G#3
        KeyCode::Char('u') => Some(58), // A#3
        KeyCode::Char('o') => Some(61), // C#4
        KeyCode::Char('p') => Some(63), // D#4
        // Extended: another octave higher
        KeyCode::Char('v') => Some(65), // F4
        KeyCode::Char('b') => Some(67), // G4
        KeyCode::Char('n') => Some(69), // A4 (440 Hz)
        KeyCode::Char('m') => Some(71), // B4
        _ => None,
    }
}

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
