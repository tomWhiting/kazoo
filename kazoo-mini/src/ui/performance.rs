//! Performance section rendering — glide, legato, retrigger, mod wheel.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::{BORDER_ACTIVE, BORDER_INACTIVE, CREAM, MOOG_RED, PANEL, param_line};
use crate::app::{App, Section};

/// Draw the performance section.
pub fn draw_performance(f: &mut Frame, area: Rect, app: &App) {
    let is_active = app.section == Section::Performance;
    let border_color = if is_active {
        BORDER_ACTIVE
    } else {
        BORDER_INACTIVE
    };

    let block = Block::default()
        .title(Span::styled(
            " PERF ",
            Style::new()
                .fg(if is_active { MOOG_RED } else { CREAM })
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::new().fg(border_color))
        .style(Style::new().bg(PANEL));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rate = app.voice.glide.rate;
    let rate_label = if rate < 12.0 {
        "Slow"
    } else if rate < 120.0 {
        "Med"
    } else {
        "Fast"
    };

    let lines = vec![
        param_line(
            "Rate",
            &format!("{rate:.0} st/s {rate_label}"),
            is_active && app.param_index == 0,
        ),
        param_line(
            "Glide",
            if app.voice.glide.enabled { "ON" } else { "OFF" },
            is_active && app.param_index == 1,
        ),
        param_line(
            "Legato",
            if app.voice.legato { "ON" } else { "OFF" },
            is_active && app.param_index == 2,
        ),
        param_line(
            "Retrig",
            if app.voice.retrigger { "ON" } else { "OFF" },
            is_active && app.param_index == 3,
        ),
        param_line(
            "Mod Whl",
            &format!("{}%", (app.voice.xmod.mod_wheel * 100.0) as u32),
            is_active && app.param_index == 4,
        ),
        param_line(
            "Mod Dst",
            app.voice.xmod.mod_wheel_dest.name(),
            is_active && app.param_index == 5,
        ),
    ];

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}
