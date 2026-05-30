//! Full-width mixing desk / console view.
//!
//! Renders a channel-strip console inspired by 1970s recording studios
//! (Abbey Road, Sun Studios). Each channel strip shows the track name,
//! synthesis mode, pan position, vertical VU meter, dB readout, and
//! solo/mute/arm buttons. A master section with stereo L/R meters is
//! pinned to the right edge.

use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::App;
use crate::theme;
use kazoo_core::synthesis::SynthesisMode;

// ---------------------------------------------------------------------------
// Layout constants
// ---------------------------------------------------------------------------

/// Width of each channel strip in terminal columns.
const CHANNEL_WIDTH: u16 = 14;

/// Width of the master section (including the separator column).
const MASTER_WIDTH: u16 = 16;

/// Width of a single vertical meter bar column.
const METER_BAR_WIDTH: u16 = 2;

/// Minimum dB value shown on the meter scale (below this = silence).
const MIN_DB: f32 = -60.0;

/// Maximum dB value shown on the meter scale.
const MAX_DB: f32 = 0.0;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Draw the full-width mixing desk into the given area.
///
/// This is the centerpiece view of the application, filling the entire
/// content region below the transport bar. It renders channel strips for
/// all visible tracks plus a master bus section on the right.
pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(false))
        .title(" MIXING DESK ")
        .title_style(theme::style_panel_title(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 10 || inner.height < 6 {
        return;
    }

    if app.tracks.is_empty() {
        draw_empty_state(frame, inner);
        return;
    }

    // Reserve space for the master section on the right.
    let master_w = MASTER_WIDTH.min(inner.width);
    let channels_w = inner.width.saturating_sub(master_w);

    if channels_w < CHANNEL_WIDTH {
        // Not enough room for even one channel strip; just draw master.
        draw_master_section(frame, app, inner);
        return;
    }

    let channels_area = Rect {
        x: inner.x,
        y: inner.y,
        width: channels_w,
        height: inner.height,
    };

    let master_area = Rect {
        x: inner.x + channels_w,
        y: inner.y,
        width: master_w,
        height: inner.height,
    };

    draw_channels(frame, app, channels_area);
    draw_master_section(frame, app, master_area);
}

// ---------------------------------------------------------------------------
// Empty state
// ---------------------------------------------------------------------------

/// Render a placeholder message when there are no tracks.
fn draw_empty_state(frame: &mut Frame, area: Rect) {
    let msg = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled("  No tracks", theme::style_text_secondary())),
        Line::from(Span::styled(
            "  Press 'n' to add a track",
            theme::style_text_dimmed(),
        )),
    ]);
    frame.render_widget(msg, area);
}

// ---------------------------------------------------------------------------
// Channel strip area (scrollable)
// ---------------------------------------------------------------------------

/// Draw all visible channel strips, handling scrolling.
fn draw_channels(frame: &mut Frame, app: &mut App, area: Rect) {
    let track_count = app.tracks.len();
    let visible_count = (area.width / CHANNEL_WIDTH).max(1) as usize;
    let visible_count = visible_count.min(track_count);

    // Ensure the selected channel is visible by adjusting scroll.
    let selected = app
        .mixer_view_state
        .selected_channel
        .min(track_count.saturating_sub(1));
    let scroll = &mut app.mixer_view_state.channel_scroll;
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + visible_count {
        *scroll = selected.saturating_sub(visible_count.saturating_sub(1));
    }
    // Clamp scroll to valid range.
    let max_scroll = track_count.saturating_sub(visible_count);
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }
    let scroll_offset = *scroll;

    // Draw each visible channel strip.
    for i in 0..visible_count {
        let track_index = scroll_offset + i;
        if track_index >= track_count {
            break;
        }

        let strip_area = Rect {
            x: area.x + (i as u16) * CHANNEL_WIDTH,
            y: area.y,
            width: CHANNEL_WIDTH.min(area.x + area.width - area.x - (i as u16) * CHANNEL_WIDTH),
            height: area.height,
        };

        draw_channel_strip(frame, app, track_index, strip_area);
    }

    // Draw scroll indicators if there are hidden channels.
    let remaining_x = area.x + (visible_count as u16) * CHANNEL_WIDTH;
    let remaining_w = area
        .width
        .saturating_sub((visible_count as u16) * CHANNEL_WIDTH);

    if scroll_offset > 0 {
        // Left scroll indicator at top-left of channel area.
        let indicator_area = Rect {
            x: area.x,
            y: area.y,
            width: 2,
            height: 1,
        };
        let indicator =
            Paragraph::new(Span::styled("\u{25c0}\u{2500}", theme::style_text_dimmed()));
        frame.render_widget(indicator, indicator_area);
    }

    if scroll_offset + visible_count < track_count && remaining_w >= 2 {
        // Right scroll indicator.
        let indicator_area = Rect {
            x: remaining_x.min(area.x + area.width - 2),
            y: area.y,
            width: 2,
            height: 1,
        };
        let hidden = track_count - (scroll_offset + visible_count);
        let label = format!("+{hidden}");
        let indicator = Paragraph::new(Span::styled(label, theme::style_text_dimmed()));
        frame.render_widget(indicator, indicator_area);
    }
}

// ---------------------------------------------------------------------------
// Single channel strip
// ---------------------------------------------------------------------------

/// Draw one channel strip at the given position.
///
/// Layout (top to bottom, fitting into the available height):
///   Row 0:      Channel number ("Ch 1")
///   Row 1:      Track name (truncated)
///   Row 2:      Synth mode abbreviation
///   Row 3:      (blank spacer)
///   Row 4:      Pan display
///   Row 5:      (blank spacer)
///   Rows 6..N-4: Vertical VU meter
///   Row N-3:    dB readout
///   Row N-2:    [S] [M] buttons
///   Row N-1:    [R] arm button
#[allow(clippy::too_many_lines)]
fn draw_channel_strip(frame: &mut Frame, app: &App, index: usize, area: Rect) {
    let Some(track) = app.tracks.get(index) else {
        return;
    };
    let is_selected = index == app.mixer_view_state.selected_channel;
    let color = theme::track_color(index);

    if area.height < 8 || area.width < 4 {
        return;
    }

    // --- Row 0: Channel number ---
    let ch_label = format!(" Ch {}", index + 1);
    let ch_style = if is_selected {
        Style::new()
            .fg(theme::BG_PRIMARY)
            .bg(theme::ACCENT_FOCUS)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(color).add_modifier(Modifier::BOLD)
    };
    render_row(frame, &ch_label, ch_style, area, 0);

    // --- Row 1: Track name (truncated to strip width - 2) ---
    let max_name_len = (area.width as usize).saturating_sub(2);
    let name = truncate_str(&track.name, max_name_len);
    let name_style = if is_selected {
        Style::new().fg(color).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(color)
    };
    render_row(frame, &format!(" {name}"), name_style, area, 1);

    // --- Row 2: Synth mode abbreviation ---
    let mode_abbr = synth_mode_abbrev(track.synthesis_mode);
    render_row(
        frame,
        &format!(" [{mode_abbr}]"),
        theme::style_text_secondary(),
        area,
        2,
    );

    // --- Row 3: blank spacer ---
    // (no rendering needed)

    // --- Row 4: Pan display ---
    if area.height > 10 {
        let pan_str = format_pan(track.pan.value(), area.width);
        render_row(frame, &pan_str, theme::style_text_secondary(), area, 4);
    }

    // --- Vertical VU meter ---
    // Reserve rows from the bottom: 3 rows for dB readout + S/M + R.
    // Reserve rows from the top: 6 rows for header section.
    let meter_top = 6u16;
    let meter_bottom_margin = 3u16;
    let meter_height = area.height.saturating_sub(meter_top + meter_bottom_margin);

    if meter_height >= 2 {
        let meter_area = Rect {
            x: area.x + (area.width / 2).saturating_sub(METER_BAR_WIDTH / 2),
            y: area.y + meter_top,
            width: METER_BAR_WIDTH.min(area.width),
            height: meter_height,
        };

        // Get meter data for this track.
        let (peak_db, rms_db) = app
            .display
            .mixer
            .track_meters
            .get(index)
            .map_or((-100.0_f32, -100.0_f32), |m| {
                (m.peak_db[0].max(m.peak_db[1]), m.rms_db[0].max(m.rms_db[1]))
            });

        draw_vertical_meter(frame, meter_area, peak_db, rms_db);
    }

    // --- Bottom rows (from the bottom up) ---
    let bottom = area.y + area.height;

    // Row N-1: [R] arm button.
    if bottom >= 1 {
        let arm_y = bottom - 1;
        let arm_area = Rect {
            x: area.x,
            y: arm_y,
            width: area.width,
            height: 1,
        };
        let arm_style = if track.armed {
            theme::style_armed()
        } else {
            theme::style_text_dimmed()
        };
        let arm_label = if track.armed { " \u{25cf}R" } else { " [R]" };
        frame.render_widget(Paragraph::new(arm_label).style(arm_style), arm_area);
    }

    // Row N-2: [S] [M] buttons.
    if bottom >= 2 {
        let sm_y = bottom - 2;
        let sm_area = Rect {
            x: area.x,
            y: sm_y,
            width: area.width,
            height: 1,
        };
        let solo_span = if track.soloed {
            Span::styled("[S]", theme::style_soloed())
        } else {
            Span::styled("[S]", theme::style_text_dimmed())
        };
        let mute_span = if track.muted {
            Span::styled("[M]", theme::style_muted())
        } else {
            Span::styled("[M]", theme::style_text_dimmed())
        };
        let line = Line::from(vec![Span::raw(" "), solo_span, Span::raw(" "), mute_span]);
        frame.render_widget(Paragraph::new(line), sm_area);
    }

    // Row N-3: dB readout.
    if bottom >= 3 {
        let db_y = bottom - 3;
        let db_area = Rect {
            x: area.x,
            y: db_y,
            width: area.width,
            height: 1,
        };
        let peak_db = app
            .display
            .mixer
            .track_meters
            .get(index)
            .map_or(-100.0_f32, |m| m.peak_db[0].max(m.peak_db[1]));
        let db_str = format_db(peak_db);
        let db_color = theme::meter_color_db(peak_db);
        frame.render_widget(
            Paragraph::new(db_str).style(Style::new().fg(db_color)),
            db_area,
        );
    }
}

// ---------------------------------------------------------------------------
// Master section
// ---------------------------------------------------------------------------

/// Draw the master bus section on the right side of the mixing desk.
///
/// Includes a separator column, "MASTER" title, stereo L/R VU meters,
/// dB readout, and clipping indicator.
#[allow(clippy::too_many_lines)]
fn draw_master_section(frame: &mut Frame, app: &App, area: Rect) {
    if area.width < 4 || area.height < 6 {
        return;
    }

    // Draw separator line down the left edge.
    for row in 0..area.height {
        let sep_area = Rect {
            x: area.x,
            y: area.y + row,
            width: 1,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new("\u{2551}").style(theme::style_panel_border(true)),
            sep_area,
        );
    }

    // Content area (to the right of separator).
    let content = Rect {
        x: area.x + 2,
        y: area.y,
        width: area.width.saturating_sub(3),
        height: area.height,
    };

    if content.width < 2 {
        return;
    }

    // Row 0: "MASTER" title.
    let title_area = Rect {
        height: 1,
        ..content
    };
    frame.render_widget(
        Paragraph::new("MASTER").style(
            Style::new()
                .fg(theme::FG_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        title_area,
    );

    // Row 1: Master volume readout.
    if content.height > 1 {
        let vol_area = Rect {
            x: content.x,
            y: content.y + 1,
            width: content.width,
            height: 1,
        };
        let vol_db = app.master_volume.value();
        let vol_str = if vol_db.abs() < 0.05 {
            String::from("0.0 dB")
        } else {
            format!("{vol_db:+.1} dB")
        };
        frame.render_widget(
            Paragraph::new(vol_str).style(theme::style_text_secondary()),
            vol_area,
        );
    }

    // Meters: stereo L/R vertical bars.
    // Reserve top 3 rows (title + vol + spacer) and bottom 4 rows (readout + clip).
    let meter_top_offset = 3u16;
    let meter_bottom_margin = 4u16;
    let meter_height = content
        .height
        .saturating_sub(meter_top_offset + meter_bottom_margin);

    if meter_height >= 2 {
        let meter_total_width = (METER_BAR_WIDTH * 2 + 1).min(content.width);
        let meter_x = content.x;

        // "L" and "R" labels above the meters.
        let l_label_area = Rect {
            x: meter_x,
            y: content.y + 2,
            width: METER_BAR_WIDTH,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(" L").style(theme::style_text_secondary()),
            l_label_area,
        );

        if meter_total_width > METER_BAR_WIDTH + 1 {
            let r_label_area = Rect {
                x: meter_x + METER_BAR_WIDTH + 1,
                y: content.y + 2,
                width: METER_BAR_WIDTH,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(" R").style(theme::style_text_secondary()),
                r_label_area,
            );
        }

        // Left channel meter.
        let l_meter_area = Rect {
            x: meter_x,
            y: content.y + meter_top_offset,
            width: METER_BAR_WIDTH,
            height: meter_height,
        };
        draw_vertical_meter(
            frame,
            l_meter_area,
            app.display.mixer.master_peak_db[0],
            app.display.mixer.master_rms_db[0],
        );

        // Right channel meter.
        if meter_total_width > METER_BAR_WIDTH + 1 {
            let r_meter_area = Rect {
                x: meter_x + METER_BAR_WIDTH + 1,
                y: content.y + meter_top_offset,
                width: METER_BAR_WIDTH,
                height: meter_height,
            };
            draw_vertical_meter(
                frame,
                r_meter_area,
                app.display.mixer.master_peak_db[1],
                app.display.mixer.master_rms_db[1],
            );
        }
    }

    // Bottom rows: dB readouts and clip indicator.
    let bottom = content.y + content.height;

    // Row N-1: Clipping indicator.
    if bottom >= 1 {
        let clip_y = bottom - 1;
        let clip_area = Rect {
            x: content.x,
            y: clip_y,
            width: content.width,
            height: 1,
        };
        if app.display.mixer.master_clipping {
            frame.render_widget(
                Paragraph::new("!! CLIP !!").style(
                    Style::new()
                        .fg(theme::METER_RED)
                        .add_modifier(Modifier::BOLD),
                ),
                clip_area,
            );
        } else {
            frame.render_widget(
                Paragraph::new("OK").style(theme::style_text_dimmed()),
                clip_area,
            );
        }
    }

    // Row N-2: R channel dB.
    if bottom >= 2 {
        let r_db_y = bottom - 2;
        let r_db = app.display.mixer.master_peak_db[1];
        let r_str = format!("R: {r_db:>6.1}dB");
        let r_area = Rect {
            x: content.x,
            y: r_db_y,
            width: content.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(r_str).style(Style::new().fg(theme::meter_color_db(r_db))),
            r_area,
        );
    }

    // Row N-3: L channel dB.
    if bottom >= 3 {
        let l_db_y = bottom - 3;
        let l_db = app.display.mixer.master_peak_db[0];
        let l_str = format!("L: {l_db:>6.1}dB");
        let l_area = Rect {
            x: content.x,
            y: l_db_y,
            width: content.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(l_str).style(Style::new().fg(theme::meter_color_db(l_db))),
            l_area,
        );
    }

    // Row N-4: blank spacer (no rendering needed).
}

// ---------------------------------------------------------------------------
// Vertical VU meter (shared between channel and master)
// ---------------------------------------------------------------------------

/// Draw a vertical VU meter with RMS fill and peak indicator.
///
/// The meter fills from bottom to top. RMS level is shown as solid blocks,
/// the region between RMS and peak is shown as medium-shade blocks, and a
/// peak-hold line sits at the peak position. Each row is colored according
/// to its dB position on the meter scale (green/yellow/red).
fn draw_vertical_meter(frame: &mut Frame, area: Rect, peak_db: f32, rms_db: f32) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let height = area.height as usize;
    let peak_ratio = db_to_ratio(peak_db);
    let rms_ratio = db_to_ratio(rms_db);

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let peak_rows = (peak_ratio * height as f32).ceil() as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rms_rows = (rms_ratio * height as f32).ceil() as usize;

    for row in 0..height {
        let row_from_bottom = height - 1 - row;
        let y = area.y + row as u16;
        let cell_area = Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        };

        // Determine what dB this row represents for coloring.
        #[allow(clippy::cast_precision_loss)]
        let row_db = (MAX_DB - MIN_DB).mul_add(row_from_bottom as f32 / height as f32, MIN_DB);

        if row_from_bottom < rms_rows {
            // Solid fill: RMS level.
            let bar = "\u{2588}".repeat(area.width as usize);
            let style = Style::new().fg(theme::meter_color_db(row_db));
            frame.render_widget(Paragraph::new(bar).style(style), cell_area);
        } else if row_from_bottom < peak_rows {
            // Medium shade: between RMS and peak.
            let bar = "\u{2592}".repeat(area.width as usize);
            let style = Style::new().fg(theme::meter_color_db(row_db));
            frame.render_widget(Paragraph::new(bar).style(style), cell_area);
        } else if row_from_bottom == peak_rows && peak_rows > 0 {
            // Peak-hold line.
            let bar = "\u{2594}".repeat(area.width as usize);
            let style = Style::new().fg(theme::meter_color_db(row_db));
            frame.render_widget(Paragraph::new(bar).style(style), cell_area);
        } else {
            // Empty row.
            let bar = "\u{00b7}".repeat(area.width as usize);
            frame.render_widget(
                Paragraph::new(bar).style(theme::style_text_dimmed()),
                cell_area,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Map a dB value to a 0.0..1.0 ratio within the meter range.
///
/// Returns 0.0 for NaN/Inf inputs (NaN defense per CLAUDE.md).
fn db_to_ratio(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    ((db - MIN_DB) / (MAX_DB - MIN_DB)).clamp(0.0, 1.0)
}

/// Format a dB value for display under a meter.
///
/// Shows "-inf" for very low values and non-finite inputs, and a compact
/// numeric display otherwise.
fn format_db(db: f32) -> String {
    if !db.is_finite() || db <= MIN_DB {
        String::from(" -inf dB")
    } else {
        format!("{db:>5.1} dB")
    }
}

/// Format the pan position as a visual string.
///
/// Center:  " \u{256c}\u{256c}\u{25cf}\u{256c}\u{256c} "
/// Left:    "\u{25c1}\u{2500}\u{25cf}\u{2500}\u{2500}"
/// Right:   "\u{2500}\u{2500}\u{25cf}\u{2500}\u{25b7}"
fn format_pan(pan: f32, width: u16) -> String {
    let display_width = (width as usize).saturating_sub(2).max(5);
    // Map pan from [-1, 1] to [0, display_width-1].
    let positions = display_width.saturating_sub(1);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::manual_midpoint
    )]
    let knob_pos = (((pan + 1.0) / 2.0) * positions as f32)
        .round()
        .clamp(0.0, positions as f32) as usize;

    let mut result = String::with_capacity(display_width + 2);
    result.push(' ');

    for i in 0..display_width {
        if i == knob_pos {
            result.push('\u{25cf}'); // filled circle (knob)
        } else if i == 0 {
            result.push('\u{25c1}'); // left arrow
        } else if i == display_width - 1 {
            result.push('\u{25b7}'); // right arrow
        } else {
            result.push('\u{2500}'); // horizontal line
        }
    }

    result
}

/// Get a short abbreviation for a synthesis mode.
const fn synth_mode_abbrev(mode: SynthesisMode) -> &'static str {
    match mode {
        SynthesisMode::Passthrough => "Raw",
        SynthesisMode::PitchTracked => "Pt",
        SynthesisMode::Wavetable => "Wt",
        SynthesisMode::Granular => "Gr",
        SynthesisMode::Vocoder => "Vc",
        SynthesisMode::PhaseVocoder => "Pv",
    }
}

/// Truncate a string to at most `max_chars` characters, appending an
/// ellipsis if truncation occurred. Handles UTF-8 boundaries safely.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }
    // Reserve one character for the ellipsis.
    let truncated_len = max_chars.saturating_sub(1);
    let mut result: String = s.chars().take(truncated_len).collect();
    result.push('\u{2026}'); // ellipsis
    result
}

/// Render a single row of text in a channel strip.
fn render_row(frame: &mut Frame, text: &str, style: Style, area: Rect, row_offset: u16) {
    if row_offset >= area.height {
        return;
    }
    let row_area = Rect {
        x: area.x,
        y: area.y + row_offset,
        width: area.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(text.to_string()).style(style), row_area);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- db_to_ratio --------------------------------------------------------

    #[test]
    fn db_to_ratio_at_min_returns_zero() {
        assert!((db_to_ratio(MIN_DB) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_at_max_returns_one() {
        assert!((db_to_ratio(MAX_DB) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_midpoint() {
        let mid = (MIN_DB + MAX_DB) / 2.0;
        assert!((db_to_ratio(mid) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_below_min_clamped() {
        assert!((db_to_ratio(-120.0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_above_max_clamped() {
        assert!((db_to_ratio(6.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_nan_returns_zero() {
        assert!((db_to_ratio(f32::NAN) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_inf_returns_zero() {
        assert!((db_to_ratio(f32::INFINITY) - 0.0).abs() < f32::EPSILON);
        assert!((db_to_ratio(f32::NEG_INFINITY) - 0.0).abs() < f32::EPSILON);
    }

    // -- format_db ----------------------------------------------------------

    #[test]
    fn format_db_normal_value() {
        let s = format_db(-12.3);
        assert!(s.contains("-12.3"));
        assert!(s.contains("dB"));
    }

    #[test]
    fn format_db_zero() {
        let s = format_db(0.0);
        assert!(s.contains("0.0"));
    }

    #[test]
    fn format_db_below_min_shows_inf() {
        assert!(format_db(-120.0).contains("-inf"));
    }

    #[test]
    fn format_db_nan_shows_inf() {
        assert!(format_db(f32::NAN).contains("-inf"));
    }

    #[test]
    fn format_db_infinity_shows_inf() {
        assert!(format_db(f32::INFINITY).contains("-inf"));
    }

    // -- format_pan ---------------------------------------------------------

    #[test]
    fn format_pan_center_has_knob_in_middle() {
        let pan = format_pan(0.0, 12);
        // Knob (●) should be roughly in the middle.
        assert!(pan.contains('\u{25cf}'));
    }

    #[test]
    fn format_pan_hard_left() {
        let pan = format_pan(-1.0, 12);
        // Knob should be at position 0 (which replaces the left arrow).
        let chars: Vec<char> = pan.chars().collect();
        // First non-space char should be the knob.
        assert_eq!(chars[1], '\u{25cf}');
    }

    #[test]
    fn format_pan_hard_right() {
        let pan = format_pan(1.0, 12);
        let chars: Vec<char> = pan.chars().collect();
        // Last char should be the knob.
        let last = chars[chars.len() - 1];
        assert_eq!(last, '\u{25cf}');
    }

    #[test]
    fn format_pan_narrow_width() {
        // Minimum usable width.
        let pan = format_pan(0.0, 4);
        assert!(pan.contains('\u{25cf}'));
    }

    // -- synth_mode_abbrev --------------------------------------------------

    #[test]
    fn synth_mode_abbrev_all_modes() {
        assert_eq!(synth_mode_abbrev(SynthesisMode::Passthrough), "Raw");
        assert_eq!(synth_mode_abbrev(SynthesisMode::PitchTracked), "Pt");
        assert_eq!(synth_mode_abbrev(SynthesisMode::Wavetable), "Wt");
        assert_eq!(synth_mode_abbrev(SynthesisMode::Granular), "Gr");
        assert_eq!(synth_mode_abbrev(SynthesisMode::Vocoder), "Vc");
        assert_eq!(synth_mode_abbrev(SynthesisMode::PhaseVocoder), "Pv");
    }

    // -- truncate_str -------------------------------------------------------

    #[test]
    fn truncate_str_short_unchanged() {
        assert_eq!(truncate_str("abc", 5), "abc");
    }

    #[test]
    fn truncate_str_exact_length_unchanged() {
        assert_eq!(truncate_str("abcde", 5), "abcde");
    }

    #[test]
    fn truncate_str_over_limit_adds_ellipsis() {
        let s = truncate_str("Hello World", 6);
        assert_eq!(s.chars().count(), 6);
        assert!(s.ends_with('\u{2026}')); // ellipsis
    }

    #[test]
    fn truncate_str_zero_max_returns_empty() {
        assert_eq!(truncate_str("anything", 0), "");
    }

    #[test]
    fn truncate_str_unicode_safe() {
        // Each emoji is 1 char but multi-byte.
        let s = truncate_str("🎹🎵🎶🎤", 3);
        assert_eq!(s.chars().count(), 3);
        assert!(s.ends_with('\u{2026}'));
    }
}
