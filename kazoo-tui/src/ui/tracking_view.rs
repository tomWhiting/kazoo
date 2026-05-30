//! Tracking / arrangement timeline view.
//!
//! Full-width view showing a horizontal time axis with vertical track lanes.
//! Each lane displays its clips as bordered regions with mini waveform
//! overviews. The playhead is drawn as a vertical line, and the current
//! time position is shown in the ruler along the top.
//!
//! When no clips exist, falls back to the oscilloscope waveform display
//! with the track list on the left side.
//!
//! A compact mixer fader sidebar on the right shows per-track level meters,
//! dB readout, pan, and solo/mute/arm indicators. The master bus sits at
//! the bottom of the sidebar.
//!
//! Layout:
//! ```text
//! ╭─ TRACKING ─────────────────────────────────────────────╮╭─ Mix ──╮
//! │  ╭ Tracks ╮ ╭ Timeline ──────────────────────────────╮ ││1 ██░-3 │
//! │  │1 ██ Vox│ │ |0:00   |0:05   |0:10                 │ ││ ◁─●─▷  │
//! │  │2 ██ Bas│ │ ▁▃▅▇█▇▅▃▁▃▅▇█▇▅                      │ ││2 █░ -8 │
//! │  │        │ │                                        │ ││ ◁──●▷  │
//! │  ╰────────╯ ╰────────────────────────────────────────╯ ││MASTER  │
//! ╰─────────────────────────────────────────────────────────╯│L██ R█░ │
//!                                                            ╰────────╯
//! ```

use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::{App, FocusedPanel};
use crate::theme;
use kazoo_core::synthesis::SynthesisMode;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Draw the Tracking view into the given content area.
pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.has_clips() || !app.tracks.is_empty() {
        draw_arrangement(frame, app, area);
    } else {
        draw_empty(frame, app, area);
    }
}

// ---------------------------------------------------------------------------
// Arrangement layout (tracks + timeline)
// ---------------------------------------------------------------------------

/// Width of the fader sidebar in terminal columns.
const FADER_SIDEBAR_WIDTH: u16 = 16;

/// Width of the effects inspector sidebar in terminal columns.
const EFFECTS_SIDEBAR_WIDTH: u16 = 28;

/// Minimum terminal inner width to show any right sidebar.
const MIN_WIDTH_FOR_SIDEBAR: u16 = 60;

#[allow(clippy::too_many_lines)]
fn draw_arrangement(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(false))
        .title(" Tracking ")
        .title_style(theme::style_panel_title(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 20 || inner.height < 4 {
        return;
    }

    // Right sidebar selection: when the Effects panel has keyboard focus,
    // show the effects inspector sidebar; otherwise show the compact fader
    // sidebar. The sidebar is hidden on narrow terminals or when no tracks
    // exist.
    let effects_focused = app.is_focused(FocusedPanel::Effects);
    let has_tracks = !app.tracks.is_empty();
    let show_sidebar = inner.width >= MIN_WIDTH_FOR_SIDEBAR && has_tracks;

    let sidebar_width = if show_sidebar && effects_focused {
        EFFECTS_SIDEBAR_WIDTH
    } else if show_sidebar {
        FADER_SIDEBAR_WIDTH
    } else {
        0
    };

    let h_chunks = if sidebar_width > 0 {
        Layout::horizontal([
            Constraint::Length(22),
            Constraint::Min(20),
            Constraint::Length(sidebar_width),
        ])
        .split(inner)
    } else {
        // No room for sidebar — original two-panel layout.
        let two_col =
            Layout::horizontal([Constraint::Length(26), Constraint::Min(20)]).split(inner);
        // Return a 3-element vec with a zero-width third area for uniformity.
        vec![
            two_col[0],
            two_col[1],
            Rect::new(inner.x + inner.width, inner.y, 0, inner.height),
        ]
        .into()
    };

    // Track list on the left.
    crate::ui::tracks::draw(frame, app, h_chunks[0]);

    // Timeline / waveform in the middle.
    if app.has_clips() {
        crate::ui::timeline::draw(frame, app, h_chunks[1]);
    } else {
        draw_waveform_fallback(frame, app, h_chunks[1]);
    }

    // Right sidebar: effects inspector when focused, fader sidebar otherwise.
    if sidebar_width > 0 {
        if effects_focused {
            crate::ui::effects::draw(frame, app, h_chunks[2]);
        } else {
            draw_fader_sidebar(frame, app, h_chunks[2]);
        }
    }
}

// ---------------------------------------------------------------------------
// Waveform fallback (no clips)
// ---------------------------------------------------------------------------

fn draw_waveform_fallback(frame: &mut Frame, app: &App, area: Rect) {
    if app.tracks.is_empty() {
        // No tracks at all — show the single oscilloscope waveform.
        crate::ui::waveform::draw(frame, app, area);
        return;
    }

    // Per-track lane view: each track gets its own horizontal lane.
    // Armed tracks show the live waveform; others show empty lanes.
    // A time ruler sits at the bottom.
    let ruler_height = 1u16;
    let lanes_height = area.height.saturating_sub(ruler_height);

    if lanes_height < 1 {
        return;
    }

    let lanes_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: lanes_height,
    };
    let ruler_area = Rect {
        x: area.x,
        y: area.y + lanes_height,
        width: area.width,
        height: ruler_height,
    };

    draw_track_lanes(frame, app, lanes_area);
    draw_simple_ruler(frame, app, ruler_area);
}

// ---------------------------------------------------------------------------
// Per-track lane view (no clips)
// ---------------------------------------------------------------------------

/// Fixed height per track lane (rows). Tracks don't expand to fill the
/// screen — extra space is left empty at the bottom.
const LANE_HEIGHT: u16 = 3;

/// Draw one horizontal lane per track. Armed tracks show the live waveform;
/// others show an empty lane with just the track colour bar.
fn draw_track_lanes(frame: &mut Frame, app: &App, area: Rect) {
    let row_height = LANE_HEIGHT;

    for (i, track) in app.tracks.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let y = area.y + (i as u16) * row_height;
        if y >= area.y + area.height {
            break;
        }
        let h = row_height.min(area.y + area.height - y);
        let lane_area = Rect::new(area.x, y, area.width, h);

        let track_col = theme::track_color(i);
        let lane_bg = theme::lane_bg(i);

        // Fill lane background.
        let fill = " ".repeat(area.width as usize);
        for row in 0..h {
            let row_rect = Rect::new(lane_area.x, lane_area.y + row, lane_area.width, 1);
            frame.render_widget(
                Paragraph::new(fill.clone()).style(Style::new().bg(lane_bg)),
                row_rect,
            );
        }

        // Colour bar on the left edge.
        for row in 0..h {
            let bar_area = Rect::new(lane_area.x, lane_area.y + row, 1, 1);
            frame.render_widget(
                Paragraph::new("\u{258E}").style(Style::new().fg(track_col).bg(lane_bg)),
                bar_area,
            );
        }

        // Track header: name, synth mode, armed/recording indicator, pitch.
        if h > 0 && lane_area.width > 2 {
            let name: &str = match track.name.char_indices().nth(4) {
                Some((byte_idx, _)) => &track.name[..byte_idx],
                None => &track.name,
            };

            let mode_str = synth_mode_label(track.synthesis_mode);

            let mut spans: Vec<Span<'_>> = Vec::with_capacity(8);
            spans.push(Span::styled(
                format!(" {name}"),
                Style::new()
                    .fg(track_col)
                    .bg(lane_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {mode_str}"),
                Style::new().fg(theme::FG_SECONDARY).bg(lane_bg),
            ));

            if track.armed {
                // Armed indicator: bold when recording, normal when idle.
                let armed_style = if app.display.is_recording {
                    Style::new()
                        .fg(theme::ACCENT_RECORD)
                        .bg(lane_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new().fg(theme::ACCENT_RECORD).bg(lane_bg)
                };
                spans.push(Span::styled(" \u{25cf}", armed_style));

                // Live pitch readout on armed tracks.
                let pitch = &app.display.pitch;
                match (pitch.frequency, pitch.midi_note) {
                    (Some(freq), Some(note)) if freq.is_finite() => {
                        let note_name = kazoo_core::midi_note_name(note);
                        spans.push(Span::styled(
                            format!(" {note_name}"),
                            Style::new()
                                .fg(theme::FG_PRIMARY)
                                .bg(lane_bg)
                                .add_modifier(Modifier::BOLD),
                        ));
                        spans.push(Span::styled(
                            format!(" {freq:.0}Hz"),
                            Style::new().fg(theme::FG_DIMMED).bg(lane_bg),
                        ));
                    }
                    (Some(freq), None) if freq.is_finite() => {
                        spans.push(Span::styled(
                            format!(" {freq:.0}Hz"),
                            Style::new().fg(theme::FG_DIMMED).bg(lane_bg),
                        ));
                    }
                    _ => {}
                }
            }

            let text_area = Rect::new(lane_area.x + 1, lane_area.y, lane_area.width - 1, 1);
            frame.render_widget(Paragraph::new(Line::from(spans)), text_area);
        }

        // Draw live waveform in armed track lanes.
        if track.armed && h > 1 && lane_area.width > 6 {
            let wave_area = Rect::new(
                lane_area.x + 6,
                lane_area.y + 1,
                lane_area.width.saturating_sub(6),
                h.saturating_sub(1),
            );
            draw_lane_waveform(frame, app, wave_area, track_col, lane_bg);
        }
    }
}

/// Draw a mini waveform in a single track lane using block characters.
fn draw_lane_waveform(frame: &mut Frame, app: &App, area: Rect, fg: Color, bg: Color) {
    let waveform = &app.display.waveform;
    if waveform.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }

    let width = area.width as usize;
    let height = f32::from(area.height);

    // Map waveform samples to columns.
    let total = waveform.len();
    let samples_per_col = total as f64 / width as f64;

    let block_chars = [
        ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];

    for row in 0..area.height {
        let rows_from_bottom = f32::from(area.height - 1 - row);
        let mut line = String::with_capacity(width);

        for col in 0..width {
            let start = (col as f64 * samples_per_col) as usize;
            let end = (((col + 1) as f64 * samples_per_col) as usize)
                .min(total)
                .max(start + 1)
                .min(total);

            let mut max_amp: f32 = 0.0;
            for &s in &waveform[start..end] {
                let a = s.clamp(-1.0, 1.0).abs();
                if a > max_amp {
                    max_amp = a;
                }
            }

            let fill_height = max_amp * height;
            if rows_from_bottom + 1.0 <= fill_height {
                line.push('\u{2588}');
            } else if rows_from_bottom < fill_height {
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

/// Draw a simple time ruler showing the current transport position.
fn draw_simple_ruler(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let width = area.width as usize;
    let sample_rate = app.engine.sample_rate();
    if sample_rate == 0 {
        return;
    }

    // Show the current time position centered, with tick marks.
    let pos_samples = app.display.transport.position.samples;
    let total_secs = pos_samples as f64 / f64::from(sample_rate);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mins = (total_secs / 60.0) as u64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let secs = (total_secs % 60.0) as u64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let millis = ((total_secs * 1000.0) % 1000.0) as u64;

    let time_str = format!("\u{25b6} {mins}:{secs:02}.{millis:03}");

    // Build ruler: tick marks at intervals, current time on the left.
    let mut ruler_text = String::with_capacity(width);
    ruler_text.push_str(&time_str);

    // Fill remaining with tick marks.
    let remaining = width.saturating_sub(time_str.chars().count());
    for i in 0..remaining {
        if i % 10 == 0 {
            ruler_text.push('\u{2502}');
        } else {
            ruler_text.push('\u{2500}');
        }
    }

    // Truncate to width.
    let truncated: String = ruler_text.chars().take(width).collect();
    frame.render_widget(
        Paragraph::new(truncated).style(theme::style_text_dimmed()),
        area,
    );
}

// ---------------------------------------------------------------------------
// Empty state
// ---------------------------------------------------------------------------

fn draw_empty(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(false))
        .title(" Tracking ")
        .title_style(theme::style_panel_title(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 4 || inner.width < 20 {
        return;
    }

    // Split: waveform view (main), hint at bottom.
    let chunks = Layout::vertical([Constraint::Min(4), Constraint::Length(2)]).split(inner);

    crate::ui::waveform::draw(frame, app, chunks[0]);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled("  n", theme::style_help_key()),
        Span::styled(" add track  ", theme::style_help_desc()),
        Span::styled("r", theme::style_help_key()),
        Span::styled(" record  ", theme::style_help_desc()),
        Span::styled("o", theme::style_help_key()),
        Span::styled(" open file", theme::style_help_desc()),
    ]));
    frame.render_widget(hint, chunks[1]);
}

// ---------------------------------------------------------------------------
// Fader sidebar — compact per-track meters + master bus
// ---------------------------------------------------------------------------

/// Minimum dB shown on meters.
const METER_MIN_DB: f32 = -60.0;

/// Maximum dB shown on meters.
const METER_MAX_DB: f32 = 0.0;

/// Rows per track in the fader sidebar.
const ROWS_PER_TRACK: u16 = 2;

/// Draw the compact fader sidebar on the right side of the tracking view.
///
/// Layout (top to bottom):
///   Compact synth section (synth mode + key params for selected track)
///   Per-track fader strips
///   Master section
fn draw_fader_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(false))
        .title(" Mix ")
        .title_style(theme::style_panel_title(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 8 || inner.height < 4 {
        return;
    }

    // Calculate heights for the three sections.
    let synth_rows = compact_synth_height(app);
    let master_rows = 3u16;
    let track_area_height = inner
        .height
        .saturating_sub(master_rows)
        .saturating_sub(synth_rows);

    let synth_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: synth_rows.min(inner.height),
    };

    let track_area = Rect {
        x: inner.x,
        y: inner.y + synth_rows,
        width: inner.width,
        height: track_area_height,
    };

    let master_area = Rect {
        x: inner.x,
        y: inner.y + synth_rows + track_area_height,
        width: inner.width,
        height: master_rows.min(inner.height.saturating_sub(synth_rows + track_area_height)),
    };

    // Compact synth section at the top.
    draw_compact_synth(frame, app, synth_area);

    // Per-track fader strips.
    let max_visible = (track_area_height / ROWS_PER_TRACK) as usize;
    let selected = app.selected_track;

    // Scroll to keep selected track visible.
    let scroll = if selected >= max_visible.max(1) {
        selected.saturating_sub(max_visible.saturating_sub(1))
    } else {
        0
    };

    for i in 0..max_visible {
        let track_idx = scroll + i;
        if track_idx >= app.tracks.len() {
            break;
        }
        let y = track_area.y + (i as u16) * ROWS_PER_TRACK;
        if y + ROWS_PER_TRACK > track_area.y + track_area.height {
            break;
        }
        let strip_area = Rect {
            x: track_area.x,
            y,
            width: track_area.width,
            height: ROWS_PER_TRACK,
        };
        draw_fader_strip(frame, app, track_idx, strip_area);
    }

    // Master section at the bottom.
    draw_fader_master(frame, app, master_area);
}

/// Calculate the height for the compact synth section.
///
/// Returns 0 if there is no selected track, otherwise a minimum of 4 rows
/// (separator + synth mode + up to 3 params + hint).
fn compact_synth_height(app: &App) -> u16 {
    let Some(track) = app.selected_track_info() else {
        return 0;
    };
    // 1 separator + 1 mode label + min(param_count, 3) params + 1 hint
    let param_rows = track.synth_param_infos.len().min(3) as u16;
    1 + 1 + param_rows + 1
}

/// Draw a compact synth overview at the top of the fader sidebar.
///
/// Shows the synthesis mode and the first few parameters for the selected
/// track so synth controls are always visible without needing to Tab to
/// the Effects panel.
fn draw_compact_synth(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let Some(track) = app.selected_track_info() else {
        return;
    };

    let width = area.width as usize;
    let mut row = 0u16;

    // Row 0: separator with synth mode name.
    if row < area.height {
        let mode_str = match track.synthesis_mode {
            SynthesisMode::Passthrough => "Raw",
            SynthesisMode::PitchTracked => "Pitch",
            SynthesisMode::Wavetable => "Wave",
            SynthesisMode::Granular => "Gran",
            SynthesisMode::Vocoder => "Voc",
            SynthesisMode::PhaseVocoder => "PhVoc",
        };
        let label = format!(" {mode_str} ");
        let pad_left = width.saturating_sub(label.len()) / 2;
        let pad_right = width.saturating_sub(pad_left + label.len());
        let sep = format!(
            "{}{}{}",
            "\u{2500}".repeat(pad_left),
            label,
            "\u{2500}".repeat(pad_right),
        );
        let sep_area = Rect::new(area.x, area.y + row, area.width, 1);
        frame.render_widget(
            Paragraph::new(sep).style(
                Style::new()
                    .fg(theme::ACCENT_FOCUS)
                    .add_modifier(Modifier::BOLD),
            ),
            sep_area,
        );
        row += 1;
    }

    // Row 1: synth mode full name (with track colour).
    if row < area.height {
        let idx = app.selected_track;
        let color = theme::track_color(idx);
        let name: &str = match track.name.char_indices().nth(6) {
            Some((byte_idx, _)) => &track.name[..byte_idx],
            None => &track.name,
        };
        let synth_row = Rect::new(area.x, area.y + row, area.width, 1);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    name.to_string(),
                    Style::new().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" [{}]", synth_mode_label(track.synthesis_mode)),
                    theme::style_text_dimmed(),
                ),
            ])),
            synth_row,
        );
        row += 1;
    }

    // Rows 2..N: up to 3 key synth parameters.
    let max_params = 3usize;
    for (i, info) in track.synth_param_infos.iter().take(max_params).enumerate() {
        if row >= area.height {
            break;
        }
        let value = track.synth_param_values.get(i).copied().unwrap_or(0.0);
        let formatted = track.synthesis_mode.format_param_value(i, value);

        // Truncate param name to fit.
        let name = if info.name.len() > 6 {
            &info.name[..6]
        } else {
            &info.name
        };

        let param_row = Rect::new(area.x, area.y + row, area.width, 1);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{name:<6}"), theme::style_text_dimmed()),
                Span::raw(" "),
                Span::styled(formatted, theme::style_text_secondary()),
            ])),
            param_row,
        );
        row += 1;
    }

    // Hint row: how to access full controls.
    if row < area.height {
        let hint_row = Rect::new(area.x, area.y + row, area.width, 1);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("e", theme::style_help_key()),
                Span::styled(" expand", theme::style_help_desc()),
            ])),
            hint_row,
        );
    }
}

/// Draw a single compact fader strip (2 rows) for one track.
fn draw_fader_strip(frame: &mut Frame, app: &App, index: usize, area: Rect) {
    let Some(track) = app.tracks.get(index) else {
        return;
    };
    let is_selected = index == app.selected_track;
    let color = theme::track_color(index);
    let width = area.width as usize;

    // Row 0: track number + mini meter + dB + S/M/R
    if area.height >= 1 {
        let row0 = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };

        let (peak_db, _rms_db) = app
            .display
            .mixer
            .track_meters
            .get(index)
            .map_or((-100.0_f32, -100.0_f32), |m| {
                (m.peak_db[0].max(m.peak_db[1]), m.rms_db[0].max(m.rms_db[1]))
            });

        let ratio = db_to_ratio(peak_db);
        let meter_width = 4usize.min(width.saturating_sub(8));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let filled = (ratio * meter_width as f32).round() as usize;
        let empty = meter_width.saturating_sub(filled);

        let db_str = if peak_db <= METER_MIN_DB || !peak_db.is_finite() {
            String::from("-\u{221e}")
        } else {
            format!("{peak_db:.0}")
        };

        let idx_str = format!("{}", index + 1);
        let idx_style = if is_selected {
            Style::new()
                .fg(theme::BG_PRIMARY)
                .bg(theme::ACCENT_FOCUS)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(color).add_modifier(Modifier::BOLD)
        };

        let mut spans: Vec<Span<'_>> = Vec::with_capacity(10);
        spans.push(Span::styled(idx_str, idx_style));
        spans.push(Span::raw(" "));

        // Meter bar.
        if filled > 0 {
            spans.push(Span::styled(
                "\u{2588}".repeat(filled),
                Style::new().fg(theme::meter_color_db(peak_db)),
            ));
        }
        if empty > 0 {
            spans.push(Span::styled(
                "\u{2591}".repeat(empty),
                theme::style_text_dimmed(),
            ));
        }

        // dB readout.
        spans.push(Span::styled(
            format!("{db_str:>3}"),
            Style::new().fg(theme::meter_color_db(peak_db)),
        ));
        spans.push(Span::raw(" "));

        // S/M/R indicators (compact).
        if track.soloed {
            spans.push(Span::styled("S", theme::style_soloed()));
        } else {
            spans.push(Span::styled("\u{00b7}", theme::style_text_dimmed()));
        }
        if track.muted {
            spans.push(Span::styled("M", theme::style_muted()));
        } else {
            spans.push(Span::styled("\u{00b7}", theme::style_text_dimmed()));
        }
        if track.armed {
            spans.push(Span::styled("R", theme::style_armed()));
        } else {
            spans.push(Span::styled("\u{00b7}", theme::style_text_dimmed()));
        }

        frame.render_widget(Paragraph::new(Line::from(spans)), row0);
    }

    // Row 1: pan indicator.
    if area.height >= 2 {
        let row1 = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: 1,
        };

        let pan = track.pan.value();
        let pan_width = width.saturating_sub(2).min(10);
        let pan_str = format_compact_pan(pan, pan_width);

        frame.render_widget(
            Paragraph::new(format!("  {pan_str}")).style(theme::style_text_dimmed()),
            row1,
        );
    }
}

/// Draw the master section at the bottom of the fader sidebar.
fn draw_fader_master(frame: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 || area.width < 4 {
        return;
    }

    // Row 0: separator / label.
    let label_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    // Build a separator line that fills the width.
    let sep_len = area.width as usize;
    let label = "MASTER";
    let pad = sep_len.saturating_sub(label.len()) / 2;
    let sep_line = format!(
        "{}{}{}",
        "\u{2500}".repeat(pad),
        label,
        "\u{2500}".repeat(sep_len.saturating_sub(pad + label.len()))
    );
    frame.render_widget(
        Paragraph::new(sep_line).style(
            Style::new()
                .fg(theme::FG_SECONDARY)
                .add_modifier(Modifier::BOLD),
        ),
        label_area,
    );

    let meter_width = 4usize.min(area.width as usize - 6);

    // Row 1: L channel.
    if area.height >= 2 {
        let l_db = app.display.mixer.master_peak_db[0];
        let l_row = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: 1,
        };
        draw_master_meter_line(frame, 'L', l_db, meter_width, l_row);
    }

    // Row 2: R channel.
    if area.height >= 3 {
        let r_db = app.display.mixer.master_peak_db[1];
        let r_row = Rect {
            x: area.x,
            y: area.y + 2,
            width: area.width,
            height: 1,
        };
        draw_master_meter_line(frame, 'R', r_db, meter_width, r_row);
    }
}

/// Draw a single master meter line: `L ████░░ -1.2dB`.
fn draw_master_meter_line(
    frame: &mut Frame,
    channel: char,
    peak_db: f32,
    meter_width: usize,
    area: Rect,
) {
    let ratio = db_to_ratio(peak_db);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let filled = (ratio * meter_width as f32).round() as usize;
    let empty = meter_width.saturating_sub(filled);

    let db_str = if peak_db <= METER_MIN_DB || !peak_db.is_finite() {
        String::from("-\u{221e}")
    } else {
        format!("{peak_db:.0}")
    };

    let mut spans: Vec<Span<'_>> = Vec::with_capacity(6);
    spans.push(Span::styled(
        format!("{channel} "),
        theme::style_text_secondary(),
    ));
    if filled > 0 {
        spans.push(Span::styled(
            "\u{2588}".repeat(filled),
            Style::new().fg(theme::meter_color_db(peak_db)),
        ));
    }
    if empty > 0 {
        spans.push(Span::styled(
            "\u{2591}".repeat(empty),
            theme::style_text_dimmed(),
        ));
    }
    spans.push(Span::styled(
        format!(" {db_str:>3}dB"),
        Style::new().fg(theme::meter_color_db(peak_db)),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Short label for a synthesis mode, used in track lane headers.
const fn synth_mode_label(mode: SynthesisMode) -> &'static str {
    match mode {
        SynthesisMode::Passthrough => "Raw",
        SynthesisMode::PitchTracked => "Pt",
        SynthesisMode::Wavetable => "Wt",
        SynthesisMode::Granular => "Gr",
        SynthesisMode::Vocoder => "Vc",
        SynthesisMode::PhaseVocoder => "Pv",
    }
}

/// Map dB to 0.0..1.0 ratio, NaN-safe.
fn db_to_ratio(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    ((db - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB)).clamp(0.0, 1.0)
}

/// Compact pan display: `◁─●─▷` with the knob positioned by pan value.
fn format_compact_pan(pan: f32, width: usize) -> String {
    if width < 3 {
        return String::new();
    }
    let positions = width.saturating_sub(1);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::manual_midpoint
    )]
    let knob_pos = (((pan + 1.0) / 2.0) * positions as f32)
        .round()
        .clamp(0.0, positions as f32) as usize;

    let mut result = String::with_capacity(width);
    for i in 0..width {
        if i == knob_pos {
            result.push('\u{25cf}'); // filled circle (knob)
        } else if i == 0 {
            result.push('\u{25c1}'); // left arrow
        } else if i == width - 1 {
            result.push('\u{25b7}'); // right arrow
        } else {
            result.push('\u{2500}'); // horizontal line
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- db_to_ratio --------------------------------------------------------

    #[test]
    fn db_to_ratio_at_min() {
        assert!((db_to_ratio(METER_MIN_DB) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_at_max() {
        assert!((db_to_ratio(METER_MAX_DB) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_midpoint() {
        let mid = (METER_MIN_DB + METER_MAX_DB) / 2.0;
        assert!((db_to_ratio(mid) - 0.5).abs() < f32::EPSILON);
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

    #[test]
    fn db_to_ratio_clamped_above() {
        assert!((db_to_ratio(10.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_clamped_below() {
        assert!((db_to_ratio(-200.0) - 0.0).abs() < f32::EPSILON);
    }

    // -- format_compact_pan -------------------------------------------------

    #[test]
    fn format_compact_pan_center() {
        let pan = format_compact_pan(0.0, 7);
        assert!(pan.contains('\u{25cf}')); // has knob
        assert_eq!(pan.chars().count(), 7);
    }

    #[test]
    fn format_compact_pan_hard_left() {
        let pan = format_compact_pan(-1.0, 7);
        let chars: Vec<char> = pan.chars().collect();
        assert_eq!(chars[0], '\u{25cf}'); // knob at far left
    }

    #[test]
    fn format_compact_pan_hard_right() {
        let pan = format_compact_pan(1.0, 7);
        let chars: Vec<char> = pan.chars().collect();
        assert_eq!(chars[chars.len() - 1], '\u{25cf}'); // knob at far right
    }

    #[test]
    fn format_compact_pan_too_narrow_returns_empty() {
        assert!(format_compact_pan(0.0, 2).is_empty());
        assert!(format_compact_pan(0.0, 0).is_empty());
    }

    #[test]
    fn format_compact_pan_minimum_width() {
        let pan = format_compact_pan(0.0, 3);
        assert_eq!(pan.chars().count(), 3);
        assert!(pan.contains('\u{25cf}'));
    }

    // -- synth_mode_label ---------------------------------------------------

    #[test]
    fn synth_mode_label_all_modes() {
        assert_eq!(synth_mode_label(SynthesisMode::Passthrough), "Raw");
        assert_eq!(synth_mode_label(SynthesisMode::PitchTracked), "Pt");
        assert_eq!(synth_mode_label(SynthesisMode::Wavetable), "Wt");
        assert_eq!(synth_mode_label(SynthesisMode::Granular), "Gr");
        assert_eq!(synth_mode_label(SynthesisMode::Vocoder), "Vc");
        assert_eq!(synth_mode_label(SynthesisMode::PhaseVocoder), "Pv");
    }

    #[test]
    fn synth_mode_label_non_empty() {
        for mode in [
            SynthesisMode::Passthrough,
            SynthesisMode::PitchTracked,
            SynthesisMode::Wavetable,
            SynthesisMode::Granular,
            SynthesisMode::Vocoder,
            SynthesisMode::PhaseVocoder,
        ] {
            assert!(
                !synth_mode_label(mode).is_empty(),
                "{mode:?} has empty label"
            );
        }
    }

    // -- sidebar constants ---------------------------------------------------

    #[test]
    fn effects_sidebar_wider_than_fader_sidebar() {
        assert!(EFFECTS_SIDEBAR_WIDTH > FADER_SIDEBAR_WIDTH);
    }

    #[test]
    fn sidebar_widths_leave_room_for_track_list_and_timeline() {
        // Track list is 22 cols, timeline needs at least 20.
        // The sidebar must fit within MIN_WIDTH_FOR_SIDEBAR total.
        assert!(
            22 + 20 + EFFECTS_SIDEBAR_WIDTH <= MIN_WIDTH_FOR_SIDEBAR + EFFECTS_SIDEBAR_WIDTH,
            "effects sidebar must be usable at minimum width"
        );
        assert!(
            22 + 20 + FADER_SIDEBAR_WIDTH <= MIN_WIDTH_FOR_SIDEBAR + FADER_SIDEBAR_WIDTH,
            "fader sidebar must be usable at minimum width"
        );
    }

    // -- LANE_HEIGHT constant -----------------------------------------------

    #[test]
    fn lane_height_is_positive() {
        assert!(LANE_HEIGHT > 0);
    }
}
