//! VU meters and level indicators.
//!
//! Renders vertical stereo VU meters for the master bus. Each meter
//! fills from bottom to top based on the peak level, with a color
//! gradient (green / yellow / red), a peak-hold indicator, numeric dB
//! readout, and a clipping warning.

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::{App, FocusedPanel};
use crate::theme;

/// Minimum dB value shown on the meter scale.
const MIN_DB: f32 = -60.0;

/// Maximum dB value shown on the meter scale.
const MAX_DB: f32 = 0.0;

/// Map a dB value to a 0..1 ratio within the meter range.
#[must_use]
fn db_to_ratio(db: f32) -> f32 {
    ((db - MIN_DB) / (MAX_DB - MIN_DB)).clamp(0.0, 1.0)
}

/// Draw the master VU meters into the given area.
///
/// The panel is associated with the `Mixer` focus for keyboard navigation
/// since meters and mixer share the same conceptual region.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = super::panel_block(" Meters ", FocusedPanel::Mixer, app);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 || inner.width < 6 {
        // Not enough space to draw anything meaningful.
        return;
    }

    let master = &app.display.mixer;

    // Split the inner area into: L meter | gap | R meter | readout.
    // Each meter bar is 2 columns wide.
    let meter_width: u16 = 2;
    let gap: u16 = 1;
    let readout_width: u16 = inner.width.saturating_sub(meter_width * 2 + gap + 1);

    let left_area = Rect {
        x: inner.x,
        y: inner.y,
        width: meter_width,
        height: inner.height,
    };

    let right_area = Rect {
        x: inner.x + meter_width + gap,
        y: inner.y,
        width: meter_width,
        height: inner.height,
    };

    let readout_area = Rect {
        x: inner.x + meter_width * 2 + gap + 1,
        y: inner.y,
        width: readout_width.min(inner.width),
        height: inner.height,
    };

    // Draw the two vertical meter bars.
    draw_vertical_meter(
        frame,
        left_area,
        master.master_peak_db[0],
        master.master_rms_db[0],
    );
    draw_vertical_meter(
        frame,
        right_area,
        master.master_peak_db[1],
        master.master_rms_db[1],
    );

    // Build the readout text in the remaining space.
    draw_readout(frame, readout_area, app);
}

/// Draw a single vertical meter bar filling from bottom to top.
///
/// Uses full-block characters (`\u{2588}`) for filled rows and a dim
/// dot for empty rows. Each row is colored according to its dB position
/// on the meter scale using `meter_color_db`.
fn draw_vertical_meter(frame: &mut Frame, area: Rect, peak_db: f32, rms_db: f32) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let height = area.height as usize;
    let peak_ratio = db_to_ratio(peak_db);
    let rms_ratio = db_to_ratio(rms_db);

    // Number of rows filled by peak and RMS respectively.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let peak_rows = (peak_ratio * height as f32).ceil() as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rms_rows = (rms_ratio * height as f32).ceil() as usize;

    // Render each row from top (row 0 = highest dB) to bottom (row height-1 = lowest dB).
    for row in 0..height {
        // Which dB does this row represent? Top = MAX_DB, bottom = MIN_DB.
        let row_from_bottom = height - 1 - row;
        #[allow(clippy::cast_precision_loss)]
        let row_db = (MAX_DB - MIN_DB).mul_add(row_from_bottom as f32 / height as f32, MIN_DB);

        let y = area.y + row as u16;
        let cell_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };

        if row_from_bottom < rms_rows {
            // Filled by RMS level: solid block with level-appropriate color.
            let bar_text = "\u{2588}".repeat(area.width as usize);
            let style = Style::new().fg(theme::meter_color_db(row_db));
            frame.render_widget(Paragraph::new(bar_text).style(style), cell_area);
        } else if row_from_bottom < peak_rows {
            // Between RMS and peak: half-block to show peak extent.
            let bar_text = "\u{2592}".repeat(area.width as usize);
            let style = Style::new().fg(theme::meter_color_db(row_db));
            frame.render_widget(Paragraph::new(bar_text).style(style), cell_area);
        } else if row_from_bottom == peak_rows && peak_rows > 0 {
            // Peak-hold indicator: a single line at the peak position.
            let bar_text = "\u{2594}".repeat(area.width as usize);
            let style = Style::new().fg(theme::meter_color_db(row_db));
            frame.render_widget(Paragraph::new(bar_text).style(style), cell_area);
        } else {
            // Empty row.
            let bar_text = "\u{00b7}".repeat(area.width as usize);
            frame.render_widget(
                Paragraph::new(bar_text).style(theme::style_text_dimmed()),
                cell_area,
            );
        }
    }
}

/// Draw the numeric readout and labels next to the meters.
fn draw_readout(frame: &mut Frame, area: Rect, app: &App) {
    if area.width < 4 || area.height < 3 {
        return;
    }

    let master = &app.display.mixer;

    let mut lines: Vec<Line<'_>> = Vec::new();

    // Label.
    lines.push(Line::from(Span::styled("Master", theme::style_text())));
    lines.push(Line::from(""));

    // L channel dB.
    let l_db = master.master_peak_db[0];
    let l_str = format!("L: {l_db:>6.1}dB");
    lines.push(Line::from(Span::styled(
        l_str,
        Style::new().fg(theme::meter_color_db(l_db)),
    )));

    // R channel dB.
    let r_db = master.master_peak_db[1];
    let r_str = format!("R: {r_db:>6.1}dB");
    lines.push(Line::from(Span::styled(
        r_str,
        Style::new().fg(theme::meter_color_db(r_db)),
    )));

    lines.push(Line::from(""));

    // Clipping indicator.
    if master.master_clipping {
        lines.push(Line::from(Span::styled(
            "!! CLIP !!",
            Style::new()
                .fg(theme::METER_RED)
                .add_modifier(Modifier::BOLD),
        )));
    } else {
        lines.push(Line::from(Span::styled("OK", theme::style_text_dimmed())));
    }

    // Scale markers (if space permits).
    if area.height > 8 {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " 0dB ---",
            theme::style_text_dimmed(),
        )));
        lines.push(Line::from(Span::styled(
            "-6dB ---",
            theme::style_text_dimmed(),
        )));
        lines.push(Line::from(Span::styled(
            "-20  ---",
            theme::style_text_dimmed(),
        )));
        lines.push(Line::from(Span::styled(
            "-60  ---",
            theme::style_text_dimmed(),
        )));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}
