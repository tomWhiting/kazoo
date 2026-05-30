//! Project setup view.
//!
//! Card-based layout showing tempo, time signature, metronome, loop,
//! count-in, and recording workflow settings. Each setting group is
//! rendered as a bordered card. Tab cycles between cards, arrow keys
//! and +/- modify values.
//!
//! Layout:
//! ```text
//! ╭─ PROJECT SETUP ─────────────────────────────────────────────────────╮
//! │  ╭─ Tempo ──────────╮  ╭─ Time Sig ────╮  ╭─ Count-In ──────────╮ │
//! │  │ BPM: ████░░ 120  │  │ Beats/Bar: 4  │  │ Enabled: [*]        │ │
//! │  ╰───────────────────╯  ╰────────────────╯  │ Bars: 1            │ │
//! │                                              ╰────────────────────╯ │
//! │  ╭─ Metronome ──────╮  ╭─ Loop ────────╮  ╭─ Recording ─────────╮ │
//! │  │ Enabled: [*]     │  │ Enabled: [ ]  │  │ Workflow: CountIn    │ │
//! │  │ Volume: ████ 0.8 │  │ Start: Bar 1  │  │ Bars: 4             │ │
//! │  ╰───────────────────╯  │ End:   Bar 8  │  ╰────────────────────╯ │
//! │                          ╰────────────────╯                         │
//! ╰─────────────────────────────────────────────────────────────────────╯
//! ```

use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::App;
use crate::theme;
use kazoo_core::transport::RecordingWorkflow;

/// Width of a slider bar in the tempo card.
const SLIDER_WIDTH: usize = 12;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Draw the Project Setup view into the given content area.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(false))
        .title(" Project Setup ")
        .title_style(theme::style_panel_title(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 40 || inner.height < 8 {
        return;
    }

    // Two rows of cards.
    let rows = Layout::vertical([
        Constraint::Length(1), // top margin
        Constraint::Length(6), // row 1
        Constraint::Length(1), // gap
        Constraint::Length(7), // row 2
        Constraint::Min(1),    // hint bar / remaining
    ])
    .split(inner);

    // Row 1: Tempo | Time Signature | Count-In.
    let row1_cols = Layout::horizontal([
        Constraint::Percentage(33),
        Constraint::Percentage(33),
        Constraint::Percentage(34),
    ])
    .split(rows[1]);

    let sel = app.project_state.selected_card;
    draw_tempo_card(frame, app, row1_cols[0], sel == 0);
    draw_time_sig_card(frame, app, row1_cols[1], sel == 1);
    draw_count_in_card(frame, app, row1_cols[2], sel == 2);

    // Row 2: Metronome | Loop | Recording.
    let row2_cols = Layout::horizontal([
        Constraint::Percentage(33),
        Constraint::Percentage(33),
        Constraint::Percentage(34),
    ])
    .split(rows[3]);

    draw_metronome_card(frame, app, row2_cols[0], sel == 3);
    draw_loop_card(frame, app, row2_cols[1], sel == 4);
    draw_recording_card(frame, app, row2_cols[2], sel == 5);

    // Hint bar.
    if rows[4].height >= 1 {
        let hint = Paragraph::new(Line::from(vec![
            Span::styled("  Tab", theme::style_help_key()),
            Span::styled(" next card  ", theme::style_help_desc()),
            Span::styled("↑↓", theme::style_help_key()),
            Span::styled(" select  ", theme::style_help_desc()),
            Span::styled("+/-", theme::style_help_key()),
            Span::styled(" adjust  ", theme::style_help_desc()),
            Span::styled("Enter", theme::style_help_key()),
            Span::styled(" edit", theme::style_help_desc()),
        ]));
        frame.render_widget(hint, rows[4]);
    }
}

// ---------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------

fn card_block(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(focused))
        .title(title)
        .title_style(theme::style_panel_title(focused))
}

fn draw_tempo_card(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let block = card_block(" Tempo ", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width < 10 {
        return;
    }

    let bpm = app.display.transport.bpm;
    let ratio = ((bpm - 30.0) / (300.0 - 30.0)).clamp(0.0, 1.0) as f32;
    let filled = (ratio * SLIDER_WIDTH as f32).round() as usize;
    let empty = SLIDER_WIDTH.saturating_sub(filled);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

    let lines = vec![
        Line::from(vec![
            Span::styled(" BPM: ", theme::style_text_secondary()),
            Span::styled(bar, theme::style_slider_filled()),
            Span::styled(format!(" {bpm:.0}"), theme::style_text()),
        ]),
        Line::from(""),
        Line::from(Span::styled(" [Tap Tempo]", theme::style_text_dimmed())),
    ];
    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

fn draw_time_sig_card(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let block = card_block(" Time Signature ", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width < 10 {
        return;
    }

    let ts = &app.display.transport;
    let lines = vec![
        Line::from(vec![
            Span::styled(" Beats/Bar: ", theme::style_text_secondary()),
            Span::styled(format!("{}", ts.beats_per_bar), theme::style_text()),
        ]),
        Line::from(vec![
            Span::styled(" Beat Unit: ", theme::style_text_secondary()),
            Span::styled(format!("{}", ts.beat_unit), theme::style_text()),
        ]),
    ];
    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

fn draw_count_in_card(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let block = card_block(" Count-In ", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width < 10 {
        return;
    }

    let enabled = app.count_in_bars > 0;
    let lines = vec![
        Line::from(vec![
            Span::styled(" Enabled: ", theme::style_text_secondary()),
            Span::styled(if enabled { "[*]" } else { "[ ]" }, theme::style_text()),
        ]),
        Line::from(vec![
            Span::styled(" Bars:    ", theme::style_text_secondary()),
            Span::styled(format!("{}", app.count_in_bars), theme::style_text()),
        ]),
    ];
    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

fn draw_metronome_card(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let block = card_block(" Metronome ", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width < 10 {
        return;
    }

    let enabled = app.display.transport.metronome_enabled;
    let lines = vec![Line::from(vec![
        Span::styled(" Enabled: ", theme::style_text_secondary()),
        Span::styled(if enabled { "[*]" } else { "[ ]" }, theme::style_text()),
    ])];
    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

fn draw_loop_card(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let block = card_block(" Loop ", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width < 10 {
        return;
    }

    let enabled = app.display.transport.loop_enabled;
    let lines = vec![
        Line::from(vec![
            Span::styled(" Enabled: ", theme::style_text_secondary()),
            Span::styled(if enabled { "[*]" } else { "[ ]" }, theme::style_text()),
        ]),
        Line::from(vec![
            Span::styled(" Start:   ", theme::style_text_secondary()),
            Span::styled("Bar 1", theme::style_text()),
        ]),
        Line::from(vec![
            Span::styled(" End:     ", theme::style_text_secondary()),
            Span::styled("∞", theme::style_text()),
        ]),
    ];
    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

fn draw_recording_card(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let block = card_block(" Recording ", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width < 10 {
        return;
    }

    let workflow_name = match &app.recording_workflow {
        RecordingWorkflow::CountIn { .. } => "Count-In",
        RecordingWorkflow::FixedLength { .. } => "Fixed Length",
        RecordingWorkflow::FreeRecord => "Free Record",
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(" Workflow: ", theme::style_text_secondary()),
            Span::styled(workflow_name, theme::style_text()),
        ]),
        Line::from(vec![
            Span::styled(" Rec Bars: ", theme::style_text_secondary()),
            Span::styled(format!("{}", app.record_bars), theme::style_text()),
        ]),
    ];
    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
