//! Terminal rendering for the `kazoo-mix` desk.
//!
//! The UI is deliberately styled like a compact analogue console: dark panel,
//! brass labels, stereo master meters, and strip status that reads like hardware
//! rather than a debug log.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Row, Table};

use kazoo_mix::engine::StereoLevel;

use crate::App;

const BG: Color = Color::Rgb(0x17, 0x14, 0x12);
const PANEL: Color = Color::Rgb(0x24, 0x1E, 0x19);
const PANEL_ALT: Color = Color::Rgb(0x2E, 0x26, 0x20);
const TEXT: Color = Color::Rgb(0xE7, 0xDC, 0xCB);
const TEXT_DIM: Color = Color::Rgb(0x96, 0x87, 0x74);
const BRASS: Color = Color::Rgb(0xCF, 0xA2, 0x47);
const SAGE: Color = Color::Rgb(0x87, 0xB3, 0x61);
const AMBER: Color = Color::Rgb(0xDE, 0xA2, 0x34);
const RED: Color = Color::Rgb(0xDE, 0x58, 0x45);
const STEEL: Color = Color::Rgb(0x73, 0x6B, 0x62);

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App) {
    frame.render_widget(
        Block::default().style(Style::default().bg(BG)),
        frame.area(),
    );

    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(7),
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_console(frame, chunks[1], app);
    draw_status(frame, chunks[2], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let play = if app.transport_playing { "RUN" } else { "STOP" };
    let met = if app.metronome { "CLK ●" } else { "CLK ○" };
    let buffer = app
        .audio_info
        .buffer_size
        .map_or_else(|| "default".to_string(), |frames| frames.to_string());
    let line = Line::from(vec![
        Span::styled(
            " KAZOO MIX ",
            Style::default()
                .fg(BG)
                .bg(BRASS)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            play,
            Style::default().fg(if app.transport_playing { SAGE } else { AMBER }),
        ),
        Span::styled(
            format!("  {:.2} BPM  {met}  ", app.bpm),
            Style::default().fg(TEXT),
        ),
        Span::styled(
            format!(
                "{} Hz / {} ch / buffer {}",
                app.audio_info.sample_rate, app.audio_info.channels, buffer
            ),
            Style::default().fg(TEXT_DIM),
        ),
    ]);

    frame.render_widget(
        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .style(Style::default().bg(PANEL).fg(STEEL)),
        ),
        area,
    );
}

fn draw_console(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let rows = app.channels.iter().enumerate().map(|(idx, channel)| {
        let name = name_string(channel.name);
        let health = if channel.connected { "LIVE" } else { "OPEN" };
        let state = if channel.muted {
            "MUT"
        } else if channel.soloed {
            "SOL"
        } else {
            "---"
        };
        Row::new([
            if idx == app.selected_channel {
                format!("▶CH {:02}", usize::from(channel.id.0) + 1)
            } else {
                format!(" CH {:02}", usize::from(channel.id.0) + 1)
            },
            name,
            health.to_string(),
            state.to_string(),
            knob("TRIM", channel.gain * 0.5),
            pan_knob(channel.pan),
            led_meter(max_stereo(channel.rms), max_stereo(channel.peak), 14),
            fader(channel.gain * 0.5, 13),
            format_db(max_stereo(channel.peak)),
            channel.underruns.to_string(),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(7),
            Constraint::Length(12),
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(16),
            Constraint::Length(15),
            Constraint::Length(8),
            Constraint::Length(7),
        ],
    )
    .header(
        Row::new([
            "Strip", "Source", "I/O", "Bus", "Trim", "Pan", "VU", "Fader", "Peak", "Drops",
        ])
        .style(Style::default().fg(BRASS).add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .title(" Console / Channel Strips — trim · pan · meter · long throw fader ")
            .borders(Borders::ALL)
            .style(Style::default().bg(PANEL_ALT).fg(STEEL)),
    )
    .row_highlight_style(Style::default().bg(PANEL));

    frame.render_widget(table, area);
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(50),
        ])
        .split(area);

    draw_meter_block(frame, chunks[0], "Master Peak", app.master_peak);
    draw_meter_block(frame, chunks[1], "Master RMS", app.master_rms);

    let status = vec![
        Line::from(vec![
            Span::styled("session ", Style::default().fg(TEXT_DIM)),
            Span::styled(
                app.session.runtime_dir.display().to_string(),
                Style::default().fg(TEXT),
            ),
        ]),
        Line::from(vec![
            Span::styled("control ", Style::default().fg(TEXT_DIM)),
            Span::styled(
                app.session.control_socket.display().to_string(),
                Style::default().fg(TEXT),
            ),
        ]),
        Line::from(format!(
            "frames {}   runtime {:.1}s   accepts/errors {}/{}",
            app.frames_rendered,
            app.started.elapsed().as_secs_f64(),
            app.control.accepted_connections,
            app.control.accept_errors
        )),
        Line::from(format!(
            "stream errors {}   xruns {}   master pk {}   master rms {}",
            app.stream_errors,
            app.xruns,
            format_db(max_stereo(app.master_peak)),
            format_db(max_stereo(app.master_rms))
        )),
        Line::from(
            format!(
                "selected CH {:02}   keys: 1-8 select  j/k move  [/] fader  h/l pan  m mute  s solo  c clock  q quit",
                app.selected_channel + 1
            )
            .fg(TEXT_DIM),
        ),
    ];
    frame.render_widget(
        Paragraph::new(status).block(
            Block::default()
                .title(" Machine Room ")
                .borders(Borders::ALL)
                .style(Style::default().bg(PANEL).fg(STEEL)),
        ),
        chunks[2],
    );
}

fn draw_meter_block(frame: &mut Frame<'_>, area: Rect, title: &str, level: StereoLevel) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    let left = level.left.clamp(0.0, 1.0);
    let right = level.right.clamp(0.0, 1.0);
    let left_label = format!("L {}", format_db(level.left));
    let right_label = format!("R {}", format_db(level.right));

    frame.render_widget(
        Gauge::default()
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .style(Style::default().bg(PANEL).fg(STEEL)),
            )
            .gauge_style(Style::default().fg(level_color(left)).bg(PANEL))
            .label(left_label)
            .ratio(f64::from(left)),
        chunks[0],
    );
    frame.render_widget(
        Gauge::default()
            .block(
                Block::default()
                    .title(" ")
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .style(Style::default().bg(PANEL).fg(STEEL)),
            )
            .gauge_style(Style::default().fg(level_color(right)).bg(PANEL))
            .label(right_label)
            .ratio(f64::from(right)),
        chunks[1],
    );
}

fn knob(label: &str, value: f32) -> String {
    let pointer = knob_pointer(value);
    format!("{label} {pointer}")
}

fn pan_knob(pan: f32) -> String {
    format!("PAN {}", knob_pointer((pan + 1.0) * 0.5))
}

fn knob_pointer(value: f32) -> char {
    const POINTERS: [char; 9] = ['◜', '◠', '◝', '◞', '●', '◟', '◜', '◠', '◝'];
    let idx = ((value.clamp(0.0, 1.0) * (POINTERS.len() - 1) as f32).round() as usize)
        .min(POINTERS.len() - 1);
    POINTERS[idx]
}

fn led_meter(rms: f32, peak: f32, width: usize) -> String {
    let rms_cells = ((rms.clamp(0.0, 1.0) * width as f32).round() as usize).min(width);
    let peak_cell =
        ((peak.clamp(0.0, 1.0) * width as f32).round() as usize).min(width.saturating_sub(1));
    let mut out = String::with_capacity(width + 2);
    out.push('[');
    for idx in 0..width {
        if peak > 0.000_001 && idx == peak_cell {
            out.push('┃');
        } else if idx < rms_cells {
            out.push(if idx > width * 4 / 5 { '▰' } else { '▱' });
        } else {
            out.push('·');
        }
    }
    out.push(']');
    out
}

fn fader(value: f32, width: usize) -> String {
    let cap = ((value.clamp(0.0, 1.0) * width.saturating_sub(1) as f32).round() as usize)
        .min(width.saturating_sub(1));
    let mut out = String::with_capacity(width + 2);
    out.push('╞');
    for idx in 0..width {
        out.push(if idx == cap { '▣' } else { '═' });
    }
    out.push('╡');
    out
}

fn name_string(bytes: [u8; 12]) -> String {
    let len = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    if len == 0 {
        return "empty".to_string();
    }
    String::from_utf8_lossy(&bytes[..len]).into_owned()
}

fn format_db(level: f32) -> String {
    if level <= 0.000_001 {
        "-∞ dB".to_string()
    } else {
        format!("{:.1} dB", 20.0 * level.log10())
    }
}

fn level_color(level: f32) -> Color {
    if level >= 0.9 {
        RED
    } else if level >= 0.7 {
        AMBER
    } else {
        SAGE
    }
}

fn max_stereo(level: StereoLevel) -> f32 {
    level.left.max(level.right)
}
