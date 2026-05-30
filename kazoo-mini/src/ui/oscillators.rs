//! Oscillator section rendering — three VCOs side by side.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::{BORDER_ACTIVE, BORDER_INACTIVE, CREAM, MOOG_RED, PANEL, param_line};
use crate::app::{App, Section};

/// Draw the three oscillator panels side by side (Model D layout).
pub fn draw_oscillators(f: &mut Frame, area: Rect, app: &App) {
    let is_active = app.section == Section::Oscillators;

    // Horizontal layout: VCOs side by side like the real Model D.
    let osc_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(area);

    let oscs = [&app.voice.osc1, &app.voice.osc2, &app.voice.osc3];
    let names = ["VCO 1", "VCO 2", "VCO 3"];
    // Osc1: params 0-3, Osc2: params 4-7, Osc3: params 8-12
    let param_bases = [0, 4, 8];
    let param_counts = [4, 4, 5]; // Osc3 has LFO toggle as 5th

    for (i, (osc, col)) in oscs.iter().zip(osc_cols.iter()).enumerate() {
        let base_idx = param_bases[i];
        let count = param_counts[i];

        // Per-VCO highlight: only highlight the VCO being edited.
        let vco_active =
            is_active && app.param_index >= base_idx && app.param_index < base_idx + count;

        let label = if i == 2 && osc.lfo_mode {
            format!(" {} [LFO] ", names[i])
        } else {
            format!(" {} ", names[i])
        };

        let border_color = if vco_active {
            BORDER_ACTIVE
        } else {
            BORDER_INACTIVE
        };
        let block = Block::default()
            .title(Span::styled(
                label,
                Style::new()
                    .fg(if vco_active { MOOG_RED } else { CREAM })
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::new().fg(border_color))
            .style(Style::new().bg(PANEL));

        let inner = block.inner(*col);
        f.render_widget(block, *col);

        let mut lines = vec![
            param_line(
                "Waveform",
                osc.waveform.name(),
                is_active && app.param_index == base_idx,
            ),
            param_line(
                "Range",
                osc.octave.name(),
                is_active && app.param_index == base_idx + 1,
            ),
            param_line(
                "Tune",
                &format!("{:+.0}c", osc.fine_tune_cents),
                is_active && app.param_index == base_idx + 2,
            ),
            param_line(
                "Volume",
                &format!("{}%", (osc.level * 100.0) as u32),
                is_active && app.param_index == base_idx + 3,
            ),
        ];

        // Osc 3 gets a 5th parameter: LFO mode toggle.
        if i == 2 {
            lines.push(param_line(
                "LFO Mode",
                if osc.lfo_mode { "ON" } else { "OFF" },
                is_active && app.param_index == base_idx + 4,
            ));
        }

        let para = Paragraph::new(lines);
        f.render_widget(para, inner);
    }
}
