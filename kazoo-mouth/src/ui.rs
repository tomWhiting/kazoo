use std::time::Instant;

use kazoo_core::sanitize_sample;
use kazoo_core::voice::quantise::{NOTE_NAMES, SCALE_NAMES};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::controls::{App, Control};

mod pitch;
mod trail;

pub(crate) use trail::Display;

#[cfg(test)]
mod tests;

const DIM: Color = Color::DarkGray;
const BRIGHT: Color = Color::Cyan;
const SCALE_LINE: Color = Color::Rgb(45, 45, 50);
const FOOTER: &str =
    "↑↓ choose  ←→ change (shift: big)  space quantise  v vibrato  s silent  ? help  esc quit";
const COMPACT_FOOTER: &str =
    "↑↓ choose ←→ change (shift: big) space quantise v vibrato s silent ? help esc quit";

// Three columns and five rows keep every note the same visual size.
const FONT: [(char, [u8; 5]); 19] = [
    ('A', [0b010, 0b101, 0b111, 0b101, 0b101]),
    ('B', [0b110, 0b101, 0b110, 0b101, 0b110]),
    ('C', [0b011, 0b100, 0b100, 0b100, 0b011]),
    ('D', [0b110, 0b101, 0b101, 0b101, 0b110]),
    ('E', [0b111, 0b100, 0b110, 0b100, 0b111]),
    ('F', [0b111, 0b100, 0b110, 0b100, 0b100]),
    ('G', [0b011, 0b100, 0b101, 0b101, 0b011]),
    ('#', [0b101, 0b111, 0b101, 0b111, 0b101]),
    ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
    ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
    ('2', [0b110, 0b001, 0b010, 0b100, 0b111]),
    ('3', [0b110, 0b001, 0b010, 0b001, 0b110]),
    ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
    ('5', [0b111, 0b100, 0b110, 0b001, 0b110]),
    ('6', [0b011, 0b100, 0b111, 0b101, 0b111]),
    ('7', [0b111, 0b001, 0b010, 0b010, 0b010]),
    ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
    ('9', [0b111, 0b101, 0b111, 0b001, 0b110]),
    ('-', [0b000, 0b000, 0b111, 0b000, 0b000]),
];

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App, display: &Display, link: &str, now: Instant) {
    let area = frame.area();
    if area.width < 80 || area.height < 24 {
        frame.render_widget(
            Paragraph::new("make the terminal at least 80×24"),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Min(9),
            Constraint::Length(2),
            Constraint::Length(6),
            Constraint::Length(1),
        ])
        .split(area);
    header(frame, areas[0], app, display, link);
    pitch::draw(frame, areas[1], display);
    trail::draw(frame, areas[2], display, app.accepted.quantise);
    meters(frame, areas[3], display);
    controls(frame, areas[4], app);
    let footer = if app.busy(now) {
        "busy: change not sent"
    } else if usize::from(area.width) >= FOOTER.chars().count() {
        FOOTER
    } else {
        COMPACT_FOOTER
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(if app.busy(now) {
            Color::Yellow
        } else {
            DIM
        })),
        areas[5],
    );
    if app.help {
        help(frame, area);
    }
}

fn header(frame: &mut Frame<'_>, area: Rect, app: &App, display: &Display, link: &str) {
    let settings = app.accepted.quantise;
    let quantise = if settings.blend > 0.0 {
        format!(
            "quantise on: {} {}, snap {:.0} ms",
            SCALE_NAMES[settings.scale],
            NOTE_NAMES[usize::from(settings.root)],
            settings.snap_ms
        )
    } else {
        "quantise off: hertz for hertz".into()
    };
    let gate = if display.latest.is_some_and(|voice| voice.gate) {
        "●"
    } else {
        "○"
    };
    let line = Line::from(vec![
        Span::styled(
            format!("{gate} kazoo mouth"),
            Style::default().fg(BRIGHT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" · {link} · {quantise}")),
    ]);
    let silent_width = if app.accepted.silent { 9 } else { 0 };
    frame.render_widget(
        Paragraph::new(line),
        Rect::new(area.x, area.y, area.width - silent_width, 1),
    );
    if app.accepted.silent {
        frame.render_widget(
            Paragraph::new(" · silent").style(Style::default().fg(Color::Yellow)),
            Rect::new(area.right() - silent_width, area.y, silent_width, 1),
        );
    }
}

fn meters(frame: &mut Frame<'_>, area: Rect, display: &Display) {
    let (loudness, breath, confidence, gate) =
        display.latest.map_or((0.0, 0.0, 0.0, false), |voice| {
            (voice.loudness, voice.breath, voice.confidence, voice.gate)
        });
    let line = Line::from(vec![
        Span::raw("loud "),
        Span::styled(meter(loudness), Style::default().fg(BRIGHT)),
        Span::raw("  breath "),
        Span::styled(meter(breath), Style::default().fg(BRIGHT)),
        Span::raw("  sure "),
        Span::styled(meter(confidence), Style::default().fg(BRIGHT)),
    ]);
    frame.render_widget(
        Paragraph::new(vec![
            line,
            Line::from(if gate { "gate ●" } else { "gate ○" }),
        ]),
        area,
    );
}

fn meter(value: f32) -> String {
    let filled = (sanitize_sample(value).clamp(0.0, 1.0) * 16.0).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(16 - filled))
}

fn controls(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    for (index, control) in Control::ALL.into_iter().enumerate() {
        let column = columns[index / 6];
        let selected = app.selected == control;
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(BRIGHT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let text = format!(
            "{} {:<12} {}",
            if selected { "›" } else { " " },
            control.name(),
            app.accepted.value(control)
        );
        frame.render_widget(
            Paragraph::new(text).style(style),
            Rect::new(column.x, column.y + (index % 6) as u16, column.width, 1),
        );
    }
}

fn help(frame: &mut Frame<'_>, area: Rect) {
    let overlay = Rect::new(area.x + 1, area.y + 1, area.width - 2, 22);
    frame.render_widget(Clear, overlay);
    let mut lines = vec![
        Line::from("Up / Down: choose a control, wrapping at either end."),
        Line::from("Left / Right: change; Shift+Left / Shift+Right: big step."),
        Line::from("Space: quantise on/off; v: keep vibrato; s: silent."),
        Line::from("?: open/close help; Esc: close help, otherwise quit."),
        Line::from("Ctrl-C: quit, including while help is open."),
        Line::from(""),
    ];
    lines.extend(
        Control::ALL.map(|control| Line::from(format!("{}: {}", control.name(), control.help()))),
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" mouth keys and controls "),
            )
            .style(Style::default().fg(Color::White).bg(Color::Black)),
        overlay,
    );
}
