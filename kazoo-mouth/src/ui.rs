use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Paragraph, Wrap};

use crate::controls::App;

mod board;
mod looper;
mod pitch;
mod trail;

pub use trail::Display;

#[cfg(test)]
mod tests;

const DIM: Color = Color::DarkGray;
const BRIGHT: Color = Color::Cyan;
const SCALE_LINE: Color = Color::Rgb(45, 45, 50);
const FOOTER: &str = "Tab ↑↓ ←→ w Space b d ?";

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
    if area.width == 0 || area.height == 0 {
        return;
    }
    let voice_height = if area.height >= 35 {
        11
    } else if area.height >= 24 {
        5
    } else {
        2
    };
    let mic = format!("mic: {}", app.input_label);
    let mic_rows = u16::try_from(mic.chars().count().div_ceil(usize::from(area.width)))
        .unwrap_or(u16::MAX)
        .max(1);
    let link_rows = u16::try_from(link.chars().count().div_ceil(usize::from(area.width)))
        .unwrap_or(u16::MAX)
        .max(1);
    let header_rows = 1_u16
        .saturating_add(mic_rows)
        .saturating_add(link_rows)
        .min(area.height.saturating_sub(5));
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_rows),
            Constraint::Length(voice_height),
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(area);
    let gate = if display.latest.is_some_and(|voice| voice.gate) {
        "●"
    } else {
        "○"
    };
    let bpm = app.desk_bpm.unwrap_or(app.accepted.own_bpm);
    let title = format!(
        "{gate} kazoo mouth · {} Hz · ♩{bpm:.1} · latency --{}",
        app.loop_sample_rate,
        if app.accepted.silent {
            " · silent"
        } else {
            ""
        }
    );
    frame.render_widget(
        Paragraph::new(format!("{title}\n{link}\n{mic}")).wrap(Wrap { trim: true }),
        areas[0],
    );
    let strip = areas[1];
    let meter_width = strip.width.min(38);
    let plot_width = strip.width.saturating_sub(meter_width);
    if plot_width >= 40 && strip.height > 5 {
        pitch::draw(frame, Rect::new(strip.x, strip.y, plot_width, 5), display);
        if strip.height > 5 {
            trail::draw(
                frame,
                Rect::new(strip.x, strip.y + 5, plot_width, strip.height - 5),
                display,
                app.accepted.quantise,
            );
        }
    } else if plot_width > 0 {
        trail::draw(
            frame,
            Rect::new(strip.x, strip.y, plot_width, strip.height),
            display,
            app.accepted.quantise,
        );
    }
    board::meters(
        frame,
        Rect::new(strip.x + plot_width, strip.y, meter_width, strip.height),
        app,
    );
    board::draw(frame, areas[2], app, display);
    looper::draw(frame, areas[3], app);
    let description = app.description();
    let footer = if app.busy(now) {
        "busy: change not sent".to_owned()
    } else if let Some(notice) = app.notice(now) {
        notice.to_owned()
    } else {
        format!("{FOOTER} · {description}")
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(DIM)),
        areas[4],
    );
    if app.help {
        board::help(frame, area, app);
    }
}
