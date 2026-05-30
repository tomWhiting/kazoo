//! UI layout, panels, and custom widgets.
//!
//! The [`draw`] function renders the entire application for one frame.
//! A persistent header (transport, meters, view tabs) occupies the top
//! 5 rows; the rest is routed to the active view based on
//! [`ActiveView`](crate::state::ActiveView).

pub mod audio_io;
pub mod effects;
pub mod file_browser;
pub mod header;
#[allow(dead_code)]
pub mod meters;
#[allow(dead_code)]
pub mod mixer;
pub mod mixing_desk;
pub mod project_view;
#[allow(dead_code)]
pub mod spectrum;
pub mod timeline;
pub mod tracking_view;
pub mod tracks;
#[allow(dead_code)]
pub mod transport;
pub mod waveform;

use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::{App, AppMode, FocusedPanel};
use crate::state::ActiveView;
use crate::theme;

/// Render the entire application UI for one frame.
///
/// Layout:
/// ```text
/// +-- Header (5 rows) — transport, meters, view tabs ----+
/// +-- Active View (rest) --------------------------------+
/// |  Synth | Mixer | Tracking | Project | Audio I/O      |
/// +-------------------------------------------------------+
/// ```
pub fn draw(frame: &mut Frame, app: &mut App) {
    let terminal_area = frame.area();

    // Fill the entire terminal with the primary background.
    frame.render_widget(
        Block::default().style(Style::new().bg(theme::BG_PRIMARY)),
        terminal_area,
    );

    // Main vertical split: persistent header (5 rows) + content area.
    let main_chunks = Layout::vertical([
        Constraint::Length(5), // header
        Constraint::Min(10),   // content view
    ])
    .split(terminal_area);

    header::draw(frame, app, main_chunks[0]);

    // Route to the active view.
    match app.active_view {
        ActiveView::Mixer => mixing_desk::draw(frame, app, main_chunks[1]),
        ActiveView::Tracking => tracking_view::draw(frame, app, main_chunks[1]),
        ActiveView::Project => project_view::draw(frame, app, main_chunks[1]),
        ActiveView::AudioIO => audio_io::draw(frame, app, main_chunks[1]),
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
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(app.is_focused(panel)))
        .title(title)
        .title_style(theme::style_panel_title(app.is_focused(panel)))
}

/// Render the help overlay as a centered popup.
fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 80, area);
    frame.render_widget(Clear, popup);

    let help_text = vec![
        Line::from(vec![Span::styled(
            "  Kazoo -- Keyboard Shortcuts",
            theme::style_help_key(),
        )]),
        Line::from(""),
        // -- Views & Navigation --
        help_section("Views & Navigation"),
        help_line("1-4", "Switch view"),
        help_line("Tab", "Next panel / card"),
        help_line("S-Tab", "Prev panel / card"),
        help_line("q", "Quit"),
        help_line("?", "Toggle help"),
        Line::from(""),
        // -- Transport --
        help_section("Transport"),
        help_line("Space", "Play / Pause"),
        help_line("s", "Stop"),
        help_line("r", "Record"),
        help_line("R", "Record with count-in"),
        help_line("L", "Toggle loop"),
        help_line("M", "Toggle metronome"),
        help_line("=/-", "BPM \u{00b1}1"),
        help_line("+/_", "BPM \u{00b1}10"),
        help_line("w", "Cycle rec workflow"),
        help_line("[/]", "Rec bars \u{00b1}1"),
        Line::from(""),
        // -- Tracks --
        help_section("Tracks"),
        help_line("j/k", "Select track"),
        help_line("m/S/a", "Mute / Solo / Arm"),
        help_line("n", "New track"),
        help_line("x", "Delete track"),
        help_line("t", "Cycle synth mode"),
        Line::from(""),
        // -- Mixer View --
        help_section("Mixer (1)"),
        help_line("h/l", "Select channel"),
        help_line("j/k", "Select control"),
        help_line("+/-", "Adjust value"),
        help_line("Space", "Toggle S/M/R"),
        Line::from(""),
        // -- Effects --
        help_section("Effects"),
        help_line("J/K", "Navigate effects"),
        help_line("h/l", "Cycle params"),
        help_line("\u{2190}/\u{2192}", "Adjust param"),
        help_line("Enter", "Direct numeric input"),
        help_line("A", "Add effect"),
        help_line("X", "Remove effect"),
        help_line("b", "Toggle bypass"),
        Line::from(""),
        // -- Tracking --
        help_section("Tracking (2)"),
        help_line("[/]", "Zoom waveform"),
        help_line(",/.", "Select clip"),
        help_line("</>", "Move clip"),
        help_line("C-d/C-s", "Duplicate / Split clip"),
        help_line("C-x", "Delete clip"),
        help_line("o", "Open file browser"),
        Line::from(""),
        // -- Project --
        help_section("Project (3)"),
        help_line("Tab/S-Tab", "Cycle cards"),
        help_line("j/k", "Select field"),
        help_line("+/-", "Adjust value"),
        help_line("Enter", "Toggle field"),
        Line::from(""),
        // -- Audio I/O --
        help_section("Audio I/O (4)"),
        help_line("Tab/S-Tab", "Cycle section"),
        help_line("j/k", "Select device"),
    ];

    let help_widget = Paragraph::new(help_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Help ")
                .style(theme::style_help_bg()),
        )
        .style(theme::style_help_bg());
    frame.render_widget(help_widget, popup);
}

/// Build a section header for the help overlay.
#[must_use]
fn help_section(title: &str) -> Line<'_> {
    Line::from(vec![Span::styled(
        format!("  {title}"),
        Style::new()
            .fg(theme::FG_PRIMARY)
            .add_modifier(ratatui::style::Modifier::BOLD | ratatui::style::Modifier::UNDERLINED),
    )])
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
