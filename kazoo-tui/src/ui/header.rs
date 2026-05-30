//! Persistent header strip: branding, transport, meters, and view tabs.
//!
//! Renders a 4-row bordered strip at the top of every view. The three content
//! rows pack a dense overview of the engine state:
//!
//! - **Row 1:** "KAZOO -- mouth noises" branding + recording indicator.
//! - **Row 2:** Transport state, time, bar.beat.tick, BPM, loop, beat dots, view tabs.
//! - **Row 3:** Detected pitch, input level, L/R master VU meters, CPU load.

use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::App;
use crate::state::ActiveView;
use crate::theme;
use kazoo_core::transport::TransportState;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Minimum dB value displayed on the horizontal meter.
const METER_MIN_DB: f32 = -60.0;

/// Maximum dB value displayed on the horizontal meter.
const METER_MAX_DB: f32 = 0.0;

/// Number of block characters in a single horizontal meter bar.
const METER_BAR_WIDTH: usize = 8;

// ---------------------------------------------------------------------------
// Public draw entry point
// ---------------------------------------------------------------------------

/// Draw the persistent header into the given area (expected to be 4 rows high).
///
/// The header is wrapped in a rounded border and contains three content lines:
/// branding/recording, transport/tabs, and pitch/meters/CPU.
#[allow(clippy::too_many_lines)]
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    // Outer border with rounded corners.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(false))
        .style(Style::new().bg(theme::BG_PRIMARY));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Guard: need at least 3 rows for the content lines.
    if inner.height < 3 || inner.width < 20 {
        return;
    }

    // Split the inner area into three single-row regions.
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);

    draw_row_branding(frame, app, rows[0]);
    draw_row_transport(frame, app, rows[1]);
    draw_row_meters(frame, app, rows[2]);
}

// ---------------------------------------------------------------------------
// Row 1: Branding + recording indicator
// ---------------------------------------------------------------------------

/// Render the branding line with "KAZOO -- mouth noises" on the left and
/// a blinking recording indicator on the right when recording is active.
fn draw_row_branding(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;

    // Left side: branding.
    let brand_label = "KAZOO";
    let brand_tagline = " -- mouth noises";

    let mut left_spans: Vec<Span<'_>> = vec![
        Span::styled(
            brand_label,
            Style::new()
                .fg(theme::BORDER_FOCUS)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(brand_tagline, theme::style_text_dimmed()),
    ];

    // IPC instrument connection badges.
    if !app.display.ipc_instruments.is_empty() {
        left_spans.push(Span::styled("  ", theme::style_text_dimmed()));
        for (i, inst) in app.display.ipc_instruments.iter().enumerate() {
            if i > 0 {
                left_spans.push(Span::styled(" ", theme::style_text_dimmed()));
            }
            if inst.connected {
                // Connected: green dot + name.
                left_spans.push(Span::styled(
                    "\u{25cf}",
                    Style::new().fg(theme::ACCENT_PLAY),
                ));
                left_spans.push(Span::styled(
                    &*inst.name,
                    theme::style_text_secondary().add_modifier(Modifier::BOLD),
                ));
            } else {
                // Disconnected: dim dot + name.
                left_spans.push(Span::styled("\u{25cb}", theme::style_text_dimmed()));
                left_spans.push(Span::styled(&*inst.name, theme::style_text_dimmed()));
            }
        }
    }

    // Right side: recording indicator (blinking).
    let rec_indicator = if app.display.is_recording {
        let visible = app.recording_blink_visible();
        if visible {
            "\u{25c9}REC"
        } else {
            // Keep the same width so the layout doesn't jump.
            "    "
        }
    } else {
        ""
    };

    let rec_style = if app.display.is_recording {
        theme::style_recording(app.recording_blink_visible())
    } else {
        theme::style_text_dimmed()
    };

    // Calculate padding to right-align the recording indicator.
    // Use Span::width (display width) not .len() (byte length) because
    // the recording indicator contains multi-byte Unicode (◉ = 3 bytes UTF-8
    // but 1 column display width).
    let left_len: usize = left_spans.iter().map(Span::width).sum();
    let right_span = Span::styled(rec_indicator, rec_style);
    let right_len = right_span.width();
    let padding = width.saturating_sub(left_len + right_len);

    if padding > 0 {
        left_spans.push(Span::raw(" ".repeat(padding)));
    }
    if !rec_indicator.is_empty() {
        left_spans.push(right_span);
    }

    let line = Line::from(left_spans);
    frame.render_widget(Paragraph::new(line), area);
}

// ---------------------------------------------------------------------------
// Row 2: Transport state + view tabs
// ---------------------------------------------------------------------------

/// Render the transport status line with state icon, time, bar/beat, BPM,
/// loop indicator, beat dots, and right-aligned view tabs.
#[allow(clippy::too_many_lines)]
fn draw_row_transport(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;
    let transport = &app.display.transport;

    // Transport state indicator.
    let (state_icon, state_label, state_style) = if transport.count_in_active {
        let label = format!(
            "COUNT {}/{}",
            transport.count_in_bar, transport.count_in_total
        );
        (
            "",
            label,
            theme::style_recording(app.recording_blink_visible()),
        )
    } else {
        match transport.state {
            TransportState::Playing => ("\u{25b6}", " PLAY".to_owned(), theme::style_playing()),
            TransportState::Stopped => ("\u{25a0}", " STOP".to_owned(), theme::style_stopped()),
            TransportState::Paused => (
                "\u{2759}\u{2759}",
                " PAUSE".to_owned(),
                theme::style_paused(),
            ),
            TransportState::Recording => {
                let visible = app.recording_blink_visible();
                (
                    "\u{25cf}",
                    " REC".to_owned(),
                    theme::style_recording(visible),
                )
            }
        }
    };

    // Time position MM:SS.mmm.
    let time_str = transport.position.format_time();

    // Bar.Beat.Tick.
    let bar_beat = transport
        .position
        .format_bar_beat_tick(transport.bpm, transport.beats_per_bar);

    // BPM display.
    let bpm_str = format!("\u{2669}{:.0}", transport.bpm);

    // Loop indicator.
    let loop_str = if transport.loop_enabled {
        "\u{27f3}LOOP"
    } else {
        ""
    };

    // Beat dots: filled for beats that have passed, open for future beats.
    let beats_per_bar = transport.beats_per_bar.max(1);
    let current_beat = transport.current_beat;

    let sep = theme::style_text_dimmed();

    let mut left_spans: Vec<Span<'_>> = Vec::with_capacity(32);

    // State icon + label.
    left_spans.push(Span::styled(state_icon, state_style));
    left_spans.push(Span::styled(state_label, state_style));
    left_spans.push(Span::styled("  ", sep));

    // Time.
    left_spans.push(Span::styled(time_str, theme::style_text()));
    left_spans.push(Span::styled("  ", sep));

    // Bar.Beat.Tick.
    left_spans.push(Span::styled(
        format!("Bar {bar_beat}"),
        theme::style_text_secondary(),
    ));
    left_spans.push(Span::styled("   ", sep));

    // BPM.
    left_spans.push(Span::styled(bpm_str, theme::style_text()));
    left_spans.push(Span::styled(" ", sep));

    // Metronome indicator.
    if transport.metronome_enabled {
        left_spans.push(Span::styled(
            "M",
            Style::new()
                .fg(theme::ACCENT_PLAY)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        left_spans.push(Span::styled("M", theme::style_text_dimmed()));
    }
    left_spans.push(Span::styled("   ", sep));

    // Loop indicator.
    if transport.loop_enabled {
        left_spans.push(Span::styled(
            loop_str,
            Style::new()
                .fg(theme::ACCENT_PAUSE)
                .add_modifier(Modifier::BOLD),
        ));
        left_spans.push(Span::styled("   ", sep));
    }

    // Beat dots.
    for beat in 0..beats_per_bar {
        let is_past_or_current = beat <= current_beat && transport.beat_active;
        let is_current = beat == current_beat && transport.beat_active;
        if is_current {
            // Current beat: filled circle, accent color.
            left_spans.push(Span::styled(
                "\u{25cf}",
                Style::new().fg(theme::ACCENT_RECORD),
            ));
        } else if is_past_or_current {
            // Past beat: filled circle, dimmer.
            left_spans.push(Span::styled(
                "\u{25cf}",
                Style::new().fg(theme::ACCENT_PLAY),
            ));
        } else {
            // Future beat: open circle.
            left_spans.push(Span::styled("\u{25cb}", theme::style_text_dimmed()));
        }
    }

    // Calculate the width of left content to determine padding for tabs.
    let left_content_width: usize = left_spans.iter().map(Span::width).sum();

    // Build view tabs.
    let tab_spans = build_view_tabs(app);
    let tab_width: usize = tab_spans.iter().map(Span::width).sum();

    // Padding between left content and right-aligned tabs.
    let padding = width.saturating_sub(left_content_width + tab_width);
    if padding > 0 {
        left_spans.push(Span::raw(" ".repeat(padding)));
    }
    left_spans.extend(tab_spans);

    let line = Line::from(left_spans);
    frame.render_widget(Paragraph::new(line), area);
}

/// Build the view tab spans: `[1:Synth] [2:Mixer] ...` with the active tab
/// highlighted.
fn build_view_tabs(app: &App) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(ActiveView::ALL.len() * 2);

    for (i, view) in ActiveView::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }

        let key = view.key_number();
        let label = view.label();
        let tab_text = format!("[{key}:{label}]");

        let style = if app.active_view == *view {
            theme::style_view_tab_active()
        } else {
            theme::style_view_tab_inactive()
        };

        spans.push(Span::styled(tab_text, style));
    }

    spans
}

// ---------------------------------------------------------------------------
// Row 3: Pitch, input level, L/R meters, CPU load
// ---------------------------------------------------------------------------

/// Render the analysis/metering line with detected pitch, input level,
/// horizontal L/R master VU meters, and CPU load.
fn draw_row_meters(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;

    // Detected pitch: note name + Hz.
    let pitch_str = build_pitch_string(app);

    // Input level dB.
    let input_db = app.display.input_level_db;
    let input_str = format!("Lvl: {input_db:.1}dB");
    let input_color = theme::meter_color_db(input_db);

    // L/R master meter data.
    let l_peak_db = app.display.mixer.master_peak_db[0];
    let r_peak_db = app.display.mixer.master_peak_db[1];

    // CPU load percentage.
    let cpu_pct = app.display.cpu_load * 100.0;
    let cpu_str = format!("CPU: {cpu_pct:.1}%");

    // Clipping indicator.
    let clip_str = if app.display.mixer.master_clipping {
        "CLIP"
    } else {
        ""
    };

    let sep = theme::style_text_dimmed();

    let mut spans: Vec<Span<'_>> = Vec::with_capacity(24);

    // Pitch.
    spans.push(Span::styled("Pitch: ", theme::style_text_dimmed()));
    spans.push(Span::styled(pitch_str, theme::style_text()));
    spans.push(Span::styled("  ", sep));

    // Input level.
    spans.push(Span::styled(input_str, Style::new().fg(input_color)));
    spans.push(Span::styled("  ", sep));

    // L meter.
    spans.push(Span::styled("L ", theme::style_text_secondary()));
    build_horizontal_meter(&mut spans, l_peak_db);
    spans.push(Span::styled(
        format!(" {l_peak_db:>5.1}dB"),
        Style::new().fg(theme::meter_color_db(l_peak_db)),
    ));
    spans.push(Span::styled("  ", sep));

    // R meter.
    spans.push(Span::styled("R ", theme::style_text_secondary()));
    build_horizontal_meter(&mut spans, r_peak_db);
    spans.push(Span::styled(
        format!(" {r_peak_db:>5.1}dB"),
        Style::new().fg(theme::meter_color_db(r_peak_db)),
    ));
    spans.push(Span::styled("  ", sep));

    // Clipping indicator (if active).
    if !clip_str.is_empty() {
        spans.push(Span::styled(
            clip_str,
            Style::new()
                .fg(theme::METER_RED)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("  ", sep));
    }

    // CPU load (right-aligned).
    let left_content_width: usize = spans.iter().map(Span::width).sum();
    let cpu_width = cpu_str.len();
    let padding = width.saturating_sub(left_content_width + cpu_width);
    if padding > 0 {
        spans.push(Span::raw(" ".repeat(padding)));
    }
    spans.push(Span::styled(cpu_str, theme::style_text_dimmed()));

    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the pitch display string: "A4 440.0Hz" or "--" when unvoiced.
fn build_pitch_string(app: &App) -> String {
    match (app.display.pitch.frequency, app.display.pitch.midi_note) {
        (Some(freq), Some(note)) => {
            let name = kazoo_core::midi_note_name(note);
            format!("{name} {freq:.1}Hz")
        }
        (Some(freq), None) => {
            format!("{freq:.1}Hz")
        }
        _ => "--".to_owned(),
    }
}

/// Map a dB value to a 0.0..1.0 ratio within the meter range.
fn db_to_ratio(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    ((db - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB)).clamp(0.0, 1.0)
}

/// Build a compact horizontal VU meter and append its spans to the output.
///
/// Uses block characters for filled cells and light shade for empty cells.
/// The filled portion is colored by level (green/yellow/red) using
/// `theme::meter_color_db`. The meter is wrapped in half-block brackets
/// for visual framing.
fn build_horizontal_meter(spans: &mut Vec<Span<'_>>, peak_db: f32) {
    let ratio = db_to_ratio(peak_db);

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let filled = (ratio * METER_BAR_WIDTH as f32).round() as usize;
    let empty = METER_BAR_WIDTH.saturating_sub(filled);

    // Opening bracket.
    spans.push(Span::styled("\u{258c}", theme::style_text_dimmed()));

    // Filled cells: each cell colored by the dB level it represents.
    for i in 0..filled {
        // Determine the dB level at this cell position.
        #[allow(clippy::cast_precision_loss)]
        let cell_ratio = (i as f32 + 0.5) / METER_BAR_WIDTH as f32;
        let cell_db = cell_ratio.mul_add(METER_MAX_DB - METER_MIN_DB, METER_MIN_DB);
        let color = theme::meter_color_db(cell_db);
        spans.push(Span::styled("\u{2588}", Style::new().fg(color)));
    }

    // Empty cells: light shade.
    if empty > 0 {
        spans.push(Span::styled(
            "\u{2591}".repeat(empty),
            theme::style_text_dimmed(),
        ));
    }

    // Closing bracket.
    spans.push(Span::styled("\u{2590}", theme::style_text_dimmed()));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- db_to_ratio --------------------------------------------------------

    #[test]
    fn db_to_ratio_at_extremes() {
        assert!((db_to_ratio(METER_MIN_DB) - 0.0).abs() < f32::EPSILON);
        assert!((db_to_ratio(METER_MAX_DB) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_midpoint() {
        let mid = (METER_MIN_DB + METER_MAX_DB) / 2.0;
        assert!((db_to_ratio(mid) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_clamped() {
        assert!((db_to_ratio(-200.0) - 0.0).abs() < f32::EPSILON);
        assert!((db_to_ratio(20.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn db_to_ratio_nan_safe() {
        assert!((db_to_ratio(f32::NAN) - 0.0).abs() < f32::EPSILON);
        assert!((db_to_ratio(f32::INFINITY) - 0.0).abs() < f32::EPSILON);
        assert!((db_to_ratio(f32::NEG_INFINITY) - 0.0).abs() < f32::EPSILON);
    }

    // -- build_horizontal_meter ---------------------------------------------

    #[test]
    fn meter_at_silence_is_all_empty() {
        let mut spans = Vec::new();
        build_horizontal_meter(&mut spans, METER_MIN_DB);
        // Should have: opening bracket + empty cells + closing bracket.
        assert!(spans.len() >= 2); // at minimum bracket + bracket
        // The filled portion should be zero or nearly zero.
        let total_text: String = spans.iter().map(|s| s.content.to_string()).collect();
        // Empty cells use light shade ░.
        assert!(total_text.contains('\u{2591}'));
    }

    #[test]
    fn meter_at_full_is_all_filled() {
        let mut spans = Vec::new();
        build_horizontal_meter(&mut spans, METER_MAX_DB);
        // At 0 dB, all cells should be filled (block characters).
        let total_text: String = spans.iter().map(|s| s.content.to_string()).collect();
        // Filled cells use full block █.
        assert!(total_text.contains('\u{2588}'));
    }

    #[test]
    fn meter_has_brackets() {
        let mut spans = Vec::new();
        build_horizontal_meter(&mut spans, -30.0);
        let total_text: String = spans.iter().map(|s| s.content.to_string()).collect();
        // Should have opening ▌ and closing ▐.
        assert!(total_text.contains('\u{258c}'));
        assert!(total_text.contains('\u{2590}'));
    }

    #[test]
    fn meter_nan_produces_empty_meter() {
        let mut spans = Vec::new();
        build_horizontal_meter(&mut spans, f32::NAN);
        // NaN → 0 ratio → all empty cells.
        let total_text: String = spans.iter().map(|s| s.content.to_string()).collect();
        assert!(total_text.contains('\u{2591}')); // empty shade
    }

    // -- METER_BAR_WIDTH constant -------------------------------------------

    #[test]
    fn meter_bar_width_is_reasonable() {
        assert!(METER_BAR_WIDTH >= 4);
        assert!(METER_BAR_WIDTH <= 20);
    }
}
