//! Ratatui interface for the acid bassline synth.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Row, Table, Wrap};

use crate::App;
use crate::sequencer::STEPS_PER_PATTERN;
use crate::synth::AcidSynthParam;

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Min(10),
            Constraint::Length(4),
        ])
        .split(frame.area());

    draw_header(frame, chunks[0], app);
    draw_pattern(frame, chunks[1], app);
    draw_params(frame, chunks[2], app);
    draw_footer(frame, chunks[3]);

    if app.show_help {
        draw_help(frame);
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let state = if app.playing { "PLAY" } else { "STOP" };
    let text = format!(
        " kazoo-303  |  {state}  |  {:.1} BPM  |  waveform: {}  |  all procedural synthesis, no samples ",
        app.sequencer.clock.bpm(),
        app.synth.waveform().label()
    );
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Acid Bassline")),
        area,
    );
}

fn draw_pattern(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let pattern = app.sequencer.current_pattern();
    let mut step_cells = Vec::with_capacity(STEPS_PER_PATTERN);
    let mut note_cells = Vec::with_capacity(STEPS_PER_PATTERN);
    let mut accent_cells = Vec::with_capacity(STEPS_PER_PATTERN);
    let mut slide_cells = Vec::with_capacity(STEPS_PER_PATTERN);

    for idx in 0..STEPS_PER_PATTERN {
        let step = pattern.steps[idx];
        let selected = idx == app.cursor_step;
        let playing = idx == app.playback_step && app.playing;
        let style = if selected {
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else if playing {
            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
        } else if step.active {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        step_cells.push(Span::styled(format!("{:02} ", idx + 1), style));
        note_cells.push(Span::styled(format!("{:>3}", step.note_name()), style));
        accent_cells.push(Span::styled(if step.accent { " !!" } else { " .." }, style));
        slide_cells.push(Span::styled(if step.slide { " >>" } else { " .." }, style));
    }

    let mut step_line = vec![Span::raw("step ")];
    step_line.extend(step_cells);
    let mut note_line = vec![Span::raw("note ")];
    note_line.extend(note_cells);
    let mut accent_line = vec![Span::raw("acc  ")];
    accent_line.extend(accent_cells);
    let mut slide_line = vec![Span::raw("slid ")];
    slide_line.extend(slide_cells);

    let lines = vec![
        Line::from(step_line),
        Line::from(note_line),
        Line::from(accent_line),
        Line::from(slide_line),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(pattern.name.as_str()))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_params(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let rows = AcidSynthParam::ALL.into_iter().enumerate().map(|(idx, param)| {
        let value = app.synth.param_value(param);
        let marker = if idx == app.selected_param { ">" } else { " " };
        Row::new(vec![
            format!("{marker} {}", param.label()),
            bar(value),
            format!("{:>3}%", (value * 100.0).round()),
        ])
    });

    let table = Table::new(
        rows,
        [Constraint::Length(16), Constraint::Length(28), Constraint::Length(6)],
    )
    .block(Block::default().borders(Borders::ALL).title("Voice controls"))
    .column_spacing(2);
    frame.render_widget(table, area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect) {
    let text = vec![
        Line::from("Enter play/stop  arrows select  Space step  a accent  s slide  z/x transpose"),
        Line::from(",/. adjust param  +/- BPM  w waveform  r acid pattern  ? help  q quit"),
    ];
    frame.render_widget(
        Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("Keys")),
        area,
    );
}

fn draw_help(frame: &mut Frame<'_>) {
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);
    let text = vec![
        Line::from(Span::styled("kazoo-303", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("A TB-303-inspired acid bassline synth built from code only:"),
        Line::from("oscillator math, envelopes, glide, accent, nonlinear filtering, saturation."),
        Line::from(""),
        Line::from("No samples. No borrowed recordings. Maths and shit."),
        Line::from(""),
        Line::from("Step data: active note, accent, slide."),
        Line::from("Controls: Space toggles a note, a toggles accent, s toggles slide."),
        Line::from("Use z/x to transpose the selected step."),
        Line::from(""),
        Line::from("Press any key to close this help."),
    ];
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().borders(Borders::ALL).title("Help"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn bar(value: f32) -> String {
    let filled = (value.clamp(0.0, 1.0) * 20.0).round() as usize;
    let empty = 20usize.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}
