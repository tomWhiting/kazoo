//! Visual theme: color palette, text styles, and panel styling.
//!
//! All colors are specified as RGB hex triplets for true-color terminals.
//! The palette is designed for readability on dark backgrounds and follows
//! the Catppuccin Mocha aesthetic.

use ratatui::style::{Color, Modifier, Style};

// ---------------------------------------------------------------------------
// Background / surface
// ---------------------------------------------------------------------------

/// Deep navy background for the entire terminal.
pub const BG_PRIMARY: Color = Color::Rgb(0x1E, 0x1E, 0x2E);

/// Slightly lighter surface for secondary panels.
#[allow(dead_code)]
pub const BG_SECONDARY: Color = Color::Rgb(0x28, 0x28, 0x3C);

/// Surface color for list items, input fields, and clip backgrounds.
pub const BG_SURFACE: Color = Color::Rgb(0x31, 0x31, 0x4A);

/// Elevated surface for popups and overlays.
pub const BG_OVERLAY: Color = Color::Rgb(0x3A, 0x3A, 0x55);

/// Background for odd-numbered track lanes (alternating with primary).
pub const BG_LANE_ODD: Color = Color::Rgb(0x24, 0x24, 0x38);

// ---------------------------------------------------------------------------
// Foreground / text
// ---------------------------------------------------------------------------

/// Primary text color (high contrast).
pub const FG_PRIMARY: Color = Color::Rgb(0xCD, 0xD6, 0xF4);

/// Secondary text color (labels, descriptions).
pub const FG_SECONDARY: Color = Color::Rgb(0x9C, 0x9C, 0xB0);

/// Dimmed text (disabled items, decorative).
pub const FG_DIMMED: Color = Color::Rgb(0x6C, 0x70, 0x86);

/// Border color for unfocused panels.
pub const BORDER_NORMAL: Color = Color::Rgb(0x45, 0x47, 0x5A);

/// Border color for focused panels.
pub const BORDER_FOCUS: Color = Color::Rgb(0x89, 0xB4, 0xFA);

// ---------------------------------------------------------------------------
// Accent / state colors
// ---------------------------------------------------------------------------

/// Recording indicator (pulsing pink).
pub const ACCENT_RECORD: Color = Color::Rgb(0xF3, 0x8B, 0xA8);

/// Playing indicator (green).
pub const ACCENT_PLAY: Color = Color::Rgb(0xA6, 0xE3, 0xA1);

/// Stopped state (off-white).
pub const ACCENT_STOP: Color = Color::Rgb(0xBA, 0xC2, 0xDE);

/// Paused state (amber).
pub const ACCENT_PAUSE: Color = Color::Rgb(0xF9, 0xE2, 0xAF);

/// Focus highlight (blue).
pub const ACCENT_FOCUS: Color = Color::Rgb(0x89, 0xB4, 0xFA);

/// Selected item highlight (amber).
pub const ACCENT_SELECTED: Color = Color::Rgb(0xF9, 0xE2, 0xAF);

/// Error / warning indicator.
pub const ACCENT_ERROR: Color = Color::Rgb(0xF3, 0x8B, 0xA8);

// ---------------------------------------------------------------------------
// Meter colors
// ---------------------------------------------------------------------------

/// Meter level: safe / nominal (green).
pub const METER_GREEN: Color = Color::Rgb(0xA6, 0xE3, 0xA1);

/// Meter level: caution / approaching clip (yellow).
pub const METER_YELLOW: Color = Color::Rgb(0xF9, 0xE2, 0xAF);

/// Meter level: clipping / hot (red).
pub const METER_RED: Color = Color::Rgb(0xF3, 0x8B, 0xA8);

// ---------------------------------------------------------------------------
// Track colors (8 distinct hues for visual differentiation)
// ---------------------------------------------------------------------------

/// Track color palette — cycled via [`track_color`].
const TRACK_COLORS: [Color; 8] = [
    Color::Rgb(0x89, 0xB4, 0xFA), // steel blue
    Color::Rgb(0xF3, 0x8B, 0xA8), // soft red
    Color::Rgb(0xA6, 0xE3, 0xA1), // leaf green
    Color::Rgb(0xF9, 0xE2, 0xAF), // amber
    Color::Rgb(0xCB, 0xA6, 0xF7), // lavender
    Color::Rgb(0x94, 0xE2, 0xD5), // teal
    Color::Rgb(0xFA, 0xB3, 0x87), // peach
    Color::Rgb(0xEB, 0xA0, 0xAC), // brick / rose
];

// ---------------------------------------------------------------------------
// Public helpers
// ---------------------------------------------------------------------------

/// Return the track color for a given track index (wraps every 8).
#[must_use]
pub const fn track_color(index: usize) -> Color {
    TRACK_COLORS[index % TRACK_COLORS.len()]
}

/// Return the lane background for a track index (alternating even/odd).
#[must_use]
pub const fn lane_bg(index: usize) -> Color {
    if index % 2 == 0 {
        BG_PRIMARY
    } else {
        BG_LANE_ODD
    }
}

/// Return the meter color for a given linear ratio (0.0 = silence, 1.0+ = clipping).
///
/// - `[0.0, 0.7)` — green
/// - `[0.7, 0.9)` — yellow
/// - `[0.9, ...)` — red
#[must_use]
pub const fn meter_color(ratio: f32) -> Color {
    if ratio >= 0.9 {
        METER_RED
    } else if ratio >= 0.7 {
        METER_YELLOW
    } else {
        METER_GREEN
    }
}

/// Return the meter color for a dB value.
///
/// - `< -6 dB` — green
/// - `[-6 dB, -1 dB)` — yellow
/// - `>= -1 dB` — red
#[must_use]
pub const fn meter_color_db(db: f32) -> Color {
    if db >= -1.0 {
        METER_RED
    } else if db >= -6.0 {
        METER_YELLOW
    } else {
        METER_GREEN
    }
}

/// Style for a panel border, distinguished by whether the panel has focus.
#[must_use]
pub const fn style_panel_border(focused: bool) -> Style {
    if focused {
        Style::new().fg(BORDER_FOCUS)
    } else {
        Style::new().fg(BORDER_NORMAL)
    }
}

/// Style for a panel title, distinguished by focus state.
#[must_use]
pub const fn style_panel_title(focused: bool) -> Style {
    if focused {
        Style::new().fg(ACCENT_FOCUS).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_SECONDARY)
    }
}

/// Style for primary body text.
#[must_use]
pub const fn style_text() -> Style {
    Style::new().fg(FG_PRIMARY)
}

/// Style for secondary / descriptive text.
#[must_use]
pub const fn style_text_secondary() -> Style {
    Style::new().fg(FG_SECONDARY)
}

/// Style for dimmed / disabled text.
#[must_use]
pub const fn style_text_dimmed() -> Style {
    Style::new().fg(FG_DIMMED)
}

/// Style for a selected list item.
#[must_use]
pub const fn style_selected() -> Style {
    Style::new()
        .fg(BG_PRIMARY)
        .bg(ACCENT_SELECTED)
        .add_modifier(Modifier::BOLD)
}

/// Style for the recording indicator (pulsing effect controlled by caller).
#[must_use]
pub const fn style_recording(visible: bool) -> Style {
    if visible {
        Style::new().fg(ACCENT_RECORD).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_DIMMED)
    }
}

/// Style for the play indicator.
#[must_use]
pub const fn style_playing() -> Style {
    Style::new().fg(ACCENT_PLAY).add_modifier(Modifier::BOLD)
}

/// Style for the stopped indicator.
#[must_use]
pub const fn style_stopped() -> Style {
    Style::new().fg(ACCENT_STOP)
}

/// Style for the paused indicator.
#[must_use]
pub const fn style_paused() -> Style {
    Style::new().fg(ACCENT_PAUSE).add_modifier(Modifier::BOLD)
}

/// Style for a muted track indicator ("M").
#[must_use]
pub const fn style_muted() -> Style {
    Style::new().fg(ACCENT_ERROR).add_modifier(Modifier::BOLD)
}

/// Style for a soloed track indicator ("S").
#[must_use]
pub const fn style_soloed() -> Style {
    Style::new()
        .fg(ACCENT_SELECTED)
        .add_modifier(Modifier::BOLD)
}

/// Style for an armed track indicator ("R").
#[must_use]
pub const fn style_armed() -> Style {
    Style::new().fg(ACCENT_RECORD).add_modifier(Modifier::BOLD)
}

/// Style for a track name, colored by track index.
#[must_use]
pub const fn style_track_name(index: usize) -> Style {
    Style::new().fg(track_color(index))
}

/// Style for a parameter name.
#[must_use]
#[allow(dead_code)]
pub const fn style_param_name() -> Style {
    Style::new().fg(FG_SECONDARY)
}

/// Style for a parameter value.
#[must_use]
#[allow(dead_code)]
pub const fn style_param_value() -> Style {
    Style::new().fg(FG_PRIMARY).add_modifier(Modifier::BOLD)
}

/// Style for a parameter value currently being edited.
#[must_use]
#[allow(dead_code)]
pub const fn style_param_editing() -> Style {
    Style::new()
        .fg(ACCENT_FOCUS)
        .add_modifier(Modifier::BOLD.union(Modifier::UNDERLINED))
}

/// Style for the help overlay background.
#[must_use]
pub const fn style_help_bg() -> Style {
    Style::new().bg(BG_OVERLAY).fg(FG_PRIMARY)
}

/// Style for help overlay key labels.
#[must_use]
pub const fn style_help_key() -> Style {
    Style::new().fg(ACCENT_FOCUS).add_modifier(Modifier::BOLD)
}

/// Style for help overlay descriptions.
#[must_use]
pub const fn style_help_desc() -> Style {
    Style::new().fg(FG_SECONDARY)
}

/// Style for the filled portion of a slider bar.
#[must_use]
pub const fn style_slider_filled() -> Style {
    Style::new().fg(ACCENT_FOCUS)
}

/// Style for the empty portion of a slider bar.
#[must_use]
pub const fn style_slider_empty() -> Style {
    Style::new().fg(FG_DIMMED)
}

/// Style for a drawer section header.
#[must_use]
pub const fn style_drawer_header() -> Style {
    Style::new().fg(FG_PRIMARY).add_modifier(Modifier::BOLD)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_colors_wrap_around() {
        for i in 0..8 {
            let c = track_color(i);
            assert_eq!(c, TRACK_COLORS[i]);
        }
        assert_eq!(track_color(8), track_color(0));
        assert_eq!(track_color(15), track_color(7));
    }

    #[test]
    fn meter_color_thresholds() {
        assert_eq!(meter_color(0.0), METER_GREEN);
        assert_eq!(meter_color(0.5), METER_GREEN);
        assert_eq!(meter_color(0.69), METER_GREEN);
        assert_eq!(meter_color(0.7), METER_YELLOW);
        assert_eq!(meter_color(0.85), METER_YELLOW);
        assert_eq!(meter_color(0.9), METER_RED);
        assert_eq!(meter_color(1.0), METER_RED);
        assert_eq!(meter_color(1.5), METER_RED);
    }

    #[test]
    fn meter_color_db_thresholds() {
        assert_eq!(meter_color_db(-20.0), METER_GREEN);
        assert_eq!(meter_color_db(-6.0), METER_YELLOW);
        assert_eq!(meter_color_db(-3.0), METER_YELLOW);
        assert_eq!(meter_color_db(-1.0), METER_RED);
        assert_eq!(meter_color_db(0.0), METER_RED);
        assert_eq!(meter_color_db(3.0), METER_RED);
    }

    #[test]
    fn style_panel_border_focused_vs_not() {
        let focused = style_panel_border(true);
        let unfocused = style_panel_border(false);
        assert_ne!(focused.fg, unfocused.fg);
    }

    #[test]
    fn style_recording_visibility() {
        let visible = style_recording(true);
        let hidden = style_recording(false);
        assert_ne!(visible.fg, hidden.fg);
    }
}
