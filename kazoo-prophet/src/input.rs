//! Keyboard mapping for the Prophet TUI.

use crossterm::event::KeyCode;

/// Map a QWERTY key to a two-octave keyboard starting at C3.
#[must_use]
pub const fn key_to_note(code: KeyCode) -> Option<u8> {
    match code {
        KeyCode::Char('z') => Some(48),
        KeyCode::Char('s') => Some(49),
        KeyCode::Char('x') => Some(50),
        KeyCode::Char('d') => Some(51),
        KeyCode::Char('c') => Some(52),
        KeyCode::Char('v') => Some(53),
        KeyCode::Char('g') => Some(54),
        KeyCode::Char('b') => Some(55),
        KeyCode::Char('h') => Some(56),
        KeyCode::Char('n') => Some(57),
        KeyCode::Char('j') => Some(58),
        KeyCode::Char('m') => Some(59),
        KeyCode::Char(',' | 'q') => Some(60),
        KeyCode::Char('l' | '2') => Some(61),
        KeyCode::Char('.' | 'w') => Some(62),
        KeyCode::Char(';' | '3') => Some(63),
        KeyCode::Char('/' | 'e') => Some(64),
        KeyCode::Char('r') => Some(65),
        KeyCode::Char('5') => Some(66),
        KeyCode::Char('t') => Some(67),
        KeyCode::Char('6') => Some(68),
        KeyCode::Char('y') => Some(69),
        KeyCode::Char('7') => Some(70),
        KeyCode::Char('u') => Some(71),
        KeyCode::Char('i') => Some(72),
        _ => None,
    }
}
