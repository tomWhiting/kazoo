//! UI layout, panels, and custom widgets.
//!
//! The [`draw`] function renders the entire application for one frame,
//! splitting the terminal into transport bar, track list, waveform,
//! spectrum, meters, and inspector panels.

pub mod drawer;
pub mod effects;
pub mod file_browser;
pub mod meters;
pub mod mixer;
pub mod spectrum;
pub mod timeline;
pub mod tracks;
pub mod transport;
pub mod waveform;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, AppMode, FocusedPanel};
use crate::theme;

/// Render the entire application UI for one frame.
///
/// Layout:
/// ```text
/// +-- Transport (3 rows) -----------------------------------------------+
/// +-- Tracks (26c) --+-- Waveform (top 60%) -----+-- Inspector (36c) ---+
/// |                  |                           |  (Effects/Mixer)     |
/// |                  +-- Spectrum (bot 70%) -----+                     |
/// |                  |   Meters (bot 30%)        |                     |
/// +------------------+---------------------------+---------------------+
/// ```
pub fn draw(frame: &mut Frame, app: &mut App) {
    let terminal_area = frame.area();

    // Fill the entire terminal with the primary background.
    frame.render_widget(
        Block::default().style(Style::new().bg(theme::BG_PRIMARY)),
        terminal_area,
    );

    // Main vertical split: transport bar (3 rows) + content area.
    let main_chunks = Layout::vertical([
        Constraint::Length(3), // transport
        Constraint::Min(10),   // content
    ])
    .split(terminal_area);

    transport::draw(frame, app, main_chunks[0]);

    if app.mode == AppMode::SynthDrawer {
        // Drawer layout: tracks (26 cols) | right area (rest).
        let content_chunks =
            Layout::horizontal([Constraint::Length(26), Constraint::Min(40)]).split(main_chunks[1]);

        tracks::draw(frame, app, content_chunks[0]);

        // Right area: waveform/timeline (compressed top) | drawer (bottom).
        let right_chunks = Layout::vertical([
            Constraint::Min(6),         // waveform/timeline compressed
            Constraint::Percentage(60), // drawer
        ])
        .split(content_chunks[1]);

        // Compressed waveform/timeline in the top portion.
        if app.has_clips() || app.focused_panel == FocusedPanel::Timeline {
            timeline::draw(frame, app, right_chunks[0]);
        } else {
            waveform::draw(frame, app, right_chunks[0]);
        }

        // Synth control drawer in the bottom portion.
        drawer::draw(frame, app, right_chunks[1]);
    } else {
        // Normal layout: tracks (26 cols) | center | inspector (36 cols).
        let content_chunks = Layout::horizontal([
            Constraint::Length(26),
            Constraint::Min(20),
            Constraint::Length(36),
        ])
        .split(main_chunks[1]);

        tracks::draw(frame, app, content_chunks[0]);

        // Center area: waveform/timeline (top 60%) | bottom 40%.
        let center_chunks =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(content_chunks[1]);

        // Show timeline when clips exist or Timeline panel is focused;
        // otherwise show the oscilloscope waveform view.
        if app.has_clips() || app.focused_panel == FocusedPanel::Timeline {
            timeline::draw(frame, app, center_chunks[0]);
        } else {
            waveform::draw(frame, app, center_chunks[0]);
        }

        // Bottom center: spectrum (70%) | meters (30%).
        let bottom_chunks =
            Layout::horizontal([Constraint::Percentage(70), Constraint::Percentage(30)])
                .split(center_chunks[1]);

        spectrum::draw(frame, app, bottom_chunks[0]);
        meters::draw(frame, app, bottom_chunks[1]);

        // Inspector: show mixer strip view when Mixer is focused, effects
        // inspector otherwise.
        if app.focused_panel == FocusedPanel::Mixer {
            mixer::draw(frame, app, content_chunks[2]);
        } else {
            effects::draw(frame, app, content_chunks[2]);
        }
    }

    // Help overlay (rendered on top of everything).
    if app.mode == AppMode::Help {
        render_help_overlay(frame, terminal_area);
    }

    // File browser overlay (rendered on top of everything).
    if matches!(app.mode, AppMode::FileBrowser { .. }) {
        file_browser::draw(frame, app, terminal_area);
    }
}

/// Create a focus-aware bordered block for a panel.
///
/// Border and title colors change when the panel has keyboard focus.
#[must_use]
pub fn panel_block<'a>(title: &'a str, panel: FocusedPanel, app: &'a App) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(theme::style_panel_border(app.is_focused(panel)))
        .title(title)
        .title_style(theme::style_panel_title(app.is_focused(panel)))
}

/// Render the help overlay as a centered popup.
fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 70, area);
    frame.render_widget(Clear, popup);

    let help_text = vec![
        Line::from(vec![Span::styled(
            "  Kazoo -- Keyboard Shortcuts",
            theme::style_help_key(),
        )]),
        Line::from(""),
        help_line("Space", "Play / Pause"),
        help_line("s", "Stop"),
        help_line("r", "Record"),
        help_line("R", "Record with count-in"),
        help_line("q", "Quit"),
        help_line("?", "Toggle help"),
        help_line("Tab", "Next panel"),
        help_line("S-Tab", "Prev panel"),
        help_line("j/k", "Select track"),
        help_line("m", "Mute track"),
        help_line("S", "Solo track"),
        help_line("a", "Arm track"),
        help_line("n", "New track"),
        help_line("x", "Delete track"),
        help_line("t", "Cycle synth mode"),
        help_line("\u{2191}/\u{2193}", "Synth/effect nav"),
        help_line("\u{2190}/\u{2192}", "Adjust param"),
        help_line("h/l", "Pan / cycle param"),
        help_line("+/-", "Volume / param"),
        help_line("[/]", "Zoom waveform"),
        help_line("d", "Synth drawer"),
        help_line("n/x", "Add/remove layer (drawer)"),
        help_line("e", "Toggle layer (drawer)"),
        help_line("[/]", "Select layer (drawer)"),
        help_line("o", "Open file browser"),
        help_line(",/.", "Select clip"),
        help_line("</>>", "Move clip"),
        help_line("C-d", "Duplicate clip"),
        help_line("C-s", "Split clip"),
        help_line("C-x", "Delete clip"),
        help_line("L", "Toggle loop"),
        help_line("M", "Toggle metronome"),
        help_line("=/-", "BPM \u{00b1}1 (Transport)"),
        help_line("+/_", "BPM \u{00b1}10 (Transport)"),
        help_line("w", "Cycle rec workflow (T)"),
        help_line("[/]", "Rec bars \u{00b1}1 (T)"),
        help_line("Esc", "Close / cancel"),
    ];

    let help_widget = Paragraph::new(help_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help ")
                .style(theme::style_help_bg()),
        )
        .style(theme::style_help_bg());
    frame.render_widget(help_widget, popup);
}

/// Build a single help line with a right-aligned key and a description.
#[must_use]
fn help_line<'a>(key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("  {key:>8}"), theme::style_help_key()),
        Span::raw("  "),
        Span::styled(desc, theme::style_help_desc()),
    ])
}

/// Compute a centered `Rect` within `area`, taking the given percentage
/// of width and height.
#[must_use]
pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
