//! Audio I/O device configuration view.
//!
//! Lists available input and output audio devices, shows the current
//! selection, and displays engine configuration (sample rate, buffer size,
//! latency estimate).
//!
//! Layout:
//! ```text
//! ╭─ AUDIO I/O ─────────────────────────────────────────────────────────╮
//! │  ╭─ Input Devices ────────────╮  ╭─ Output Devices ──────────────╮ │
//! │  │ > Built-in Microphone      │  │ > Built-in Output             │ │
//! │  │   USB Audio Interface      │  │   USB Audio Interface         │ │
//! │  │   Bluetooth Headset        │  │   Bluetooth Headset           │ │
//! │  ╰─────────────────────────────╯  ╰───────────────────────────────╯ │
//! │                                                                      │
//! │  ╭─ Settings ───────────────────────────────────────────────────────╮│
//! │  │ Sample Rate: 44100 Hz    Buffer Size: 128 samples   ~2.9ms     ││
//! │  │ Input Channels: 1 (mono)  Output Channels: 2 (stereo)          ││
//! │  ╰───────────────────────────────────────────────────────────────────╯│
//! ╰──────────────────────────────────────────────────────────────────────╯
//! ```

use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::app::App;
use crate::state::DeviceListFocus;
use crate::theme;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Draw the Audio I/O view into the given content area.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(false))
        .title(" Audio I/O ")
        .title_style(theme::style_panel_title(false));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 40 || inner.height < 8 {
        return;
    }

    // Split vertically: device lists (top) | settings (bottom) | hint.
    let v_chunks = Layout::vertical([
        Constraint::Length(1), // top margin
        Constraint::Min(8),    // device lists
        Constraint::Length(1), // gap
        Constraint::Length(5), // settings
        Constraint::Length(1), // hint bar
    ])
    .split(inner);

    // Device lists: input (left) | output (right).
    let device_cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(v_chunks[1]);

    draw_input_devices(frame, app, device_cols[0]);
    draw_output_devices(frame, app, device_cols[1]);
    draw_settings(frame, app, v_chunks[3]);

    // Hint bar.
    if v_chunks[4].height >= 1 {
        let hint = Paragraph::new(Line::from(vec![
            Span::styled("  Tab", theme::style_help_key()),
            Span::styled(" switch list  ", theme::style_help_desc()),
            Span::styled("j/k", theme::style_help_key()),
            Span::styled(" select device  ", theme::style_help_desc()),
            Span::styled("Enter", theme::style_help_key()),
            Span::styled(" apply", theme::style_help_desc()),
        ]));
        frame.render_widget(hint, v_chunks[4]);
    }
}

// ---------------------------------------------------------------------------
// Device lists
// ---------------------------------------------------------------------------

fn draw_input_devices(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.audio_io_state.focus == DeviceListFocus::Input;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(focused))
        .title(" Input Devices ")
        .title_style(theme::style_panel_title(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width < 10 {
        return;
    }

    let devices = &app.audio_io_state.input_devices;
    if devices.is_empty() {
        let lines = vec![Line::from(Span::styled(
            " No input devices found",
            theme::style_text_dimmed(),
        ))];
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let selected = app.audio_io_state.selected_input_device;
    let max_visible = inner.height as usize;
    let mut lines = Vec::with_capacity(max_visible);
    for (i, name) in devices.iter().enumerate().take(max_visible) {
        let is_selected = i == selected;
        let prefix = if is_selected { " > " } else { "   " };
        let style = if is_selected && focused {
            theme::style_selected()
        } else if is_selected {
            theme::style_text()
        } else {
            theme::style_text_secondary()
        };
        let max_name_len = (inner.width as usize).saturating_sub(4);
        let display_name: String = if name.chars().count() > max_name_len {
            let mut s: String = name.chars().take(max_name_len.saturating_sub(1)).collect();
            s.push('\u{2026}');
            s
        } else {
            name.clone()
        };
        lines.push(Line::from(Span::styled(
            format!("{prefix}{display_name}"),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_output_devices(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.audio_io_state.focus == DeviceListFocus::Output;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(focused))
        .title(" Output Devices ")
        .title_style(theme::style_panel_title(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width < 10 {
        return;
    }

    let devices = &app.audio_io_state.output_devices;
    if devices.is_empty() {
        let lines = vec![Line::from(Span::styled(
            " No output devices found",
            theme::style_text_dimmed(),
        ))];
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let selected = app.audio_io_state.selected_output_device;
    let max_visible = inner.height as usize;
    let mut lines = Vec::with_capacity(max_visible);
    for (i, name) in devices.iter().enumerate().take(max_visible) {
        let is_selected = i == selected;
        let prefix = if is_selected { " > " } else { "   " };
        let style = if is_selected && focused {
            theme::style_selected()
        } else if is_selected {
            theme::style_text()
        } else {
            theme::style_text_secondary()
        };
        let max_name_len = (inner.width as usize).saturating_sub(4);
        let display_name: String = if name.chars().count() > max_name_len {
            let mut s: String = name.chars().take(max_name_len.saturating_sub(1)).collect();
            s.push('\u{2026}');
            s
        } else {
            name.clone()
        };
        lines.push(Line::from(Span::styled(
            format!("{prefix}{display_name}"),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Settings panel
// ---------------------------------------------------------------------------

fn draw_settings(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.audio_io_state.focus == DeviceListFocus::Settings;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::style_panel_border(focused))
        .title(" Settings ")
        .title_style(theme::style_panel_title(focused));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width < 10 {
        return;
    }

    let sample_rate = app.engine.sample_rate();
    let buffer_size = app.engine.buffer_size();
    let latency_ms = if sample_rate > 0 {
        (buffer_size as f64 / f64::from(sample_rate)) * 1000.0
    } else {
        0.0
    };

    let lines = vec![Line::from(vec![
        Span::styled(" Sample Rate: ", theme::style_text_secondary()),
        Span::styled(format!("{sample_rate} Hz"), theme::style_text()),
        Span::styled("    Buffer Size: ", theme::style_text_secondary()),
        Span::styled(format!("{buffer_size} samples"), theme::style_text()),
        Span::styled(
            format!("    ~{latency_ms:.1}ms"),
            theme::style_text_secondary(),
        ),
    ])];
    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
