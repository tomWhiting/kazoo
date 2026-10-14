use std::time::Instant;

use kazoo_core::sanitize_sample;
use kazoo_core::voice::quantise::{NOTE_NAMES, SCALE_NAMES};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::controls::{App, Control, Focus};
use crate::engine::fx::Slot;

mod fx;
mod pitch;
mod trail;

pub use trail::Display;

#[cfg(test)]
mod tests;

const DIM: Color = Color::DarkGray;
const BRIGHT: Color = Color::Cyan;
const SCALE_LINE: Color = Color::Rgb(45, 45, 50);
const FOOTER: &str = "↑↓ choose  ←→ change  space quantise  v vibrato  s silent  tab fx  ? help";
const FX_FOOTER: &str =
    "[ ] slot  ↑↓ knob  ←→ turn  b on/off  { } move  d default  tab synth  ? help";

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

pub fn draw(frame: &mut Frame<'_>, app: &App, display: &Display, link: &str, now: Instant) {
    let area = frame.area();
    if area.width < 80 || area.height < 24 {
        frame.render_widget(
            Paragraph::new("make the terminal at least 80×24"),
            Rect::new(area.x, area.y, area.width, 1),
        );
        return;
    }
    let both = area.height >= 30;
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Min(9),
            Constraint::Length(2),
            Constraint::Length(6),
            Constraint::Length(if both { 6 } else { 0 }),
            Constraint::Length(1),
        ])
        .split(area);
    header(frame, areas[0], app, display, link);
    pitch::draw(frame, areas[1], display);
    trail::draw(frame, areas[2], display, app.accepted.quantise);
    meters(frame, areas[3], display);
    if both || app.focus == Focus::Synth {
        controls(frame, areas[4], app);
    } else {
        fx::draw(frame, areas[4], app);
    }
    if both {
        fx::draw(frame, areas[5], app);
    }
    let footer = if app.busy(now) {
        "busy: change not sent"
    } else if let Some(notice) = app.notice(now) {
        notice
    } else if app.focus == Focus::Fx {
        FX_FOOTER
    } else {
        FOOTER
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(
            if app.busy(now) || app.notice(now).is_some() {
                Color::Yellow
            } else {
                DIM
            },
        )),
        areas[6],
    );
    if app.help {
        help(frame, area, app);
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
    let title = Line::from(vec![
        Span::styled(
            format!("{gate} kazoo mouth"),
            Style::default().fg(BRIGHT).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " · {link} · {} · {quantise}",
            if app.focus == Focus::Fx {
                "fx"
            } else {
                "synth"
            }
        )),
    ]);
    let mut badges = String::new();
    if app.fx_faults > 0 {
        badges.push_str(" fx_faults ");
        badges.push_str(&app.fx_faults.to_string());
    }
    if app.fx_refused > 0 {
        badges.push_str(" fx_refused ");
        badges.push_str(&app.fx_refused.to_string());
    }
    if app.accepted.silent {
        badges.push_str(" · silent");
    }
    let silent_width = (badges.chars().count() as u16).min(area.width);
    frame.render_widget(
        Paragraph::new(title),
        Rect::new(area.x, area.y, area.width - silent_width, 1),
    );
    if silent_width > 0 {
        frame.render_widget(
            Paragraph::new(badges).style(Style::default().fg(Color::Yellow)),
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
        let selected = app.selected == control && app.focus == Focus::Synth;
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

fn help(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let overlay = Rect::new(area.x + 1, area.y + 1, area.width - 2, area.height - 2);
    frame.render_widget(Clear, overlay);
    let mut lines = vec![
        Line::from("Up / Down: choose a control, wrapping at either end."),
        Line::from("Left / Right: change; Shift+Left / Shift+Right: big step."),
        Line::from("Space: quantise on/off; v: keep vibrato; s: silent."),
        Line::from("?: open/close help; Esc: close help, otherwise quit."),
        Line::from("Ctrl-C: quit, including while help is open."),
        Line::from("Tab: switch between synth controls and effects."),
        Line::from("[ / ]: choose the previous / next slot, wrapping."),
        Line::from("b: switch the selected effect on or off, with trails."),
        Line::from("{ / }: move the selected slot earlier / later."),
        Line::from("d: restore the selected knob's mouth default."),
        Line::from(", / .: own tempo down / up 1 BPM; < / >: 10 BPM."),
        Line::from("The desk owns tempo while connected; change it on the desk."),
        Line::from("Help: Up/Down or PageUp/PageDown scroll all descriptions."),
        Line::from(format!("Settings: {}", app.settings_path.display())),
        Line::from(""),
    ];
    lines.extend(
        Control::ALL.map(|control| Line::from(format!("{}: {}", control.name(), control.help()))),
    );
    for slot in Slot::ALL {
        for index in 0..slot.ids().len() {
            if let Some(kind) = slot.kind(index) {
                lines.push(Line::from(format!("{}: {}", kind.id, kind.description)));
            }
        }
    }
    let lines = wrap_help(lines, overlay.width.saturating_sub(2));
    let maximum = lines
        .len()
        .saturating_sub(usize::from(overlay.height.saturating_sub(2)))
        .min(usize::from(u16::MAX)) as u16;
    let scroll = app.help_scroll.get().min(maximum);
    app.help_scroll.set(scroll);
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scroll, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" mouth keys and controls "),
            )
            .style(Style::default().fg(Color::White).bg(Color::Black)),
        overlay,
    );
}

fn wrap_help(lines: Vec<Line<'_>>, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let mut wrapped = Vec::new();
    for line in lines {
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        let mut row = String::new();
        let mut used = 0;
        for word in text.split_whitespace() {
            let word_width = Span::raw(word).width();
            if used > 0 && used + 1 + word_width > width {
                wrapped.push(Line::from(std::mem::take(&mut row)));
                used = 0;
            }
            if used > 0 {
                row.push(' ');
                used += 1;
            }
            for character in word.chars() {
                let mut encoded = [0; 4];
                let char_width = Span::raw(&*character.encode_utf8(&mut encoded)).width();
                if used + char_width > width {
                    wrapped.push(Line::from(std::mem::take(&mut row)));
                    used = 0;
                }
                row.push(character);
                used += char_width;
            }
        }
        wrapped.push(Line::from(row));
    }
    wrapped
}
