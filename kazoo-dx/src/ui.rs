//! Ratatui front panel for kazoo-dx.
//!
//! Everything is edited in place: the operator grid *is* the control surface,
//! so there is no separate drawer duplicating what is on screen.

use std::sync::atomic::Ordering;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::synth::{ALGORITHMS, OPERATORS, OperatorField, PATCHES, note_name};
use kazoo_core::ipc::link::LinkStatus;

use crate::{App, Focus, GLOBAL_FIELDS, PhraseState, piano_offset};

const ACCENT: Color = Color::Rgb(255, 176, 0);
const CARRIER: Color = Color::Rgb(90, 220, 170);
const MODULATOR: Color = Color::Rgb(130, 160, 255);
const DIM: Color = Color::DarkGray;
/// Width that shows every operator field plus the envelope sketch.
const GRID_WIDTH: u16 = 86;

pub fn draw(frame: &mut Frame, app: &App) {
    let [header, body, keys, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(14),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app);

    // The operator grid needs its full width to show every field; the scope
    // and voices take whatever is left.
    let left_width = GRID_WIDTH
        .max(body.width * 3 / 5)
        .min(body.width.saturating_sub(16));
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(body);
    let [grid, algo] =
        Layout::vertical([Constraint::Length(OPERATORS as u16 + 5), Constraint::Min(5)])
            .areas(left);
    let [scope, voices] =
        Layout::vertical([Constraint::Min(6), Constraint::Length(4)]).areas(right);

    draw_operator_grid(frame, grid, app);
    draw_algorithm(frame, algo, app);
    draw_scope(frame, scope, app);
    draw_voices(frame, voices, app);
    draw_keyboard(frame, keys, app);

    let mut footer_spans = vec![
        Span::styled(" ? ", Style::new().fg(Color::Black).bg(ACCENT)),
        Span::raw(" help  "),
    ];
    footer_spans.extend(delivery_warnings(app));
    footer_spans.push(Span::styled(app.status.as_str(), Style::new().fg(DIM)));
    frame.render_widget(Paragraph::new(Line::from(footer_spans)), footer);

    if app.show_help {
        draw_help(frame);
    }
}

/// Warnings for messages that could not be delivered between the UI and the
/// audio thread. Empty while everything is getting through.
fn delivery_warnings(app: &App) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if app.commands_dropped > 0 {
        spans.push(Span::styled(
            format!(
                "audio busy: {} edits/notes lost (space clears notes)  ",
                app.commands_dropped
            ),
            Style::new().fg(Color::Black).bg(Color::Red),
        ));
    }
    if app.stats.stream_lost.load(Ordering::Acquire) {
        spans.push(Span::styled(
            "AUDIO DEVICE LOST - restart kazoo-dx  ",
            Style::new().fg(Color::Black).bg(Color::Red),
        ));
    } else {
        let errors = app.stats.stream_errors.load(Ordering::Relaxed);
        if errors > 0 {
            spans.push(Span::styled(
                format!("{errors} audio stream errors  "),
                Style::new().fg(Color::Yellow),
            ));
        }
    }
    let desk_lost = app.stats.desk_lost.load(Ordering::Relaxed);
    if desk_lost > 0 {
        spans.push(Span::styled(
            format!("desk: {desk_lost} transport messages lost  "),
            Style::new().fg(Color::Yellow),
        ));
    }
    let display_dropped = app.stats.display_dropped.load(Ordering::Relaxed);
    if display_dropped > 0 {
        spans.push(Span::styled(
            format!("screen lagging: {display_dropped} frames skipped  "),
            Style::new().fg(Color::Yellow),
        ));
    }
    spans
}

/// The phrase: playing or not, its tempo, and who is driving it (the desk
/// when plugged in, this synth when not).
fn phrase_spans(app: &App) -> Vec<Span<'static>> {
    if app.phrase == PhraseState::Absent {
        return Vec::new();
    }
    let driver = if app.link.connected { "desk" } else { "local" };
    let (mark, style) = match app.phrase {
        PhraseState::Looping => ("  ▶ phrase", Style::new().fg(CARRIER)),
        PhraseState::Stopped | PhraseState::Absent => ("  ■ phrase (n)", Style::new().fg(DIM)),
    };
    vec![
        Span::styled(mark, style),
        Span::styled(
            format!(" {:.0} BPM [ ] ({driver})", app.bpm),
            Style::new().fg(DIM),
        ),
    ]
}

/// Where this synth's audio is going: the desk strip it is plugged into, or
/// why it is not, plus any audio or messages the link has lost.
fn link_spans(link: &LinkStatus) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(3);
    spans.push(Span::raw("  "));
    if link.connected {
        spans.push(Span::styled(
            link.strip.map_or_else(
                || "● → kazoo-mix".to_owned(),
                |strip| format!("● → kazoo-mix strip {}", u16::from(strip) + 1),
            ),
            Style::new().fg(CARRIER),
        ));
    } else if let Some(refusal) = &link.last_refusal {
        spans.push(Span::styled(
            format!("○ standalone: {refusal}"),
            Style::new().fg(ACCENT).add_modifier(Modifier::DIM),
        ));
    } else {
        spans.push(Span::styled("○ standalone", Style::new().fg(DIM)));
    }
    if link.blocks_dropped > 0 || link.messages_dropped > 0 {
        spans.push(Span::styled(
            format!(
                "  link lost {} blocks, {} msgs",
                link.blocks_dropped, link.messages_dropped
            ),
            Style::new().fg(ACCENT),
        ));
    }
    spans
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let alg = ALGORITHMS[app.patch.algorithm];
    let global = |i: usize, text: String| {
        let style = if app.focus == Focus::Global(i) {
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::White)
        };
        Span::styled(text, style)
    };
    let mut spans = vec![
        Span::styled(
            " KAZOO-DX ",
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{}/{} {}{}",
                app.patch_index + 1,
                PATCHES.len(),
                app.patch.name,
                // Edited away from the factory sound: tab or 1-6 restores it.
                if app.patch == PATCHES[app.patch_index] {
                    ""
                } else {
                    " *"
                }
            ),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::raw(format!("oct {}  vel {}  ", app.octave, app.velocity)),
        level_meter(app.display.peak),
        Span::raw("  "),
    ];
    spans.extend(phrase_spans(app));
    spans.extend(link_spans(&app.link));
    let line = Line::from(spans);
    let controls = Line::from(vec![
        global(0, format!(" ALG {} ", app.patch.algorithm + 1)),
        Span::styled(format!(" {} ", alg.diagram), Style::new().fg(DIM)),
        global(1, format!(" FB {:>3.0}% ", app.patch.feedback * 100.0)),
        Span::raw(" "),
        global(2, format!(" LFO {:>4.1}Hz ", lfo_hz(app.patch.lfo_rate))),
        Span::raw(" "),
        global(3, format!(" VIB {:>3.0}% ", app.patch.vibrato * 100.0)),
        Span::raw(" "),
        global(4, format!(" VOL {:>3.0}% ", app.master * 100.0)),
    ]);
    frame.render_widget(
        Paragraph::new(vec![line, controls])
            .block(Block::bordered().border_style(Style::new().fg(DIM))),
        area,
    );
}

/// Mirror of the engine's LFO rate mapping, for display.
fn lfo_hz(rate: f32) -> f32 {
    0.1 * 120.0_f32.powf(rate.clamp(0.0, 1.0))
}

fn level_meter(peak: f32) -> Span<'static> {
    const WIDTH: usize = 10;
    const FLOOR_DB: f32 = -48.0;
    // Decibel scale: -48 dBFS and below is empty, 0 dBFS is full.
    let db = if peak > 0.0 && peak.is_finite() {
        20.0 * peak.log10()
    } else {
        FLOOR_DB
    };
    let fill = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
    let lit = ((fill * WIDTH as f32).round() as usize).min(WIDTH);
    let color = if db > -1.0 {
        Color::Red
    } else if db > -6.0 {
        ACCENT
    } else {
        CARRIER
    };
    Span::styled(
        format!("{}{} {db:>4.0}dB", "█".repeat(lit), "·".repeat(WIDTH - lit)),
        Style::new().fg(color),
    )
}

fn draw_operator_grid(frame: &mut Frame, area: Rect, app: &App) {
    let alg = ALGORITHMS[app.patch.algorithm];
    let mut lines = Vec::with_capacity(OPERATORS + 2);

    let mut header = vec![Span::styled(format!("{:<9}", "op"), Style::new().fg(DIM))];
    header.extend(
        OperatorField::ALL
            .iter()
            .map(|f| Span::styled(format!("{:>8}", f.label()), Style::new().fg(DIM))),
    );
    header.push(Span::styled("  env", Style::new().fg(DIM)));
    lines.push(Line::from(header));

    for op in 0..OPERATORS {
        let carrier = alg.carriers & (1 << op) != 0;
        let role_color = if carrier { CARRIER } else { MODULATOR };
        let role = if carrier { "out" } else { "mod" };
        let fb = if op == OPERATORS - 1 { "↺" } else { " " };
        let mut spans = vec![Span::styled(
            format!("OP{} {role}{fb} ", op + 1),
            Style::new().fg(role_color).add_modifier(Modifier::BOLD),
        )];
        let params = &app.patch.operators[op];
        for (col, field) in OperatorField::ALL.iter().enumerate() {
            let value = params.get(*field);
            let text = match field {
                OperatorField::Ratio => format!("{value:>7.2}x"),
                OperatorField::Detune => format!("{value:>+7.0}c"),
                OperatorField::Attack => format!("{:>7}", seconds(params.attack_seconds())),
                OperatorField::Decay => format!("{:>7}", seconds(params.decay_seconds())),
                OperatorField::Release => format!("{:>7}", seconds(params.release_seconds())),
                _ => format!("{:>7.0}%", value * 100.0),
            };
            let text = format!("{text:>8}");
            let style = if app.focus == Focus::Operator(op, col) {
                Style::new()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::White)
            };
            spans.push(Span::styled(text, style));
        }
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            envelope_sketch(params),
            Style::new().fg(role_color),
        ));
        lines.push(Line::from(spans));
    }

    let hint = match app.focus {
        Focus::Global(i) => format!("editing {}", GLOBAL_FIELDS[i]),
        Focus::Operator(op, f) => format!("editing OP{} {}", op + 1, OperatorField::ALL[f].label()),
    };
    lines.push(Line::from(Span::styled(
        format!("{hint}  ·  arrows move  ·  - / = change  ·  shift for big steps"),
        Style::new().fg(DIM),
    )));

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" operators ")
                .border_style(Style::new().fg(DIM)),
        ),
        area,
    );
}

fn seconds(s: f32) -> String {
    if s < 1.0 {
        format!("{:.0}ms", s * 1_000.0)
    } else {
        format!("{s:.2}s")
    }
}

/// Eight-cell sketch of an operator's ADSR shape.
fn envelope_sketch(p: &crate::synth::OperatorParams) -> String {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let bar = |v: f32| {
        let i = (v.clamp(0.0, 1.0) * 7.0).round() as usize;
        BARS[i.min(7)]
    };
    let attack_peak = p.attack.mul_add(-0.6, 1.0);
    let decay_mid = ((1.0 - p.sustain) * (1.0 - p.decay)).mul_add(0.3, p.sustain);
    [
        bar(attack_peak * 0.5),
        bar(attack_peak),
        bar(1.0),
        bar(decay_mid.max(p.sustain)),
        bar(p.sustain),
        bar(p.sustain),
        bar(p.sustain * p.release),
        bar(p.sustain * p.release * 0.3),
    ]
    .iter()
    .collect()
}

fn draw_algorithm(frame: &mut Frame, area: Rect, app: &App) {
    let alg = ALGORITHMS[app.patch.algorithm];
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("algorithm {}  ", app.patch.algorithm + 1),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(alg.diagram, Style::new().fg(Color::White)),
    ])];
    for op in (0..OPERATORS).rev() {
        let mods: Vec<String> = (0..OPERATORS)
            .filter(|src| alg.modulators[op] & (1 << src) != 0)
            .map(|src| format!("OP{}", src + 1))
            .collect();
        let carrier = alg.carriers & (1 << op) != 0;
        let mut spans = Vec::new();
        if mods.is_empty() {
            spans.push(Span::styled("        ", Style::new()));
        } else {
            spans.push(Span::styled(
                format!("{:>8}", mods.join("+")),
                Style::new().fg(MODULATOR),
            ));
        }
        spans.push(Span::styled(
            if mods.is_empty() { "   " } else { " ─▶" },
            Style::new().fg(DIM),
        ));
        spans.push(Span::styled(
            format!(
                " [OP{}{}] ",
                op + 1,
                if op == OPERATORS - 1 { "↺" } else { "" }
            ),
            Style::new()
                .fg(if carrier { CARRIER } else { MODULATOR })
                .add_modifier(Modifier::BOLD),
        ));
        if carrier {
            spans.push(Span::styled("─▶ OUT", Style::new().fg(CARRIER)));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" routing ")
                .border_style(Style::new().fg(DIM)),
        ),
        area,
    );
}

fn draw_scope(frame: &mut Frame, area: Rect, app: &App) {
    let scope = &app.display.scope;
    // Trigger on a rising zero crossing so periodic tones stand still.
    let window = scope.len() / 2;
    let start = (1..scope.len() - window)
        .find(|&i| scope[i - 1] <= 0.0 && scope[i] > 0.0)
        .unwrap_or(0);
    let samples = &scope[start..start + window];
    // Auto-gain so quiet tails stay readable; the title shows the zoom.
    let peak = samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
    let zoom = if peak > 1.0e-4 {
        (0.9 / peak).min(16.0)
    } else {
        1.0
    };
    let canvas = Canvas::default()
        .block(
            Block::bordered()
                .title(format!(" scope  x{zoom:.1} "))
                .border_style(Style::new().fg(DIM)),
        )
        .x_bounds([0.0, window as f64])
        .y_bounds([-1.0, 1.0])
        .paint(move |ctx| {
            for (i, pair) in samples.windows(2).enumerate() {
                ctx.draw(&CanvasLine {
                    x1: i as f64,
                    y1: f64::from(pair[0] * zoom),
                    x2: (i + 1) as f64,
                    y2: f64::from(pair[1] * zoom),
                    color: CARRIER,
                });
            }
        });
    frame.render_widget(canvas, area);
}

fn draw_voices(frame: &mut Frame, area: Rect, app: &App) {
    let spans: Vec<Span> = app
        .display
        .voices
        .iter()
        .map(|voice| match voice {
            Some((note, true)) => Span::styled(
                format!(" {:<4}", note_name(*note)),
                Style::new()
                    .fg(Color::Black)
                    .bg(CARRIER)
                    .add_modifier(Modifier::BOLD),
            ),
            Some((note, false)) => Span::styled(
                format!(" {:<4}", note_name(*note)),
                Style::new().fg(CARRIER),
            ),
            None => Span::styled("  ·  ", Style::new().fg(DIM)),
        })
        .collect();
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .title(" voices ")
                    .border_style(Style::new().fg(DIM)),
            ),
        area,
    );
}

fn draw_keyboard(frame: &mut Frame, area: Rect, app: &App) {
    const BLACK_ROW: &str = " w e   t y u   o p  ";
    const WHITE_ROW: &str = "a s d f g h j k l ; ";
    let held = app.held_chars();
    let row = |layout: &str| -> Line<'static> {
        let spans: Vec<Span> = layout
            .chars()
            .map(|c| {
                if c == ' ' {
                    Span::raw(" ")
                } else if held.contains(&c) {
                    Span::styled(
                        c.to_string(),
                        Style::new()
                            .fg(Color::Black)
                            .bg(ACCENT)
                            .add_modifier(Modifier::BOLD),
                    )
                } else if piano_offset(c).is_some() {
                    Span::styled(c.to_string(), Style::new().fg(Color::White))
                } else {
                    Span::raw(" ")
                }
            })
            .collect();
        Line::from(spans)
    };
    let base = note_name((app.octave.clamp(0, 8) as u8 + 1) * 12);
    let mut black = row(BLACK_ROW);
    black.spans.insert(0, Span::raw("  "));
    let mut white = row(WHITE_ROW);
    white.spans.insert(0, Span::raw("  "));
    let info = Line::from(Span::styled(
        format!("  a = {base}   z/x octave   c/v velocity   space all off   tab next patch"),
        Style::new().fg(DIM),
    ));
    frame.render_widget(
        Paragraph::new(vec![black, white, info]).block(
            Block::bordered()
                .title(" play ")
                .border_style(Style::new().fg(DIM)),
        ),
        area,
    );
}

fn draw_help(frame: &mut Frame) {
    let area = frame.area();
    let w = area.width.min(64);
    let h = area.height.min(22);
    let popup = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    let key = |k: &'static str, d: &'static str| {
        Line::from(vec![
            Span::styled(
                format!("{k:>14}  "),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw(d),
        ])
    };
    let lines = vec![
        Line::from(Span::styled(
            "kazoo-dx: four-operator FM",
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        key("a s d f g h j", "white keys (w e t y u: black)"),
        key("k l ;  o p", "next octave up"),
        key("z / x", "octave down / up"),
        key("c / v", "velocity down / up"),
        key("arrows", "move around operator grid and header"),
        key("- / =", "change the highlighted value"),
        key("shift - / =", "change in big steps"),
        key("tab / shift-tab", "next / previous patch"),
        key("1-6", "jump to patch"),
        key("space", "all notes off"),
        key(
            "n",
            "play/stop the --phrase (asks the desk when plugged in)",
        ),
        key("[ ]", "phrase tempo -1/+1 BPM ({ } for 10)"),
        key("q / esc", "quit"),
        Line::raw(""),
        Line::styled(
            "Green operators are heard. Blue operators modulate.",
            Style::new().fg(DIM),
        ),
        Line::styled(
            "OP4 feeds back into itself (FB in the header).",
            Style::new().fg(DIM),
        ),
        Line::styled(
            "With kazoo-tui running, audio routes to the hub",
            Style::new().fg(DIM),
        ),
        Line::styled(
            "and hub note events (kazoo-arp) play this synth.",
            Style::new().fg(DIM),
        ),
        Line::raw(""),
        Line::styled("any key to close", Style::new().fg(DIM)),
    ];
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" help ")
                .border_style(Style::new().fg(ACCENT)),
        ),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn status() -> LinkStatus {
        LinkStatus {
            connected: false,
            strip: None,
            blocks_sent: 0,
            blocks_dropped: 0,
            messages_dropped: 0,
            connections: 0,
            last_refusal: None,
        }
    }

    #[test]
    fn connected_link_names_the_strip() {
        let link = LinkStatus {
            connected: true,
            strip: Some(2),
            ..status()
        };
        assert!(text(&link_spans(&link)).contains("→ kazoo-mix strip 3"));
    }

    #[test]
    fn refusal_is_shown_while_standalone() {
        let link = LinkStatus {
            last_refusal: Some("desk is full".to_owned()),
            ..status()
        };
        assert!(text(&link_spans(&link)).contains("standalone: desk is full"));
        assert_eq!(text(&link_spans(&status())).trim(), "○ standalone");
    }

    #[test]
    fn lost_blocks_are_reported() {
        let link = LinkStatus {
            connected: true,
            blocks_dropped: 4,
            ..status()
        };
        assert!(text(&link_spans(&link)).contains("lost 4 blocks"));
    }
}
