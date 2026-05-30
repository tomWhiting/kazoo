//! Filter section rendering — ladder filter with cross-mod controls.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::{BORDER_ACTIVE, BORDER_INACTIVE, CREAM, DIM, MOOG_RED, PANEL, param_line};
use crate::app::{App, Section};

/// Draw the filter section: ladder filter + cross-mod + drive.
pub fn draw_filter(f: &mut Frame, area: Rect, app: &App) {
    let is_active = app.section == Section::Filter;
    let border_color = if is_active {
        BORDER_ACTIVE
    } else {
        BORDER_INACTIVE
    };

    let block = Block::default()
        .title(Span::styled(
            " FILTER ",
            Style::new()
                .fg(if is_active { MOOG_RED } else { CREAM })
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(
            " 24dB/oct ladder ",
            Style::new().fg(DIM),
        )))
        .borders(Borders::ALL)
        .border_style(Style::new().fg(border_color))
        .style(Style::new().bg(PANEL));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let cutoff = app.voice.filter.base_cutoff;
    let cutoff_str = if cutoff >= 1000.0 {
        format!("{:.1}k", cutoff / 1000.0)
    } else {
        format!("{cutoff:.0}")
    };

    let key_track_pct = (app.voice.filter.key_track * 100.0) as u32;

    let drive = app.voice.filter.drive();
    let drive_str = format!("{drive:.1}x");

    let mut lines = vec![
        param_line(
            "Cutoff",
            &format!("{cutoff_str}Hz"),
            is_active && app.param_index == 0,
        ),
        param_line(
            "Emphasis",
            &format!("{}%", (app.voice.filter.resonance() * 100.0) as u32),
            is_active && app.param_index == 1,
        ),
        param_line(
            "Contour",
            &format!("{}%", (app.voice.filter_env_amount * 100.0) as u32),
            is_active && app.param_index == 2,
        ),
        param_line(
            "Key Trk",
            &format!("{key_track_pct}%"),
            is_active && app.param_index == 3,
        ),
        param_line("Drive", &drive_str, is_active && app.param_index == 4),
    ];

    // Cross-mod section — editable.
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " CROSS-MOD",
        Style::new().fg(MOOG_RED).add_modifier(Modifier::BOLD),
    )));
    lines.push(param_line(
        "O3>O2",
        &if app.voice.xmod.osc3_to_osc2_fm > 0.0 {
            format!("{}%", (app.voice.xmod.osc3_to_osc2_fm * 100.0) as u32)
        } else {
            "Off".to_string()
        },
        is_active && app.param_index == 5,
    ));
    lines.push(param_line(
        "O2>Flt",
        &if app.voice.xmod.osc2_to_filter > 0.0 {
            format!("{}%", (app.voice.xmod.osc2_to_filter * 100.0) as u32)
        } else {
            "Off".to_string()
        },
        is_active && app.param_index == 6,
    ));

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}
