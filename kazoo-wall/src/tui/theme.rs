//! The console's look: kazoo-mix's analogue desk carried onto the wall.
//! Cream module panels with black knobs hang on a dark rack, and every
//! cable has its own colour, the same on its jacks and in the cable list.

use ratatui::style::{Color, Modifier, Style};

use kazoo_wall::catalogue::Signal;

/// The rack the panels hang on.
pub const RACK: Color = Color::Rgb(0x17, 0x14, 0x12);
/// Header, sidebar and popups.
pub const PANEL: Color = Color::Rgb(0x24, 0x1E, 0x19);
/// A focused row on a dark panel.
pub const PANEL_FOCUS: Color = Color::Rgb(0x30, 0x28, 0x21);
/// Text on dark panels.
pub const TEXT: Color = Color::Rgb(0xE7, 0xDC, 0xCB);
/// Quiet text on dark panels.
pub const TEXT_DIM: Color = Color::Rgb(0x96, 0x87, 0x74);
/// Engraving and focus.
pub const BRASS: Color = Color::Rgb(0xCF, 0xA2, 0x47);
/// Good news.
pub const SAGE: Color = Color::Rgb(0x87, 0xB3, 0x61);
/// Attention.
pub const AMBER: Color = Color::Rgb(0xDE, 0xA2, 0x34);
/// Trouble.
pub const RED: Color = Color::Rgb(0xDE, 0x58, 0x45);
/// Quiet chrome.
pub const STEEL: Color = Color::Rgb(0x73, 0x6B, 0x62);
/// A module's face.
pub const CREAM: Color = Color::Rgb(0xEA, 0xDF, 0xC4);
/// The selected knob's row on a module's face.
pub const CREAM_FOCUS: Color = Color::Rgb(0xD8, 0xC8, 0x9E);
/// Printing on a module's face.
pub const INK: Color = Color::Rgb(0x2B, 0x22, 0x1A);
/// Quiet printing on a module's face.
pub const INK_DIM: Color = Color::Rgb(0x7A, 0x6A, 0x55);
/// A knob cap.
pub const KNOB: Color = Color::Rgb(0x14, 0x12, 0x10);
/// The pointer on a knob cap.
pub const POINTER: Color = Color::Rgb(0xF4, 0xEE, 0xE0);
/// An empty jack on a module's face.
pub const JACK_EMPTY: Color = Color::Rgb(0x8C, 0x7D, 0x68);
/// Meter background.
pub const METER_BG: Color = Color::Rgb(0x10, 0x0E, 0x0C);

/// Patch-cable colours, chosen to read on both the cream faces and the dark
/// sidebar. A cable keeps its colour for its whole life (by its number).
pub const CABLES: [Color; 10] = [
    Color::Rgb(0xC8, 0x3A, 0x2C),
    Color::Rgb(0x2F, 0x6F, 0xB8),
    Color::Rgb(0xD9, 0x8A, 0x1C),
    Color::Rgb(0x2E, 0x8B, 0x57),
    Color::Rgb(0x8E, 0x44, 0xAD),
    Color::Rgb(0xC2, 0x18, 0x5B),
    Color::Rgb(0x16, 0x8F, 0x8F),
    Color::Rgb(0xB0, 0x8D, 0x10),
    Color::Rgb(0x5D, 0x6D, 0x7E),
    Color::Rgb(0x8B, 0x5A, 0x2B),
];

/// The colour of cable number `id`.
#[must_use]
pub const fn cable_colour(id: u32) -> Color {
    CABLES[id as usize % CABLES.len()]
}

/// Knob pointer from fully anticlockwise (7 o'clock) to fully clockwise
/// (5 o'clock), as on the desk.
const KNOB_POINTERS: [char; 7] = ['↙', '←', '↖', '↑', '↗', '→', '↘'];

/// The pointer for a knob at `position` (0 to 1) along its travel.
#[must_use]
pub fn knob_pointer(position: f64) -> char {
    let position = if position.is_finite() {
        position.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let index = (position * (KNOB_POINTERS.len() - 1) as f64).round() as usize;
    KNOB_POINTERS[index.min(KNOB_POINTERS.len() - 1)]
}

/// A jack's glyph: audio jacks are round, gates square, CV diamonds;
/// filled when a cable is in them.
#[must_use]
pub const fn jack_glyph(signal: Signal, plugged: bool) -> char {
    match (signal, plugged) {
        (Signal::Audio, true) => '●',
        (Signal::Audio, false) => '○',
        (Signal::Gate, true) => '■',
        (Signal::Gate, false) => '□',
        (Signal::Cv, true) => '◆',
        (Signal::Cv, false) => '◇',
    }
}

/// Bold text in `colour` on `background`.
#[must_use]
pub const fn bold(colour: Color, background: Color) -> Style {
    Style::new()
        .fg(colour)
        .bg(background)
        .add_modifier(Modifier::BOLD)
}

/// Plain text in `colour` on `background`.
#[must_use]
pub const fn plain(colour: Color, background: Color) -> Style {
    Style::new().fg(colour).bg(background)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knob_pointers_sweep_from_seven_to_five_oclock() {
        assert_eq!(knob_pointer(0.0), '↙');
        assert_eq!(knob_pointer(0.5), '↑');
        assert_eq!(knob_pointer(1.0), '↘');
        assert_eq!(knob_pointer(f64::NAN), '↙');
        assert_eq!(knob_pointer(7.0), '↘');
    }

    #[test]
    fn cables_keep_their_colour_and_jacks_show_their_signal() {
        assert_eq!(cable_colour(3), cable_colour(13));
        assert_ne!(cable_colour(3), cable_colour(4));
        assert_eq!(jack_glyph(Signal::Audio, true), '●');
        assert_eq!(jack_glyph(Signal::Gate, false), '□');
        assert_eq!(jack_glyph(Signal::Cv, true), '◆');
    }
}
