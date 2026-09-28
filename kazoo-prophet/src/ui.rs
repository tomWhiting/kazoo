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
            Constraint::Length(1),
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
    draw_footer(frame, app, root[3]);
}

/// Key hints, plus anything the user must know about notes or edits that did
/// not reach the audio engine.
fn draw_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    frame.render_widget(Paragraph::new(footer_line(app)), area);
}

fn footer_line(app: &App) -> Line<'static> {
    let mut spans = Vec::with_capacity(6);
    if app.stream.lost {
        spans.push(Span::styled(
            " AUDIO DEVICE LOST: restart to hear anything ",
            Style::new().fg(Color::Black).bg(Color::Red),
        ));
        spans.push(Span::raw(" "));
    } else if app.stream.errors > 0 {
        spans.push(Span::styled(
            format!("{} audio stream errors  ", app.stream.errors),
            Style::new().fg(Color::Yellow),
        ));
    }
    if app.commands_dropped > 0 || app.params_pending {
        let reason = app
            .delivery_warning
            .unwrap_or("earlier commands were refused");
        let pending = if app.params_pending {
            ", edits waiting"
        } else {
            ""
        };
        spans.push(Span::styled(
            format!(
                " {reason}: {} commands refused{pending} (Space releases notes) ",
                app.commands_dropped
            ),
            Style::new().fg(Color::Black).bg(Color::Red),
        ));
        spans.push(Span::raw(" "));
    }
    if let Some(note) = &app.key_release_note {
        spans.push(Span::styled(
            format!("{note}  "),
            Style::new().fg(Color::Yellow),
        ));
    }
    if app.display_dropped > 0 {
        spans.push(Span::styled(
            format!("screen lagging: {} frames skipped  ", app.display_dropped),
            Style::new().fg(Color::Yellow),
        ));
    }
    spans.push(Span::styled(
        "Esc/Ctrl+Q quit  Tab section  ↑↓ select  ←→ adjust  Space all notes off  keys z..i play",
        Style::new().fg(Color::DarkGray),
    ));
    Line::from(spans)
}

fn draw_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let mut title = Line::from(vec![
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
    if let Some((text, warn)) = app.hub_badge() {
        let style = if warn {
            Style::default().fg(Color::Yellow)
        } else if app.hub.as_ref().is_some_and(|hub| hub.connected) {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        title.spans.push(Span::raw("  "));
        title.spans.push(Span::styled(text, style));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn footer_shows_quit_key() {
        let app = App::new(48_000);
        assert!(text(&footer_line(&app)).contains("Esc/Ctrl+Q quit"));
    }

    #[test]
    fn footer_reports_refused_commands_and_key_release_note() {
        let mut app = App::new(48_000);
        app.commands_dropped = 3;
        app.delivery_warning = Some("audio engine busy");
        app.key_release_note = Some("terminal can't report key releases".to_owned());
        let line = text(&footer_line(&app));
        assert!(line.contains("audio engine busy: 3 commands refused"));
        assert!(line.contains("terminal can't report key releases"));
    }

    #[test]
    fn footer_reports_stream_trouble() {
        let mut app = App::new(48_000);
        app.stream.errors = 2;
        assert!(text(&footer_line(&app)).contains("2 audio stream errors"));
        app.stream.lost = true;
        assert!(text(&footer_line(&app)).contains("AUDIO DEVICE LOST"));
    }
}
