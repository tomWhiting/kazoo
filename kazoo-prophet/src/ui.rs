//! Ratatui drawing for the Prophet instrument.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Sparkline, Table};

use crate::app::{App, Section};

pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(8),
        ])
        .split(frame.area());

    draw_header(frame, app, root[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(24),
            Constraint::Min(40),
            Constraint::Length(32),
        ])
        .split(root[1]);

    draw_sections(frame, app, body[0]);
    draw_params(frame, app, body[1]);
    draw_voices(frame, app, body[2]);
    draw_waveform(frame, app, root[2]);
}

fn draw_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let title = Line::from(vec![
        Span::styled(
            "KAZOO PROPHET-5",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("pure synth engine", Style::default().fg(Color::Gray)),
        Span::raw(format!("  {} Hz", app.sample_rate)),
    ]);
    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_sections(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let items: Vec<_> = Section::ALL
        .iter()
        .map(|section| {
            let style = if *section == app.section {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(Span::styled(section.name(), style)))
        })
        .collect();
    frame.render_widget(
        List::new(items).block(Block::default().title("BANK").borders(Borders::ALL)),
        area,
    );
}

fn draw_params(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let rows: Vec<_> = app
        .param_rows()
        .into_iter()
        .enumerate()
        .map(|(idx, text)| {
            let style = if idx == app.param_index {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(Span::styled(text, style)))
        })
        .collect();
    frame.render_widget(
        List::new(rows).block(
            Block::default()
                .title(app.section.name())
                .borders(Borders::ALL),
        ),
        area,
    );
}

fn draw_voices(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let rows = app.voice_status.iter().map(|voice| {
        let state = if voice.releasing {
            "REL"
        } else if voice.active {
            "ON"
        } else {
            "--"
        };
        Row::new([
            Cell::from(voice.index.to_string()),
            Cell::from(state),
            Cell::from(
                voice
                    .note
                    .map_or_else(|| String::from("--"), |note| note.to_string()),
            ),
            Cell::from(format!("{:+.1}", voice.drift_cents)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Length(8),
        ],
    )
    .header(Row::new(["V", "STATE", "NOTE", "DRIFT"]).style(Style::default().fg(Color::Yellow)))
    .block(Block::default().title("VOICES").borders(Borders::ALL));
    frame.render_widget(table, area);
}

fn draw_waveform(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let data: Vec<u64> = app
        .waveform_buf
        .iter()
        .step_by(8)
        .map(|sample| ((sample.clamp(-1.0, 1.0) + 1.0) * 32.0) as u64)
        .collect();
    frame.render_widget(
        Sparkline::default()
            .block(Block::default().title("WAVEFORM").borders(Borders::ALL))
            .style(Style::default().fg(Color::Green))
            .data(&data),
        area,
    );
}
