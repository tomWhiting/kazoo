//! Timeline panel: clip arrangement view with playhead and time ruler.
//!
//! Displays a horizontal scrollable timeline with one row per track. Each clip
//! is drawn as a colored rectangle with a mini waveform overview. A vertical
//! playhead line shows the current transport position, and active recordings
//! are shown as growing red rectangles.

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use kazoo_core::engine::{ClipSnapshot, TimelineSnapshot, TrackClipSnapshot};
use kazoo_core::transport::TransportState;

use crate::app::{App, FocusedPanel};
use crate::theme;

/// Minimum width in terminal columns for the timeline to be renderable.
const MIN_WIDTH: u16 = 10;

/// Height of the time ruler at the bottom of the timeline.
const RULER_HEIGHT: u16 = 1;

/// Draw the timeline panel into the given area.
///
/// Renders track rows with clip rectangles, a playhead, and a time ruler.
/// When no clips exist and no recording is active, shows a placeholder message.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = super::panel_block(" Timeline ", FocusedPanel::Timeline, app);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < MIN_WIDTH || inner.height < 2 {
        return;
    }

    let timeline = &app.display.timeline;
    let has_content = timeline
        .tracks
        .iter()
        .any(|t| !t.clips.is_empty() || t.is_recording_clip);

    if !has_content {
        let empty =
            Paragraph::new("  No clips\n  Record or load audio").style(theme::style_text_dimmed());
        frame.render_widget(empty, inner);
        return;
    }

    let sample_rate = app.engine.sample_rate();

    // Split: track rows on top, ruler at bottom.
    let chunks =
        Layout::vertical([Constraint::Min(1), Constraint::Length(RULER_HEIGHT)]).split(inner);

    let track_area = chunks[0];
    let ruler_area = chunks[1];

    // Calculate visible range in samples.
    let samples_per_col = app.timeline_zoom;
    let view_start = app.timeline_scroll;
    let view_end = view_start + (f64::from(track_area.width) * samples_per_col);

    // Draw track rows.
    draw_track_rows(
        frame,
        app,
        timeline,
        track_area,
        view_start,
        view_end,
        samples_per_col,
        sample_rate,
    );

    // Draw playhead.
    draw_playhead(frame, app, track_area, view_start, samples_per_col);

    // Draw time ruler.
    draw_ruler(frame, ruler_area, view_start, samples_per_col, sample_rate);
}

/// Draw one row per track with clip rectangles.
#[allow(clippy::too_many_arguments)]
fn draw_track_rows(
    frame: &mut Frame,
    app: &App,
    timeline: &TimelineSnapshot,
    area: Rect,
    view_start: f64,
    view_end: f64,
    samples_per_col: f64,
    sample_rate: u32,
) {
    let num_tracks = timeline.tracks.len().max(1);
    let row_height = (area.height as usize / num_tracks).max(1) as u16;

    for (i, track) in timeline.tracks.iter().enumerate() {
        let y = area.y + (i as u16 * row_height);
        if y >= area.y + area.height {
            break;
        }
        let h = row_height.min(area.y + area.height - y);
        let row_area = Rect::new(area.x, y, area.width, h);

        draw_track_row(
            frame,
            app,
            track,
            i,
            row_area,
            view_start,
            view_end,
            samples_per_col,
            sample_rate,
        );
    }
}

/// Draw a single track row with lane background, colored edge bar, and label.
#[allow(clippy::too_many_arguments)]
fn draw_track_row(
    frame: &mut Frame,
    app: &App,
    track: &TrackClipSnapshot,
    track_index: usize,
    area: Rect,
    view_start: f64,
    view_end: f64,
    samples_per_col: f64,
    _sample_rate: u32,
) {
    let track_col = theme::track_color(track_index);
    let lane_bg = theme::lane_bg(track_index);

    // Fill entire row with alternating lane background for visual separation.
    let fill = " ".repeat(area.width as usize);
    for row in 0..area.height {
        let row_rect = Rect::new(area.x, area.y + row, area.width, 1);
        frame.render_widget(
            Paragraph::new(fill.clone()).style(Style::new().bg(lane_bg)),
            row_rect,
        );
    }

    // Track label area: 1 col color bar + 5 cols name.
    let label_width = 6_u16.min(area.width);

    // Column 0: colored vertical bar for track identity.
    for row in 0..area.height {
        let bar_area = Rect::new(area.x, area.y + row, 1, 1);
        frame.render_widget(
            Paragraph::new("\u{258E}").style(Style::new().fg(track_col).bg(lane_bg)),
            bar_area,
        );
    }

    // Columns 1-5: track name (truncated to 4 chars).
    if label_width > 1 && area.height > 0 {
        let name = truncate_str(&track.track_name, 4);
        let label_text = format!(" {name:<4}");
        let text_area = Rect::new(area.x + 1, area.y, label_width - 1, 1);
        frame.render_widget(
            Paragraph::new(label_text)
                .style(Style::new().fg(track_col).bg(lane_bg).add_modifier(Modifier::BOLD)),
            text_area,
        );
    }

    let clip_area_x = area.x + label_width;
    let clip_area_width = area.width.saturating_sub(label_width);
    if clip_area_width == 0 {
        return;
    }
    let clip_area = Rect::new(clip_area_x, area.y, clip_area_width, area.height);

    // Draw clips.
    for clip in &track.clips {
        draw_clip(
            frame,
            app,
            clip,
            clip_area,
            view_start,
            view_end,
            samples_per_col,
            track_col,
        );
    }

    // Draw active recording.
    if track.is_recording_clip {
        draw_recording(frame, app, track, clip_area, view_start, samples_per_col);
    }
}

/// Draw a single clip as a colored rectangle with mini waveform.
#[allow(clippy::too_many_arguments)]
fn draw_clip(
    frame: &mut Frame,
    app: &App,
    clip: &ClipSnapshot,
    area: Rect,
    view_start: f64,
    view_end: f64,
    samples_per_col: f64,
    track_color: Color,
) {
    let clip_start = clip.position as f64;
    let clip_end = clip_start + clip.length as f64;

    // Skip clips entirely outside the visible range.
    if clip_end <= view_start || clip_start >= view_end {
        return;
    }

    // Calculate column range.
    let col_start = ((clip_start - view_start) / samples_per_col).max(0.0) as u16;
    let col_end = ((clip_end - view_start) / samples_per_col).min(f64::from(area.width)) as u16;

    if col_start >= col_end || col_start >= area.width {
        return;
    }

    let clip_width = col_end - col_start;
    let clip_rect = Rect::new(area.x + col_start, area.y, clip_width, area.height);

    // Determine clip style: selected clips get a highlight.
    let is_selected = app.selected_clip.is_some_and(|id| id.0 == clip.id);

    let (fg, bg) = if clip.muted {
        (theme::FG_DIMMED, theme::BG_SURFACE)
    } else if is_selected {
        (theme::BG_PRIMARY, track_color)
    } else {
        (track_color, theme::BG_SURFACE)
    };

    // Build the clip display: name on top row, waveform overview below.
    if area.height >= 2 {
        // Top line: clip name (truncated to fit).
        let name = truncate_str(&clip.name, clip_width as usize);
        let name_area = Rect::new(clip_rect.x, clip_rect.y, clip_rect.width, 1);
        let name_widget =
            Paragraph::new(name).style(Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD));
        frame.render_widget(name_widget, name_area);

        // Below: mini waveform from overview data.
        let wave_height = clip_rect.height.saturating_sub(1);
        if wave_height > 0 {
            let wave_area = Rect::new(clip_rect.x, clip_rect.y + 1, clip_rect.width, wave_height);
            draw_mini_waveform(frame, &clip.waveform_overview, wave_area, fg, bg);
        }
    } else {
        // Single row: just show a colored bar.
        let bar = "\u{2584}".repeat(clip_width as usize);
        let bar_area = Rect::new(clip_rect.x, clip_rect.y, clip_rect.width, 1);
        let bar_widget = Paragraph::new(bar).style(Style::new().fg(fg).bg(bg));
        frame.render_widget(bar_widget, bar_area);
    }
}

/// Draw a mini waveform from overview (min, max) pairs.
///
/// Fills all available rows vertically — amplitude maps to bar height
/// growing upward from the bottom of the area, using block characters
/// for sub-row resolution.
fn draw_mini_waveform(
    frame: &mut Frame,
    overview: &[(f32, f32)],
    area: Rect,
    fg: Color,
    bg: Color,
) {
    if overview.is_empty() || area.width == 0 || area.height == 0 {
        let fill = " ".repeat(area.width as usize);
        for row in 0..area.height {
            let row_area = Rect::new(area.x, area.y + row, area.width, 1);
            frame.render_widget(
                Paragraph::new(fill.clone()).style(Style::new().bg(bg)),
                row_area,
            );
        }
        return;
    }

    let width = area.width as usize;
    let height = f32::from(area.height);
    let overview_per_col = overview.len() as f64 / width as f64;

    // Pre-compute per-column amplitudes to avoid redundant work per row.
    let amplitudes: Vec<f32> = (0..width)
        .map(|col| {
            let idx_start = (col as f64 * overview_per_col) as usize;
            let idx_end = (((col + 1) as f64 * overview_per_col) as usize)
                .min(overview.len())
                .max(idx_start + 1)
                .min(overview.len());

            if idx_start >= overview.len() {
                return 0.0;
            }

            let mut max_amp: f32 = 0.0;
            for &(min_v, max_v) in &overview[idx_start..idx_end] {
                let amp = max_v.abs().max(min_v.abs());
                if amp > max_amp {
                    max_amp = amp;
                }
            }
            max_amp.clamp(0.0, 1.0)
        })
        .collect();

    // Block characters: index 0 = empty, 8 = full block.
    let block_chars = [
        ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}',
        '\u{2587}', '\u{2588}',
    ];

    // Render row by row. Waveform fills from bottom upward.
    for row in 0..area.height {
        let rows_from_bottom = f32::from(area.height - 1 - row);
        let mut line = String::with_capacity(width);

        for &amp in &amplitudes {
            let fill_height = amp * height;

            if rows_from_bottom + 1.0 <= fill_height {
                // Fully within the waveform — full block.
                line.push('\u{2588}');
            } else if rows_from_bottom < fill_height {
                // Partial top edge — use block character for fractional part.
                let frac = fill_height - rows_from_bottom;
                let idx = ((frac * 8.0) as usize).clamp(1, 8);
                line.push(block_chars[idx]);
            } else {
                line.push(' ');
            }
        }

        let row_area = Rect::new(area.x, area.y + row, area.width, 1);
        let widget = Paragraph::new(line).style(Style::new().fg(fg).bg(bg));
        frame.render_widget(widget, row_area);
    }
}

/// Draw an active recording as a growing red rectangle.
fn draw_recording(
    frame: &mut Frame,
    app: &App,
    track: &TrackClipSnapshot,
    area: Rect,
    view_start: f64,
    samples_per_col: f64,
) {
    let rec_start = track.recording_start as f64;
    let rec_end = rec_start + track.recording_length as f64;

    let col_start = ((rec_start - view_start) / samples_per_col).max(0.0) as u16;
    let col_end = ((rec_end - view_start) / samples_per_col).min(f64::from(area.width)) as u16;

    if col_start >= col_end || col_start >= area.width {
        return;
    }

    let rec_width = col_end - col_start;
    let rec_rect = Rect::new(area.x + col_start, area.y, rec_width, area.height);

    let visible = app.recording_blink_visible();
    let rec_style = if visible {
        Style::new()
            .fg(theme::ACCENT_RECORD)
            .bg(Color::Rgb(0x3A, 0x1A, 0x2A))
    } else {
        Style::new()
            .fg(theme::FG_DIMMED)
            .bg(Color::Rgb(0x2A, 0x1A, 0x2A))
    };

    let fill = "\u{2591}".repeat(rec_width as usize);
    for row in 0..rec_rect.height {
        let row_area = Rect::new(rec_rect.x, rec_rect.y + row, rec_rect.width, 1);
        frame.render_widget(Paragraph::new(fill.clone()).style(rec_style), row_area);
    }

    // Label "REC" on the first row if there's space.
    if rec_width >= 3 && rec_rect.height > 0 {
        let label_area = Rect::new(rec_rect.x, rec_rect.y, 3.min(rec_width), 1);
        let label = Paragraph::new("REC").style(rec_style.add_modifier(Modifier::BOLD));
        frame.render_widget(label, label_area);
    }
}

/// Draw the playhead as a vertical line at the current transport position.
fn draw_playhead(frame: &mut Frame, app: &App, area: Rect, view_start: f64, samples_per_col: f64) {
    let position = app.display.transport.position.samples as f64;
    let col = ((position - view_start) / samples_per_col) as i64;

    if col < 0 || col >= i64::from(area.width) {
        return;
    }

    let x = area.x + col as u16;
    let color = match app.display.transport.state {
        TransportState::Playing => theme::ACCENT_PLAY,
        TransportState::Recording => theme::ACCENT_RECORD,
        TransportState::Paused => theme::ACCENT_PAUSE,
        TransportState::Stopped => theme::ACCENT_STOP,
    };

    for row in 0..area.height {
        let cell_area = Rect::new(x, area.y + row, 1, 1);
        let cursor =
            Paragraph::new("\u{2502}").style(Style::new().fg(color).add_modifier(Modifier::BOLD));
        frame.render_widget(cursor, cell_area);
    }
}

/// Draw the time ruler showing MM:SS labels.
fn draw_ruler(
    frame: &mut Frame,
    area: Rect,
    view_start: f64,
    samples_per_col: f64,
    sample_rate: u32,
) {
    if area.width == 0 || area.height == 0 || sample_rate == 0 {
        return;
    }

    let width = area.width as usize;
    let mut ruler_text = vec![' '; width];

    // Choose tick interval: aim for a tick every ~10 columns minimum.
    let seconds_per_col = samples_per_col / f64::from(sample_rate);
    let view_seconds = seconds_per_col * width as f64;

    // Choose a nice interval in seconds.
    let tick_interval_secs = choose_tick_interval(view_seconds, width);
    if tick_interval_secs <= 0.0 {
        return;
    }

    let view_start_secs = view_start / f64::from(sample_rate);
    let first_tick_idx = (view_start_secs / tick_interval_secs).ceil() as i64;
    let end_secs = view_start_secs + view_seconds;
    let max_tick_idx = (end_secs / tick_interval_secs).ceil() as i64;

    for tick_idx in first_tick_idx..max_tick_idx {
        let tick_secs = tick_idx as f64 * tick_interval_secs;
        if tick_secs >= end_secs {
            break;
        }
        let col = ((tick_secs - view_start_secs) / seconds_per_col) as usize;
        if col < width {
            // Write tick mark.
            ruler_text[col] = '\u{2502}';

            // Write time label after tick.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let total_secs = tick_secs.max(0.0) as u64;
            let mins = total_secs / 60;
            let secs = total_secs % 60;
            let label = format!("{mins}:{secs:02}");

            let label_start = col + 1;
            for (i, ch) in label.chars().enumerate() {
                let pos = label_start + i;
                if pos < width {
                    ruler_text[pos] = ch;
                }
            }
        }
    }

    let ruler_string: String = ruler_text.into_iter().collect();
    let ruler = Paragraph::new(ruler_string).style(theme::style_text_dimmed());
    frame.render_widget(ruler, area);
}

/// Choose a nice tick interval (in seconds) for the ruler.
fn choose_tick_interval(view_seconds: f64, width: usize) -> f64 {
    // We want roughly one tick every 10-15 columns.
    let target_ticks = (width / 12).max(1) as f64;
    let raw_interval = view_seconds / target_ticks;

    // Snap to a nice value.
    let nice_intervals = [
        0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0,
    ];
    for &interval in &nice_intervals {
        if interval >= raw_interval {
            return interval;
        }
    }
    // Fallback: round to nearest 10 minutes.
    (raw_interval / 600.0).ceil() * 600.0
}

/// Truncate a string to at most `max_len` characters.
fn truncate_str(s: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }
    match s.char_indices().nth(max_len) {
        Some((byte_idx, _)) => s[..byte_idx].to_owned(),
        None => s.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_tick_interval_small_view() {
        // 1 second visible in 80 cols -> should pick a small interval.
        let interval = choose_tick_interval(1.0, 80);
        assert!(interval > 0.0);
        assert!(interval <= 1.0);
    }

    #[test]
    fn choose_tick_interval_large_view() {
        // 300 seconds (5 min) visible -> should pick 30s or 60s.
        let interval = choose_tick_interval(300.0, 80);
        assert!(interval >= 10.0);
    }

    #[test]
    fn choose_tick_interval_zero_width() {
        let interval = choose_tick_interval(10.0, 1);
        assert!(interval > 0.0);
    }

    #[test]
    fn truncate_str_shorter_than_max() {
        assert_eq!(truncate_str("abc", 10), "abc");
    }

    #[test]
    fn truncate_str_at_max() {
        assert_eq!(truncate_str("abcdef", 3), "abc");
    }

    #[test]
    fn truncate_str_empty() {
        assert_eq!(truncate_str("abc", 0), "");
    }

    #[test]
    fn truncate_str_unicode() {
        // Emoji is 1 char but multiple bytes.
        let result = truncate_str("a\u{1F600}b", 2);
        assert_eq!(result, "a\u{1F600}");
    }
}
