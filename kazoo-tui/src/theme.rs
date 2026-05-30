//! Visual theme: color palette, text styles, and panel styling.
//!
//! The palette evokes a 1970s recording studio: warm espresso backgrounds,
//! brushed-metal silver text, burnished gold accents, and vintage VU meter
//! colours. All values are RGB hex triplets for true-color terminals.

use ratatui::style::{Color, Modifier, Style};

// ---------------------------------------------------------------------------
// Background / surface — warm dark tones
// ---------------------------------------------------------------------------

/// Deep espresso background for the entire terminal.
pub const BG_PRIMARY: Color = Color::Rgb(0x1A, 0x14, 0x10);

/// Warm dark brown for secondary panels.
pub const BG_SECONDARY: Color = Color::Rgb(0x24, 0x1C, 0x16);

/// Walnut surface for list items, input fields, and clip backgrounds.
pub const BG_SURFACE: Color = Color::Rgb(0x2E, 0x24, 0x1C);

/// Coffee-toned elevated surface for popups and overlays.
pub const BG_OVERLAY: Color = Color::Rgb(0x38, 0x2C, 0x22);

/// Background for odd-numbered track lanes (alternating with primary).
pub const BG_LANE_ODD: Color = Color::Rgb(0x20, 0x18, 0x12);

// ---------------------------------------------------------------------------
// Foreground / text — warm silver and cream
// ---------------------------------------------------------------------------

/// Warm cream primary text (high contrast against espresso).
pub const FG_PRIMARY: Color = Color::Rgb(0xE8, 0xDE, 0xD0);

/// Dusty silver secondary text (labels, descriptions).
pub const FG_SECONDARY: Color = Color::Rgb(0xA8, 0x9C, 0x8C);

/// Warm gray dimmed text (disabled items, decorative).
pub const FG_DIMMED: Color = Color::Rgb(0x6E, 0x62, 0x56);

/// Tarnished silver for unfocused panel borders.
pub const BORDER_NORMAL: Color = Color::Rgb(0x5A, 0x50, 0x44);

/// Burnished gold for focused panel borders.
pub const BORDER_FOCUS: Color = Color::Rgb(0xD4, 0xA0, 0x40);

// ---------------------------------------------------------------------------
// Accent / state colours — warm analogue palette
// ---------------------------------------------------------------------------

/// Vintage red for recording indicators (tube glow).
pub const ACCENT_RECORD: Color = Color::Rgb(0xE0, 0x44, 0x30);

/// Olive green for playing / safe levels (classic VU).
pub const ACCENT_PLAY: Color = Color::Rgb(0x7C, 0xB0, 0x50);

/// Warm off-white for stopped state.
pub const ACCENT_STOP: Color = Color::Rgb(0xC0, 0xB4, 0xA4);

/// Amber for paused state (warm tube glow).
pub const ACCENT_PAUSE: Color = Color::Rgb(0xE0, 0xA8, 0x30);

/// Burnished gold for keyboard focus highlight.
pub const ACCENT_FOCUS: Color = Color::Rgb(0xD4, 0xA0, 0x40);

/// Amber for selected item highlight.
pub const ACCENT_SELECTED: Color = Color::Rgb(0xE0, 0xA8, 0x30);

/// Vintage red for errors and warnings.
pub const ACCENT_ERROR: Color = Color::Rgb(0xE0, 0x44, 0x30);

// ---------------------------------------------------------------------------
// Meter colours — classic VU needle progression
// ---------------------------------------------------------------------------

/// Meter level: safe / nominal (olive green).
pub const METER_GREEN: Color = Color::Rgb(0x7C, 0xB0, 0x50);

/// Meter level: caution / approaching clip (warm amber).
pub const METER_YELLOW: Color = Color::Rgb(0xE0, 0xA8, 0x30);

/// Meter level: clipping / hot (vintage red).
pub const METER_RED: Color = Color::Rgb(0xE0, 0x44, 0x30);

// ---------------------------------------------------------------------------
// Track colours (8 earth tones + warm metals for visual differentiation)
// ---------------------------------------------------------------------------

/// Track colour palette — cycled via [`track_color`].
const TRACK_COLORS: [Color; 8] = [
    Color::Rgb(0xD4, 0xA0, 0x40), // burnished gold
    Color::Rgb(0xC0, 0x64, 0x40), // rust / copper
    Color::Rgb(0x7C, 0xB0, 0x50), // sage green
    Color::Rgb(0xE0, 0xA8, 0x30), // warm amber
    Color::Rgb(0xA0, 0x80, 0xB0), // dusty mauve
    Color::Rgb(0x60, 0x9C, 0x90), // verdigris / patina
    Color::Rgb(0xD0, 0x90, 0x60), // tan / sandstone
    Color::Rgb(0xB0, 0x70, 0x60), // terracotta
];

// ---------------------------------------------------------------------------
// Public helpers
// ---------------------------------------------------------------------------

/// Return the track colour for a given track index (wraps every 8).
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

/// Return the meter colour for a given linear ratio (0.0 = silence, 1.0+ = clipping).
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

/// Return the meter colour for a dB value.
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

/// Style for an active view tab in the header.
#[must_use]
pub const fn style_view_tab_active() -> Style {
    Style::new()
        .fg(BG_PRIMARY)
        .bg(ACCENT_FOCUS)
        .add_modifier(Modifier::BOLD)
}

/// Style for an inactive view tab in the header.
#[must_use]
pub const fn style_view_tab_inactive() -> Style {
    Style::new().fg(FG_SECONDARY).bg(BG_SECONDARY)
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

    #[test]
    fn view_tab_styles_differ() {
        let active = style_view_tab_active();
        let inactive = style_view_tab_inactive();
        assert_ne!(active.fg, inactive.fg);
    }

    #[test]
    fn lane_bg_alternates() {
        assert_eq!(lane_bg(0), BG_PRIMARY);
        assert_eq!(lane_bg(1), BG_LANE_ODD);
        assert_eq!(lane_bg(2), BG_PRIMARY);
    }
}
