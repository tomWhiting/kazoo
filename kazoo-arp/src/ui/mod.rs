//! TUI rendering for the Jupiter-8 style arpeggiator.
//!
//! Layout:
//! ```text
//! +================== JUPITER-8 ARPEGGIATOR ==================+
//! |  120 BPM   1/8th   Swing 50%   Gate 75%   Oct 1   LATCH  |
//! +--- MODE ---------------------------------------------------+
//! |  1 ▲ Up   2 ▼ Down   3 ↕ Up/Down   4 ☆ Random   5 ▸ As-Played |
//! +--- SEQUENCE -----------------------------------------------+
//! |  ┌──┐ ┌──┐ ┌──┐ ┌──┐ ┌──┐ ┌──┐ ┌──┐ ┌──┐  ...          |
//! |  │C4│ │E4│ │G4│ │C5│ │E5│ │G5│ │C4│ │E4│                 |
//! |  └──┘ └──┘ └──┘ └──┘ └──┘ └──┘ └──┘ └──┘                 |
//! |   3    4    5    6    1    2    3    4                      |
//! +--- POOL ---------------------------------------------------+
//! |  C4  E4  G4  (3 notes × 2 oct = 6 steps)                  |
//! +------------------------------------------------------------+
//! | Z-M:notes  ←→:param  ↑↓:adjust  Space:latch  Esc:quit     |
//! | (alerts: lost audio, undelivered edits, desk drops)        |
//! +------------------------------------------------------------+
//! ```

mod controls;
mod pattern;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use kazoo_arp::ArpMode;

use crate::app::{App, KeyMode, Param};
use crate::audio::SendFailure;

// ---------------------------------------------------------------------------
// Jupiter-8 color palette — electric blue + hot orange, silver labels
// ---------------------------------------------------------------------------

/// Primary accent: electric blue (Jupiter-8 LED buttons).
const BLUE: Color = Color::Rgb(60, 140, 255);
/// Hot accent: active step / current note indicator.
const HOT: Color = Color::Rgb(255, 80, 50);
/// Silver: labels and secondary text.
const SILVER: Color = Color::Rgb(170, 180, 200);
/// Selected parameter background.
const SEL_BG: Color = Color::Rgb(20, 35, 65);
/// Dim: inactive/background elements.
const DIM: Color = Color::Rgb(55, 60, 75);
/// Active status (latch on, playing).
const GREEN: Color = Color::Rgb(80, 220, 100);
/// Warnings: something the user asked for did not fully happen.
const AMBER: Color = Color::Rgb(255, 176, 0);
/// Errors: the engine is not doing what the screen shows.
const ALERT: Color = Color::Rgb(255, 60, 60);
/// Border for active sections.
const BORDER_ACTIVE: Color = Color::Rgb(60, 100, 200);
/// Border for inactive sections.
const BORDER_DIM: Color = Color::Rgb(45, 48, 60);

/// Render the full arpeggiator TUI.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER_ACTIVE))
        .title(Span::styled(
            " JUPITER-8 ARPEGGIATOR ",
            Style::new().fg(BLUE).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(
            " kazoo-arp ",
            Style::new().fg(DIM),
        )));

    let inner = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Header: BPM + params
            Constraint::Length(3), // Mode selector
            Constraint::Min(7),    // Step grid (main visual)
            Constraint::Length(3), // Note pool
            Constraint::Length(3), // Help bar + alerts
        ])
        .split(inner);

    draw_header(frame, chunks[0], app);
    controls::draw_mode_selector(frame, chunks[1], app);
    pattern::draw_step_grid(frame, chunks[2], app);
    draw_note_pool(frame, chunks[3], app);
    draw_footer(frame, chunks[4], app);
}

// ---------------------------------------------------------------------------
// Header — BPM + parameter readouts + target display
// ---------------------------------------------------------------------------

fn draw_header(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let sel = app.selected_param;

    let bpm_style = param_value_style(sel, Param::Bpm);
    let div_style = param_value_style(sel, Param::Division);
    let swing_style = param_value_style(sel, Param::Swing);
    let gate_style = param_value_style(sel, Param::Gate);
    let oct_style = param_value_style(sel, Param::Octave);
    let latch_style = if app.arp.latch {
        Style::new()
            .fg(Color::Black)
            .bg(GREEN)
            .add_modifier(Modifier::BOLD)
    } else if sel == Param::Latch {
        Style::new()
            .fg(BLUE)
            .bg(SEL_BG)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(DIM)
    };

    let bpm_text = format!(" {} BPM", app.clock.bpm().round() as u16);
    let (transport, transport_style) = if app.playing {
        (
            format!(" \u{25b6} {} ", bar_beat(app.beat)),
            Style::new()
                .fg(Color::Black)
                .bg(GREEN)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (" \u{25a0} STOP ".to_string(), Style::new().fg(SILVER))
    };
    // Triplets play straight (see `ClockDivision::swings`): say so.
    let swing_text = if app.clock.division().swings() {
        swing_display(app.clock.swing())
    } else {
        "straight (triplets)".to_string()
    };

    let mut spans = vec![
        Span::styled(transport, transport_style),
        Span::styled(bpm_text, bpm_style),
        Span::styled("   ", Style::new().fg(DIM)),
        Span::styled("Div ", Style::new().fg(SILVER)),
        Span::styled(app.clock.division().label().to_string(), div_style),
        Span::styled("   ", Style::new().fg(DIM)),
        Span::styled("Swing ", Style::new().fg(SILVER)),
        Span::styled(swing_text, swing_style),
        Span::styled("   ", Style::new().fg(DIM)),
        Span::styled("Gate ", Style::new().fg(SILVER)),
        Span::styled(gate_display(app.arp.gate_pct), gate_style),
        Span::styled("   ", Style::new().fg(DIM)),
        Span::styled("Oct ", Style::new().fg(SILVER)),
        Span::styled(format!("{}", app.arp.octave_range), oct_style),
        Span::styled("   ", Style::new().fg(DIM)),
        Span::styled(
            if app.arp.latch { " LATCH " } else { " latch " },
            latch_style,
        ),
    ];

    // Target / desk link status.
    spans.push(Span::styled("   ", Style::new().fg(DIM)));
    spans.push(Span::styled("Target ", Style::new().fg(SILVER)));
    let strip = app
        .hub
        .as_ref()
        .filter(|hub| hub.connected)
        .and_then(|hub| hub.strip);
    if let Some(strip) = strip {
        spans.push(Span::styled(
            format!("\u{2192} kazoo-mix strip {}", u16::from(strip) + 1),
            Style::new().fg(GREEN).add_modifier(Modifier::BOLD),
        ));
    } else if app.hub_connected() {
        spans.push(Span::styled(
            "\u{2192} kazoo-mix",
            Style::new().fg(GREEN).add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled("\u{25cf} local (sine)", Style::new().fg(DIM)));
        if let Some(refusal) = app.hub.as_ref().and_then(|hub| hub.last_refusal.as_ref()) {
            spans.push(Span::styled(
                format!("  desk: {refusal}"),
                Style::new().fg(AMBER).add_modifier(Modifier::DIM),
            ));
        }
    }

    let paragraph = Paragraph::new(Line::from(spans));
    f.render_widget(paragraph, area);
}

// ---------------------------------------------------------------------------
// Note pool display
// ---------------------------------------------------------------------------

fn draw_note_pool(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::new().fg(BORDER_DIM))
        .title(Span::styled(
            " POOL ",
            Style::new().fg(SILVER).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Use the correct pool for the current mode (#5: AsPlayed uses insertion order).
    let pool = if app.arp.mode == ArpMode::AsPlayed {
        app.arp.insertion_order_pool()
    } else {
        app.arp.pitch_sorted_pool()
    };

    if pool.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  (no notes held \u{2014} play Z X C V B N M keys)",
            Style::new().fg(DIM),
        )));
        f.render_widget(empty, inner);
        return;
    }

    let mut spans: Vec<Span> = Vec::with_capacity(pool.len() * 2 + 2);
    spans.push(Span::styled("  ", Style::new().fg(DIM)));

    for (i, note) in pool.iter().enumerate() {
        let name = kazoo_core::midi_note_name(note.midi_note);
        let is_current = app.last_note_on == Some(note.midi_note);

        let style = if is_current {
            Style::new().fg(HOT).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(BLUE)
        };

        spans.push(Span::styled(name, style));
        if i < pool.len() - 1 {
            spans.push(Span::styled("  ", Style::new().fg(DIM)));
        }
    }

    // Show octave expansion info (#11) when octave_range > 1.
    let pool_len = pool.len();
    if app.arp.octave_range > 1 {
        let expanded = app.expanded_pool_len();
        spans.push(Span::styled(
            format!(
                "  ({pool_len} \u{00d7} {} oct = {expanded} steps)",
                app.arp.octave_range
            ),
            Style::new().fg(DIM),
        ));
    } else {
        spans.push(Span::styled(
            format!("  ({pool_len} held)"),
            Style::new().fg(DIM),
        ));
    }

    let paragraph = Paragraph::new(Line::from(spans));
    f.render_widget(paragraph, inner);
}

// ---------------------------------------------------------------------------
// Footer — keyboard help
// ---------------------------------------------------------------------------

fn draw_footer(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let piano_hint = match app.key_mode {
        KeyMode::Hold => "  (chromatic C4-C5)",
        KeyMode::Toggle => "  (chromatic C4-C5, tap to hold / tap again to release)",
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(" Piano: ", Style::new().fg(SILVER)),
            Span::styled("z s x d c v g b h n j m ,", Style::new().fg(Color::White)),
            Span::styled(piano_hint, Style::new().fg(DIM)),
        ]),
        Line::from(vec![
            Span::styled(" \u{2190}/\u{2192}", Style::new().fg(BLUE)),
            Span::styled(":param  ", Style::new().fg(DIM)),
            Span::styled("\u{2191}/\u{2193}", Style::new().fg(BLUE)),
            Span::styled(":adjust  ", Style::new().fg(DIM)),
            Span::styled("Shift+\u{2191}/\u{2193}", Style::new().fg(BLUE)),
            Span::styled(":BPM\u{00b1}10  ", Style::new().fg(DIM)),
            Span::styled("Space", Style::new().fg(BLUE)),
            Span::styled(":latch  ", Style::new().fg(DIM)),
            Span::styled("Enter", Style::new().fg(BLUE)),
            Span::styled(
                if app.hub_connected() {
                    ":play/stop studio  "
                } else {
                    ":play/stop  "
                },
                Style::new().fg(DIM),
            ),
            Span::styled("1-5", Style::new().fg(BLUE)),
            Span::styled(":mode  ", Style::new().fg(DIM)),
            Span::styled("Esc", Style::new().fg(BLUE)),
            Span::styled(":quit", Style::new().fg(DIM)),
        ]),
        alert_line(app),
    ];

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, area);
}

/// Everything that went wrong, so nothing fails out of sight: lost audio,
/// edits the engine never got, display events and desk blocks lost.
fn alert_line(app: &App) -> Line<'static> {
    let mut spans = Vec::with_capacity(6);

    if app.stream.lost {
        spans.push(Span::styled(
            " AUDIO DEVICE LOST - restart kazoo-arp",
            Style::new().fg(ALERT).add_modifier(Modifier::BOLD),
        ));
    } else if app.stream.errors > 0 {
        spans.push(Span::styled(
            format!(" {} audio stream errors", app.stream.errors),
            Style::new().fg(AMBER),
        ));
    }

    let failed = app.commands.failed();
    if failed > 0 {
        let why = match app.commands.last_failure() {
            Some(SendFailure::EngineGone) => "audio engine stopped",
            Some(SendFailure::QueueFull) | None => "audio engine busy",
        };
        spans.push(Span::styled(
            format!(" {failed} edits not applied ({why})"),
            Style::new().fg(ALERT),
        ));
    }

    let pending = app.pending_note_offs();
    if pending > 0 {
        spans.push(Span::styled(
            format!(" {pending} note releases waiting"),
            Style::new().fg(AMBER),
        ));
    }

    if app.stream.display_dropped > 0 {
        spans.push(Span::styled(
            format!(" {} display updates missed", app.stream.display_dropped),
            Style::new().fg(AMBER),
        ));
    }

    if app.desk_lost > 0 {
        spans.push(Span::styled(
            format!(" {} transport messages lost to/from desk", app.desk_lost),
            Style::new().fg(ALERT),
        ));
    }

    if let Some(hub) = app.hub.as_ref().filter(|hub| hub.messages_dropped > 0) {
        spans.push(Span::styled(
            format!(" {} desk messages dropped", hub.messages_dropped),
            Style::new().fg(AMBER),
        ));
    }

    if let Some(hub) = app.hub.as_ref().filter(|hub| hub.blocks_dropped > 0) {
        spans.push(Span::styled(
            format!(" {} blocks lost to desk", hub.blocks_dropped),
            Style::new().fg(AMBER),
        ));
    }

    if let Some(reason) = &app.key_mode_reason {
        spans.push(Span::styled(
            format!(" {reason}"),
            Style::new().fg(AMBER).add_modifier(Modifier::DIM),
        ));
    }

    Line::from(spans)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Song position as bar.beat (4/4, both from 1), for the transport
/// readout. Before the song starts (a count-in) it reads 0.0.
pub fn bar_beat(beat: f64) -> String {
    if !beat.is_finite() || beat < 0.0 {
        return "0.0".to_string();
    }
    // Song positions are small and non-negative: the conversion is exact.
    let whole = beat.floor() as u64;
    format!("{}.{}", whole / 4 + 1, whole % 4 + 1)
}

/// Formatted swing percentage for display (rounds instead of truncating).
pub fn swing_display(swing: f64) -> String {
    let pct = (swing * 100.0).round().clamp(0.0, 100.0) as u8;
    format!("{pct}%")
}

/// Formatted gate percentage for display (rounds instead of truncating).
pub fn gate_display(gate: f32) -> String {
    let pct = (gate * 100.0).round().clamp(0.0, 100.0) as u8;
    format!("{pct}%")
}

/// Style for a parameter value based on whether it's selected.
pub fn param_value_style(selected: Param, current: Param) -> Style {
    if selected == current {
        Style::new()
            .fg(BLUE)
            .bg(SEL_BG)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::White)
    }
}
