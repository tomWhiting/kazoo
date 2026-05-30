//! TUI rendering for the Minimoog bass/lead synth.
//!
//! Inspired by the Model D front panel: dark walnut cabinet, black control
//! panels with white text, red indicators and highlights. Monophonic character.
//! Knob-per-function — everything visible, everything directly editable.
//!
//! Split into sub-modules matching the Model D panel sections:
//! - `oscillators` — three VCOs side by side
//! - `filter` — ladder filter with cross-mod
//! - `envelopes` — filter contour + loudness contour
//! - `performance` — glide, legato, retrigger, mod wheel
//! - `monitor` — waveform display + mixer

mod envelopes;
mod filter;
mod monitor;
mod oscillators;
mod performance;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;

// ---------------------------------------------------------------------------
// Color palette — Moog red on dark walnut
// ---------------------------------------------------------------------------

/// Dark walnut background (the cabinet).
const WALNUT: Color = Color::Rgb(28, 20, 16);
/// Control panel background (black face plate).
pub const PANEL: Color = Color::Rgb(18, 16, 14);
/// Primary text — cream/ivory like vintage panel labels.
pub const CREAM: Color = Color::Rgb(230, 220, 200);
/// Dimmed text — aged label feel.
pub const DIM: Color = Color::Rgb(90, 80, 70);
/// Primary accent — Moog red. Active selections, highlights.
pub const MOOG_RED: Color = Color::Rgb(200, 50, 40);
/// Warm red for active note/voice indicator.
const NOTE_RED: Color = Color::Rgb(255, 80, 50);
/// Section border when active.
pub const BORDER_ACTIVE: Color = Color::Rgb(180, 60, 40);
/// Section border when inactive.
pub const BORDER_INACTIVE: Color = Color::Rgb(55, 45, 38);
/// Selected parameter — bright cream on red.
const SEL_FG: Color = Color::Rgb(255, 240, 220);
const SEL_BG: Color = Color::Rgb(120, 30, 25);
/// Waveform trace color.
pub const WAVE_RED: Color = Color::Rgb(200, 60, 40);
/// Slider fill color.
const SLIDER_FILL: Color = Color::Rgb(160, 50, 35);

// ---------------------------------------------------------------------------
// Main draw function
// ---------------------------------------------------------------------------

/// Render the complete Minimoog TUI.
pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    // Full walnut background.
    let bg_block = Block::default().style(Style::new().bg(WALNUT));
    f.render_widget(bg_block, area);

    // Main layout: header, body, waveform, status.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(10),   // body
            Constraint::Length(4), // waveform monitor
            Constraint::Length(2), // status bar + keyboard help
        ])
        .split(area);

    draw_header(f, chunks[0], app);
    draw_body(f, chunks[1], app);
    monitor::draw_waveform(f, chunks[2], app);
    draw_status_bar(f, chunks[3], app);
}

// ---------------------------------------------------------------------------
// Header — brand strip
// ---------------------------------------------------------------------------

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER_INACTIVE))
        .style(Style::new().bg(PANEL));

    let note_display = app
        .voice
        .current_note()
        .map_or_else(|| "---".to_string(), kazoo_core::midi_note_name);

    let glide_str = if app.voice.glide.enabled {
        "GLIDE"
    } else {
        "     "
    };

    let title = Line::from(vec![
        Span::styled(" M", Style::new().fg(MOOG_RED).add_modifier(Modifier::BOLD)),
        Span::styled("OOG ", Style::new().fg(CREAM).add_modifier(Modifier::BOLD)),
        Span::styled("MINIMOOG MODEL D ", Style::new().fg(DIM)),
        Span::styled(" | ", Style::new().fg(BORDER_INACTIVE)),
        Span::styled(
            format!(" {note_display} "),
            Style::new().fg(NOTE_RED).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" | ", Style::new().fg(BORDER_INACTIVE)),
        Span::styled(
            format!(" {glide_str} "),
            Style::new().fg(if app.voice.glide.enabled {
                MOOG_RED
            } else {
                DIM
            }),
        ),
        Span::styled(" | ", Style::new().fg(BORDER_INACTIVE)),
        Span::styled(format!(" {}Hz ", app.sample_rate), Style::new().fg(DIM)),
    ]);

    let header = Paragraph::new(title).block(block);
    f.render_widget(header, area);
}

// ---------------------------------------------------------------------------
// Body: five panels laid out like the Model D front panel
// ---------------------------------------------------------------------------

fn draw_body(f: &mut Frame, area: Rect, app: &App) {
    // Model D layout: Oscillators | Mixer | Filter | Envelopes | Performance
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30), // 3 oscillators side by side
            Constraint::Percentage(15), // mixer
            Constraint::Percentage(20), // filter
            Constraint::Percentage(22), // envelopes
            Constraint::Percentage(13), // performance
        ])
        .split(area);

    oscillators::draw_oscillators(f, cols[0], app);
    monitor::draw_mixer(f, cols[1], app);
    filter::draw_filter(f, cols[2], app);
    envelopes::draw_envelopes(f, cols[3], app);
    performance::draw_performance(f, cols[4], app);
}

// ---------------------------------------------------------------------------
// Status bar — keyboard help
// ---------------------------------------------------------------------------

fn draw_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let section_name = app.section.name();

    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!(" [{section_name}] "),
                Style::new().fg(MOOG_RED).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Piano: ", Style::new().fg(DIM)),
            Span::styled("z-/ ", Style::new().fg(CREAM)),
            Span::styled("(lower)  ", Style::new().fg(DIM)),
            Span::styled("q-p ", Style::new().fg(CREAM)),
            Span::styled("(upper)", Style::new().fg(DIM)),
        ]),
        Line::from(vec![
            Span::styled(" Tab", Style::new().fg(MOOG_RED)),
            Span::styled(":section  ", Style::new().fg(DIM)),
            Span::styled("\u{2191}/\u{2193}", Style::new().fg(MOOG_RED)),
            Span::styled(":param  ", Style::new().fg(DIM)),
            Span::styled("+/-", Style::new().fg(MOOG_RED)),
            Span::styled(":adjust  ", Style::new().fg(DIM)),
            Span::styled("Esc", Style::new().fg(MOOG_RED)),
            Span::styled(":quit", Style::new().fg(DIM)),
        ]),
    ];

    let bar = Paragraph::new(lines).style(Style::new().bg(WALNUT));
    f.render_widget(bar, area);
}

// ---------------------------------------------------------------------------
// Shared helpers (used by sub-modules)
// ---------------------------------------------------------------------------

/// Format a parameter line with optional selection highlight.
pub fn param_line<'a>(name: &str, value: &str, selected: bool) -> Line<'a> {
    let (style, indicator) = if selected {
        (
            Style::new()
                .fg(SEL_FG)
                .bg(SEL_BG)
                .add_modifier(Modifier::BOLD),
            ">",
        )
    } else {
        (Style::new().fg(CREAM), " ")
    };

    Line::from(Span::styled(
        format!("{indicator} {name:<10}{value:>8}"),
        style,
    ))
}

/// Format a mixer slider line.
pub fn vertical_slider_line(name: &str, value: f32, selected: bool) -> Line<'static> {
    let (style, indicator) = if selected {
        (
            Style::new()
                .fg(SEL_FG)
                .bg(SEL_BG)
                .add_modifier(Modifier::BOLD),
            ">",
        )
    } else {
        (Style::new().fg(CREAM), " ")
    };

    let pct = (value * 100.0) as u32;
    let bar_width = 8;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let filled = ((value * bar_width as f32) as usize).min(bar_width);
    let empty = bar_width - filled;

    let fill_str: String = "\u{2588}".repeat(filled);
    let empty_str: String = "\u{2591}".repeat(empty);

    Line::from(vec![
        Span::styled(format!("{indicator} {name:<5} "), style),
        Span::styled(fill_str, Style::new().fg(SLIDER_FILL)),
        Span::styled(empty_str, Style::new().fg(BORDER_INACTIVE)),
        Span::styled(format!(" {pct:>3}%"), style),
    ])
}

/// Format a time value for display.
pub fn format_time(secs: f32) -> String {
    if secs < 0.01 {
        format!("{:.1}ms", secs * 1000.0)
    } else if secs < 1.0 {
        format!("{:.0}ms", secs * 1000.0)
    } else {
        format!("{secs:.1}s")
    }
}
