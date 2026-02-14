//! Mixer channel strip view.
//!
//! Shows vertical channel strips for all tracks plus a master bus strip in
//! the 30-column inspector area. Each strip displays the track number (or
//! "M" for master), a vertical level meter driven by peak meter data,
//! mute/solo indicators, and a pan position readout.

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::{App, FocusedPanel};
use crate::theme;

/// Width of each channel strip in terminal columns.
const STRIP_WIDTH: u16 = 4;

/// Draw the mixer channel strip view into the given area.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = super::panel_block(" Mixer ", FocusedPanel::Mixer, app);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if app.tracks.is_empty() {
        let empty = Paragraph::new("  No tracks").style(theme::style_text_dimmed());
        frame.render_widget(empty, inner);
        return;
    }

    // Calculate how many strips we can fit: all tracks + 1 master.
    let num_strips = app.tracks.len() + 1; // +1 for master
    let visible_strips = (inner.width / STRIP_WIDTH).min(num_strips as u16) as usize;

    if visible_strips == 0 {
        return;
    }

    // Create horizontal layout for strips.
    let constraints: Vec<Constraint> = (0..visible_strips)
        .map(|_| Constraint::Length(STRIP_WIDTH))
        .collect();
    let strip_areas = Layout::horizontal(constraints).split(inner);

    // Draw track strips, then master strip in the last position.
    for (i, &strip_area) in strip_areas.iter().enumerate() {
        if i < app.tracks.len() {
            draw_track_strip(frame, app, i, strip_area);
        } else {
            draw_master_strip(frame, app, strip_area);
        }
    }
}

/// Draw a single track channel strip: number, vertical meter, M/S, pan.
fn draw_track_strip(frame: &mut Frame, app: &App, index: usize, area: Rect) {
    let Some(track) = app.tracks.get(index) else {
        return;
    };
    let is_selected = index == app.selected_track;
    let color = theme::track_color(index);

    // Need at least 4 rows: label + meter (1+) + indicators + pan.
    if area.height < 4 {
        return;
    }

    // Row 0: track number.
    let num_area = Rect { height: 1, ..area };
    let num_style = if is_selected {
        Style::new().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(color)
    };
    let num = Paragraph::new(format!(" {}", index + 1)).style(num_style);
    frame.render_widget(num, num_area);

    // Middle rows: vertical level meter. Height is total minus 3 (label + indicators + pan).
    let meter_height = area.height.saturating_sub(3);
    let meter_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: meter_height,
    };

    // Get peak dB from meter data (max of L/R channels).
    let peak_db = app
        .display
        .mixer
        .track_meters
        .get(index)
        .map_or(-100.0, |m| m.peak_db[0].max(m.peak_db[1]));
    draw_mini_meter(frame, peak_db, meter_area);

    // Second-to-last row: M/S indicators.
    let indicator_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(2),
        width: area.width,
        height: 1,
    };
    let mut indicators: Vec<Span<'_>> = Vec::new();
    if track.muted {
        indicators.push(Span::styled("M", theme::style_muted()));
    } else {
        indicators.push(Span::styled("\u{00b7}", theme::style_text_dimmed()));
    }
    if track.soloed {
        indicators.push(Span::styled("S", theme::style_soloed()));
    } else {
        indicators.push(Span::styled("\u{00b7}", theme::style_text_dimmed()));
    }
    let indicator_line = Paragraph::new(Line::from(indicators));
    frame.render_widget(indicator_line, indicator_area);

    // Last row: pan indicator.
    let pan_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    let pan_val = track.pan.value();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pan_str = if pan_val < -0.05 {
        format!("L{:.0}", pan_val.abs() * 100.0)
    } else if pan_val > 0.05 {
        format!("R{:.0}", pan_val * 100.0)
    } else {
        String::from(" C ")
    };
    let pan = Paragraph::new(pan_str).style(theme::style_text_dimmed());
    frame.render_widget(pan, pan_area);
}

/// Draw the master bus channel strip: label, vertical meter, clip indicator.
fn draw_master_strip(frame: &mut Frame, app: &App, area: Rect) {
    if area.height < 4 {
        return;
    }

    // Row 0: "M" label.
    let label_area = Rect { height: 1, ..area };
    let label = Paragraph::new(" M").style(
        Style::new()
            .fg(theme::FG_PRIMARY)
            .add_modifier(Modifier::BOLD),
    );
    frame.render_widget(label, label_area);

    // Middle rows: vertical meter.
    let meter_height = area.height.saturating_sub(2);
    let meter_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: meter_height,
    };
    let peak_db = app.display.mixer.master_peak_db[0].max(app.display.mixer.master_peak_db[1]);
    draw_mini_meter(frame, peak_db, meter_area);

    // Last row: clip indicator (only when clipping).
    let bottom_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    if app.display.mixer.master_clipping {
        let clip = Paragraph::new("CLP").style(
            Style::new()
                .fg(theme::METER_RED)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(clip, bottom_area);
    }
}

/// Draw a mini vertical level meter filling the given area.
///
/// Maps dB values to a column of filled/empty block characters. The meter
/// fills from the bottom up, with color changing from green to yellow to
/// red as the level increases.
fn draw_mini_meter(frame: &mut Frame, peak_db: f32, area: Rect) {
    let height = area.height as usize;
    if height == 0 || area.width == 0 {
        return;
    }

    // Map dB to fill ratio: -60 dB = empty, 0 dB = full.
    let ratio = ((peak_db + 60.0) / 60.0).clamp(0.0, 1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let filled_rows = (ratio * height as f32) as usize;

    // Limit the rendering width to 2 columns for a clean look.
    let render_width = area.width.min(2);

    for row in 0..height {
        let row_from_bottom = height - 1 - row;
        let y = area.y + row as u16;
        let row_area = Rect {
            x: area.x,
            y,
            width: render_width,
            height: 1,
        };

        let (ch, color) = if row_from_bottom < filled_rows {
            let row_ratio = row_from_bottom as f32 / height as f32;
            ("\u{2588}", theme::meter_color(row_ratio))
        } else {
            ("\u{2591}", theme::FG_DIMMED)
        };

        let cell = Paragraph::new(ch).style(Style::new().fg(color));
        frame.render_widget(cell, row_area);
    }
}
