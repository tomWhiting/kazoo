//! Persistent header strip: branding, transport, meters, and view tabs.
//!
//! Renders a 4-row bordered strip at the top of every view. The three content
//! rows pack a dense overview of the engine state:
//!
//! - **Row 1:** "KAZOO -- mouth noises" branding, the kazoo-mix desk link,
//!   status messages and the recording indicator.
//! - **Row 2:** Transport state, time, bar.beat.tick, BPM, loop, beat dots, view tabs.
//! - **Row 3:** Detected pitch, input level, L/R master VU meters, engine
//!   failure counters (while any is non-zero), CPU load.

use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use std::time::Instant;

use crate::app::{App, DeskView};
use crate::state::ActiveView;
use crate::status::StatusLevel;
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

// Compile-time check: the meter must be readable yet fit in the header row.
const _: () = assert!(METER_BAR_WIDTH >= 4 && METER_BAR_WIDTH <= 20);

// ---------------------------------------------------------------------------
// Public draw entry point
// ---------------------------------------------------------------------------

/// Draw the persistent header into the given area (expected to be 4 rows high).
///
/// The header is wrapped in a rounded border and contains three content lines:
/// branding/recording, transport/tabs, and pitch/meters/CPU.
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

    // Where this synth's audio is going: the desk strip, or standalone.
    left_spans.extend(desk_spans(&app.desk));

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

    // Status message (errors / confirmations) sits just left of the
    // recording indicator, truncated to the space that is left.
    let status_budget = width.saturating_sub(left_len + right_len + 2);
    let status_span = status_span(app, Instant::now(), status_budget);
    let status_len = status_span.as_ref().map_or(0, Span::width);

    let padding = width.saturating_sub(left_len + status_len + right_len);
    if padding > 0 {
        left_spans.push(Span::raw(" ".repeat(padding)));
    }
    if let Some(span) = status_span {
        left_spans.push(span);
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
fn draw_row_transport(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width as usize;
    let transport = &app.display.transport;

    // Transport state indicator.
    let (state_icon, state_label, state_style) = transport_state_indicator(app);

    // Time position MM:SS.mmm.
    let time_str = transport.position.format_time();

    // Bar.Beat.Tick.
    let bar_beat = transport
        .position
        .format_bar_beat_tick(transport.bpm, transport.beats_per_bar);

    // BPM display.
    let bpm_str = format!("\u{2669}{:.0}", transport.bpm);

    // Loop indicator.
    let loop_str = if transport.is_looping() {
        "\u{27f3}LOOP"
    } else {
        ""
    };

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
    if transport.is_looping() {
        left_spans.push(Span::styled(
            loop_str,
            Style::new()
                .fg(theme::ACCENT_PAUSE)
                .add_modifier(Modifier::BOLD),
        ));
        left_spans.push(Span::styled("   ", sep));
    }

    // Beat dots.
    push_beat_dots(&mut left_spans, transport);

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

/// The transport state icon, label and style for the header.
fn transport_state_indicator(app: &App) -> (&'static str, String, Style) {
    let transport = &app.display.transport;
    if let Some(count_in) = transport.count_in {
        let label = format!("COUNT {}/{}", count_in.bar, count_in.total);
        return (
            "",
            label,
            theme::style_recording(app.recording_blink_visible()),
        );
    }
    match transport.state {
        TransportState::Playing => ("\u{25b6}", " PLAY".to_owned(), theme::style_playing()),
        TransportState::Stopped => ("\u{25a0}", " STOP".to_owned(), theme::style_stopped()),
        TransportState::Paused => (
            "\u{2759}\u{2759}",
            " PAUSE".to_owned(),
            theme::style_paused(),
        ),
        TransportState::Recording => (
            "\u{25cf}",
            " REC".to_owned(),
            theme::style_recording(app.recording_blink_visible()),
        ),
    }
}

/// Append one dot per beat in the bar: the current beat highlighted, past
/// beats filled, future beats open.
fn push_beat_dots(spans: &mut Vec<Span<'_>>, transport: &kazoo_core::transport::TransportSnapshot) {
    let beats_per_bar = transport.beats_per_bar.max(1);
    let indicator = transport.beat;
    for beat in 0..beats_per_bar {
        let is_current = beat == indicator.beat && indicator.flash;
        let is_past = beat < indicator.beat && indicator.flash;
        if is_current {
            spans.push(Span::styled(
                "\u{25cf}",
                Style::new().fg(theme::ACCENT_RECORD),
            ));
        } else if is_past {
            spans.push(Span::styled(
                "\u{25cf}",
                Style::new().fg(theme::ACCENT_PLAY),
            ));
        } else {
            spans.push(Span::styled("\u{25cb}", theme::style_text_dimmed()));
        }
    }
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

    // Engine failure counters, in the space left before the CPU load.
    let left_content_width: usize = spans.iter().map(Span::width).sum();
    let cpu_width = cpu_str.len();
    let health_budget = width.saturating_sub(left_content_width + cpu_width + 2);
    if let Some(span) = health_span(app, Instant::now(), health_budget) {
        spans.push(span);
    }

    // CPU load (right-aligned).
    let left_content_width: usize = spans.iter().map(Span::width).sum();
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

/// Where this synth's audio is going, as header spans: the desk strip it is
/// plugged into, or that it is standalone and why, plus anything the link
/// has lost. Nothing when the engine was not asked to plug in.
fn desk_spans(desk: &DeskView) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(3);
    match desk {
        DeskView::Off => return spans,
        DeskView::Failed(reason) => spans.push(Span::styled(
            format!("  \u{25cb} standalone: {reason}"),
            Style::new().fg(theme::METER_RED),
        )),
        DeskView::Link(link) if link.connected => spans.push(Span::styled(
            link.strip.map_or_else(
                || "  \u{25cf} \u{2192} kazoo-mix".to_owned(),
                |strip| {
                    format!(
                        "  \u{25cf} \u{2192} kazoo-mix strip {}",
                        u16::from(strip) + 1
                    )
                },
            ),
            Style::new()
                .fg(theme::ACCENT_PLAY)
                .add_modifier(Modifier::BOLD),
        )),
        DeskView::Link(link) => spans.push(link.last_refusal.as_ref().map_or_else(
            || Span::styled("  \u{25cb} standalone", theme::style_text_dimmed()),
            |refusal| {
                Span::styled(
                    format!("  \u{25cb} standalone: {refusal}"),
                    Style::new().fg(theme::ACCENT_PAUSE),
                )
            },
        )),
    }
    if let DeskView::Link(link) = desk {
        if link.blocks_dropped > 0 || link.messages_dropped > 0 {
            spans.push(Span::styled(
                format!(
                    "  link lost {} blocks, {} msgs",
                    link.blocks_dropped, link.messages_dropped
                ),
                Style::new().fg(theme::ACCENT_PAUSE),
            ));
        }
    }
    spans
}

/// Every non-zero engine failure counter as one span (e.g.
/// `"⚠ mic 28 · params 3"`), truncated to `max_width` columns: red while a
/// counter has just risen, amber once it has been steady for a while.
fn health_span(app: &App, now: Instant, max_width: usize) -> Option<Span<'static>> {
    let counters: Vec<String> = app
        .engine_health
        .nonzero()
        .map(|(name, total)| format!("{name} {total}"))
        .collect();
    if counters.is_empty() {
        return None;
    }
    let text = format!("\u{26a0} {}", counters.join(" \u{b7} "));
    let text = truncate_to_width(&text, max_width);
    if text.is_empty() {
        return None;
    }
    let style = if app.engine_health.is_fresh(now) {
        Style::new()
            .fg(theme::METER_RED)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme::ACCENT_PAUSE)
    };
    Some(Span::styled(text, style))
}

/// Build the status-line span for the message visible at `now`, truncated
/// to at most `max_width` columns. Returns `None` when there is no live
/// message or no room to show one.
fn status_span(app: &App, now: Instant, max_width: usize) -> Option<Span<'static>> {
    let message = app.status.visible(now)?;
    let (icon, style) = match message.level {
        StatusLevel::Error => (
            "\u{26a0} ",
            Style::new()
                .fg(theme::METER_RED)
                .add_modifier(Modifier::BOLD),
        ),
        StatusLevel::Info => ("\u{2713} ", theme::style_text_secondary()),
    };
    let text = if message.repeats > 1 {
        format!("{icon}{} (\u{00d7}{})", message.text, message.repeats)
    } else {
        format!("{icon}{}", message.text)
    };
    let text = truncate_to_width(&text, max_width);
    if text.is_empty() {
        return None;
    }
    Some(Span::styled(text, style))
}

/// Truncate `text` to at most `max_width` characters, ending with an
/// ellipsis when anything was cut.
fn truncate_to_width(text: &str, max_width: usize) -> String {
    if text.chars().count() <= max_width {
        return text.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(max_width - 1).collect();
    out.push('\u{2026}');
    out
}

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

    let filled = (ratio * METER_BAR_WIDTH as f32).round() as usize;
    let empty = METER_BAR_WIDTH.saturating_sub(filled);

    // Opening bracket.
    spans.push(Span::styled("\u{258c}", theme::style_text_dimmed()));

    // Filled cells: each cell colored by the dB level it represents.
    for i in 0..filled {
        // Determine the dB level at this cell position.
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
        let mid = f32::midpoint(METER_MIN_DB, METER_MAX_DB);
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

    // -- status line --------------------------------------------------------

    fn render_header(app: &App, width: u16) -> String {
        let backend = ratatui::backend::TestBackend::new(width, 5);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw(frame, app, Rect::new(0, 0, width, 5)))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..5 {
            for x in 0..width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn header_shows_status_error() {
        let mut app = crate::test_support::TestApp::empty();
        app.status.error("Mute failed: Engine not running");
        let text = render_header(&app, 120);
        assert!(text.contains("Mute failed: Engine not running"), "{text}");
    }

    #[test]
    fn header_shows_repeat_count() {
        let mut app = crate::test_support::TestApp::empty();
        app.status.error("boom");
        app.status.error("boom");
        let text = render_header(&app, 120);
        assert!(text.contains("boom (\u{00d7}2)"), "{text}");
    }

    #[test]
    fn header_without_status_renders_branding() {
        let app = crate::test_support::TestApp::empty();
        let text = render_header(&app, 120);
        assert!(text.contains("KAZOO"), "{text}");
    }

    fn link(connected: bool) -> kazoo_core::ipc::link::LinkStatus {
        kazoo_core::ipc::link::LinkStatus {
            connected,
            strip: None,
            blocks_sent: 0,
            blocks_dropped: 0,
            messages_dropped: 0,
            connections: 0,
            last_refusal: None,
        }
    }

    #[test]
    fn header_names_the_desk_strip() {
        let mut app = crate::test_support::TestApp::empty();
        app.desk = DeskView::Link(kazoo_core::ipc::link::LinkStatus {
            strip: Some(2),
            ..link(true)
        });
        let text = render_header(&app, 120);
        assert!(text.contains("\u{2192} kazoo-mix strip 3"), "{text}");
    }

    #[test]
    fn header_shows_standalone_and_why() {
        let mut app = crate::test_support::TestApp::empty();
        app.desk = DeskView::Link(link(false));
        assert!(render_header(&app, 120).contains("\u{25cb} standalone"));

        app.desk = DeskView::Link(kazoo_core::ipc::link::LinkStatus {
            last_refusal: Some("desk is full".to_owned()),
            ..link(false)
        });
        let text = render_header(&app, 120);
        assert!(text.contains("standalone: desk is full"), "{text}");

        app.desk = DeskView::Failed("could not start the desk link: boom".to_owned());
        let text = render_header(&app, 120);
        assert!(text.contains("standalone: could not start"), "{text}");
    }

    #[test]
    fn header_shows_link_losses() {
        let mut app = crate::test_support::TestApp::empty();
        app.desk = DeskView::Link(kazoo_core::ipc::link::LinkStatus {
            blocks_dropped: 4,
            messages_dropped: 1,
            ..link(true)
        });
        let text = render_header(&app, 140);
        assert!(text.contains("link lost 4 blocks, 1 msgs"), "{text}");
    }

    #[test]
    fn header_without_a_desk_link_says_nothing_about_it() {
        let app = crate::test_support::TestApp::empty();
        assert_eq!(app.desk, DeskView::Off);
        let text = render_header(&app, 120);
        assert!(!text.contains("standalone"), "{text}");
        assert!(!text.contains("kazoo-mix"), "{text}");
    }

    #[test]
    fn header_shows_engine_failure_counters() {
        let mut app = crate::test_support::TestApp::empty();
        let clean = render_header(&app, 160);
        assert!(!clean.contains('\u{26a0}'), "{clean}");

        let stats = kazoo_core::engine::EngineStatsSnapshot {
            mic_samples_dropped: 28,
            params_rejected: 3,
            ..kazoo_core::engine::EngineStatsSnapshot::default()
        };
        let message = app.engine_health.observe(stats, Instant::now());
        assert!(message.is_some());
        let text = render_header(&app, 160);
        assert!(text.contains("\u{26a0} params 3 \u{b7} mic 28"), "{text}");
    }

    #[test]
    fn truncate_to_width_adds_ellipsis() {
        assert_eq!(truncate_to_width("abcdef", 4), "abc\u{2026}");
        assert_eq!(truncate_to_width("abc", 4), "abc");
        assert_eq!(truncate_to_width("abc", 0), "");
    }
}
