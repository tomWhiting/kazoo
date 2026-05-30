//! Envelope section rendering — filter contour + loudness contour.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::{BORDER_ACTIVE, BORDER_INACTIVE, CREAM, MOOG_RED, PANEL, format_time, param_line};
use crate::app::{App, Section};

/// Draw the two envelope panels (filter contour + loudness contour).
pub fn draw_envelopes(f: &mut Frame, area: Rect, app: &App) {
    let is_active = app.section == Section::Envelopes;

    // Two sub-panels stacked.
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    draw_envelope_panel(
        f,
        rows[0],
        " FILTER CONTOUR ",
        &app.voice.filter_env,
        is_active,
        0,
        app.param_index,
    );
    draw_envelope_panel(
        f,
        rows[1],
        " LOUDNESS CONTOUR ",
        &app.voice.amp_env,
        is_active,
        4,
        app.param_index,
    );
}

fn draw_envelope_panel(
    f: &mut Frame,
    area: Rect,
    title: &str,
    env: &crate::synth::envelope::AdsrEnvelope,
    section_active: bool,
    param_offset: usize,
    current_param: usize,
) {
    let border_color = if section_active {
        BORDER_ACTIVE
    } else {
        BORDER_INACTIVE
    };

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::new()
                .fg(if section_active { MOOG_RED } else { CREAM })
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::new().fg(border_color))
        .style(Style::new().bg(PANEL));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // ADSR display.
    let lines = vec![
        param_line(
            "Attack",
            &format_time(env.attack),
            section_active && current_param == param_offset,
        ),
        param_line(
            "Decay",
            &format_time(env.decay),
            section_active && current_param == param_offset + 1,
        ),
        param_line(
            "Sustain",
            &format!("{}%", (env.sustain * 100.0) as u32),
            section_active && current_param == param_offset + 2,
        ),
        param_line(
            "Release",
            &format_time(env.release),
            section_active && current_param == param_offset + 3,
        ),
    ];

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}
