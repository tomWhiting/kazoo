//! Ratatui front panel for kazoo-string.
//!
//! Everything is edited in place: the slider column *is* the control surface,
//! so there is no separate drawer duplicating what is on screen.

use std::sync::atomic::Ordering;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crate::patch::{PATCHES, ParamField};
use crate::synth::note_name;
use kazoo_core::ipc::link::LinkStatus;

use crate::{App, PhraseState, ROWS, VOLUME_ROW, piano_offset};

const ACCENT: Color = Color::Rgb(255, 176, 0);
const TONE: Color = Color::Rgb(90, 220, 170);
const WOOD: Color = Color::Rgb(205, 140, 80);
const DIM: Color = Color::DarkGray;
/// Cells in a slider bar.
const BAR_CELLS: usize = 26;

pub fn draw(frame: &mut Frame, app: &App) {
    let [header, body, keys, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(14),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, app);

    let left_width = 64.min(body.width * 3 / 5).max(body.width.min(40));
    let [left, right] =
        Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(body);
    let [panel, pluck] =
        Layout::vertical([Constraint::Length(ROWS as u16 + 4), Constraint::Min(5)]).areas(left);
    let [scope, voices] =
        Layout::vertical([Constraint::Min(6), Constraint::Length(4)]).areas(right);

    draw_panel(frame, panel, app);
    draw_pluck(frame, pluck, app);
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
            "AUDIO DEVICE LOST - restart kazoo-string  ",
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
        PhraseState::Looping => ("  ▶ phrase", Style::new().fg(TONE)),
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
            Style::new().fg(TONE),
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
    let mut spans = vec![
        Span::styled(
            " KAZOO-STRING ",
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
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .block(Block::bordered().border_style(Style::new().fg(DIM))),
        area,
    );
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
        TONE
    };
    Span::styled(
        format!("{}{} {db:>4.0}dB", "█".repeat(lit), "·".repeat(WIDTH - lit)),
        Style::new().fg(color),
    )
}

/// The text beside a slider: the value in the unit a player thinks in.
fn value_text(app: &App, row: usize) -> String {
    if row == VOLUME_ROW {
        return format!("{:.0}%", app.master * 100.0);
    }
    let field = ParamField::ALL[row];
    match field {
        ParamField::Decay => seconds(app.patch.decay_seconds()),
        ParamField::Release => seconds(app.patch.release_seconds()),
        _ => format!("{:.0}%", app.patch.get(field) * 100.0),
    }
}

/// A slider bar for a value in 0..=1.
fn bar(value: f32) -> String {
    let value = if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let lit = ((value * BAR_CELLS as f32).round() as usize).min(BAR_CELLS);
    format!("{}{}", "█".repeat(lit), "░".repeat(BAR_CELLS - lit))
}

fn draw_panel(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = Vec::with_capacity(ROWS + 2);
    for row in 0..ROWS {
        let (label, value) = if row == VOLUME_ROW {
            ("volume", app.master)
        } else {
            let field = ParamField::ALL[row];
            (field.label(), app.patch.get(field))
        };
        let selected = app.focus == row;
        let (label_style, bar_style) = if selected {
            (
                Style::new()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
                Style::new().fg(ACCENT),
            )
        } else if row == VOLUME_ROW {
            (Style::new().fg(Color::White), Style::new().fg(DIM))
        } else {
            (Style::new().fg(Color::White), Style::new().fg(WOOD))
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {label:<14}"), label_style),
            Span::raw(" "),
            Span::styled(bar(value), bar_style),
            Span::styled(
                format!(" {:>7}", value_text(app, row)),
                label_style_for(selected),
            ),
        ]));
    }
    let hint = if app.focus == VOLUME_ROW {
        "the level sent to the speakers and to the desk strip"
    } else {
        ParamField::ALL[app.focus].hint()
    };
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        format!(" {hint}"),
        Style::new().fg(DIM),
    )));
    lines.push(Line::from(Span::styled(
        " up/down select  ·  left/right or - / = change  ·  shift for big steps",
        Style::new().fg(DIM),
    )));
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" string ")
                .border_style(Style::new().fg(DIM)),
        ),
        area,
    );
}

const fn label_style_for(selected: bool) -> Style {
    if selected {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::White)
    }
}

fn seconds(s: f32) -> String {
    if s < 1.0 {
        format!("{:.0}ms", s * 1_000.0)
    } else {
        format!("{s:.1}s")
    }
}

/// The string at the moment of the pluck: pulled aside at the pick position,
/// so the picture shows what the position control does. A stiffer wire is
/// drawn with a squarer bend.
fn draw_pluck(frame: &mut Frame, area: Rect, app: &App) {
    let position = f64::from(app.patch.pick_position());
    let hardness = f64::from(app.patch.hardness.clamp(0.0, 1.0));
    let stiffness = f64::from(app.patch.stiffness.clamp(0.0, 1.0));
    let canvas = Canvas::default()
        .block(
            Block::bordered()
                .title(format!(" pluck at {:.0}% of the string ", position * 100.0))
                .border_style(Style::new().fg(DIM)),
        )
        .x_bounds([0.0, 1.0])
        .y_bounds([-0.2, 1.0])
        .paint(move |ctx| {
            // A sharp pick makes a sharp corner; a soft finger or stiff wire cuts
            // the corner off, so the two points either side of the apex are joined
            // directly.
            let round = (1.0 - hardness).mul_add(0.12, stiffness * 0.05);
            let points = [
                (0.0, 0.0),
                (
                    (position - round).max(0.0),
                    0.85 * (1.0 - round / position.max(0.02)),
                ),
                (
                    (position + round).min(1.0),
                    0.85 * (1.0 - round / (1.0 - position).max(0.02)),
                ),
                (1.0, 0.0),
            ];
            for pair in points.windows(2) {
                ctx.draw(&CanvasLine {
                    x1: pair[0].0,
                    y1: pair[0].1,
                    x2: pair[1].0,
                    y2: pair[1].1,
                    color: TONE,
                });
            }
            ctx.draw(&CanvasLine {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 0.0,
                color: DIM,
            });
            ctx.print(0.0, -0.12, "nut");
            ctx.print(0.94, -0.12, "bridge");
        });
    frame.render_widget(canvas, area);
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
                    color: TONE,
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
                    .bg(TONE)
                    .add_modifier(Modifier::BOLD),
            ),
            Some((note, false)) => {
                Span::styled(format!(" {:<4}", note_name(*note)), Style::new().fg(TONE))
            }
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
    let w = area.width.min(66);
    let h = area.height.min(24);
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
            "kazoo-string: plucked-string physical model",
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        key("a s d f g h j", "white keys (w e t y u: black)"),
        key("k l ;  o p", "next octave up"),
        key("z / x", "octave down / up"),
        key("c / v", "velocity down / up"),
        key("up / down", "select a control"),
        key("left/right, - =", "change it (shift: big steps)"),
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
            "Each note is a string: a delay line fed back through a",
            Style::new().fg(DIM),
        ),
        Line::styled(
            "treble-losing filter. Hit harder for a brighter pluck;",
            Style::new().fg(DIM),
        ),
        Line::styled(
            "let go and the string is stopped at the release time.",
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
