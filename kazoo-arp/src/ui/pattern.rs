//! Step sequencer grid visualization — the visual centerpiece.
//!
//! Shows upcoming pattern steps as a row of cells. The current step
//! is highlighted in hot orange, upcoming steps fade from blue to dim.
//! Past notes scroll off to the left as dim shadows.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use kazoo_arp::ArpMode;

use crate::app::App;

/// Color constants (from palette).
const BLUE: Color = Color::Rgb(60, 140, 255);
const HOT: Color = Color::Rgb(255, 80, 50);
const DIM: Color = Color::Rgb(55, 60, 75);
const BORDER_ACTIVE: Color = Color::Rgb(60, 100, 200);

/// Number of steps to peek from the arpeggiator.
const GRID_STEPS: usize = 16;

/// Width of each step cell (border chars + note name + spacing).
const CELL_WIDTH: usize = 5;

/// Box-drawing characters for cell borders.
const TOP_LEFT: &str = "\u{250c}";
const TOP_RIGHT: &str = "\u{2510}";
const BOT_LEFT: &str = "\u{2514}";
const BOT_RIGHT: &str = "\u{2518}";
const HORIZ: &str = "\u{2500}";
const VERT: &str = "\u{2502}";

/// Computed layout for the step grid: how many cells of each type to show.
struct GridLayout {
    past: usize,
    current: usize,
    upcoming: usize,
}

/// Draw the step sequencer grid.
///
/// Shows: recent notes (dim, scrolling left) | current note (hot) | upcoming (blue gradient)
#[allow(clippy::too_many_lines)]
pub fn draw_step_grid(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::new().fg(BORDER_ACTIVE))
        .title(Span::styled(
            " SEQUENCE ",
            Style::new().fg(BLUE).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if !app.arp.has_notes() {
        let empty_lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Hold notes (Z X C V B N M) to start the arpeggio",
                Style::new().fg(DIM),
            )),
        ];
        frame.render_widget(Paragraph::new(empty_lines), inner);
        return;
    }

    let is_random = app.arp.mode == ArpMode::Random;
    let pattern_pos = app.last_pattern_position;
    let expanded_len = app.expanded_pool_len();

    let recent = app.recent_note_list();
    let upcoming = app.arp.peek_pattern(GRID_STEPS);
    let layout = compute_layout(
        inner.width as usize,
        app.last_note_on.is_some(),
        recent.len(),
        upcoming.len(),
    );

    let lines = vec![
        build_border_line(&layout, true),
        build_note_line(&layout, &recent, app.last_note_on, &upcoming, is_random),
        build_border_line(&layout, false),
        build_number_line(&layout, pattern_pos, expanded_len, app.arp.mode),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Compute how many past/current/upcoming cells fit in the available width.
fn compute_layout(
    avail_width: usize,
    has_current: bool,
    recent_count: usize,
    upcoming_count: usize,
) -> GridLayout {
    let prefix_width = 2; // left margin
    let max = avail_width.saturating_sub(prefix_width) / CELL_WIDTH;
    let current = usize::from(has_current);
    let upcoming = upcoming_count.min(max.saturating_sub(current));
    let past = recent_count.min(max.saturating_sub(current + upcoming));

    GridLayout {
        past,
        current,
        upcoming,
    }
}

/// Build a top or bottom border line for all cells.
fn build_border_line(layout: &GridLayout, top: bool) -> Line<'static> {
    let (left, right) = if top {
        (TOP_LEFT, TOP_RIGHT)
    } else {
        (BOT_LEFT, BOT_RIGHT)
    };
    let segment = format!(" {left}{HORIZ}{HORIZ}{right}");

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled("  ", Style::new().fg(DIM)));

    // Past cells (dim).
    for _ in 0..layout.past {
        spans.push(Span::styled(segment.clone(), Style::new().fg(DIM)));
    }
    // Current cell (hot).
    if layout.current > 0 {
        spans.push(Span::styled(segment.clone(), Style::new().fg(HOT)));
    }
    // Upcoming cells (blue gradient).
    for idx in 0..layout.upcoming {
        spans.push(Span::styled(
            segment.clone(),
            Style::new().fg(step_color(idx, layout.upcoming)),
        ));
    }

    Line::from(spans)
}

/// Build the middle line containing note names inside cell borders.
///
/// For Random mode, upcoming notes show "?" instead of fictional predictions.
fn build_note_line(
    layout: &GridLayout,
    recent: &[u8],
    current_note: Option<u8>,
    upcoming: &[(u8, u8)],
    is_random: bool,
) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled("  ", Style::new().fg(DIM)));

    // Past notes (dim).
    let past_start = recent.len().saturating_sub(layout.past);
    for &note in &recent[past_start..] {
        let name = kazoo_core::midi_note_name(note);
        spans.push(Span::styled(
            format!(" {VERT}{name:<3}{VERT}"),
            Style::new().fg(DIM),
        ));
    }

    // Current note (hot orange, bold).
    if let Some(note) = current_note {
        let name = kazoo_core::midi_note_name(note);
        spans.push(Span::styled(
            format!(" {VERT}{name:<3}{VERT}"),
            Style::new().fg(HOT).add_modifier(Modifier::BOLD),
        ));
    }

    // Upcoming notes (blue gradient). Random mode shows "?" symbols.
    for (idx, upcoming_note) in upcoming.iter().take(layout.upcoming).enumerate() {
        let cell_text = if is_random {
            format!(" {VERT}{:<3}{VERT}", "?")
        } else {
            let name = kazoo_core::midi_note_name(upcoming_note.0);
            format!(" {VERT}{name:<3}{VERT}")
        };
        spans.push(Span::styled(
            cell_text,
            Style::new().fg(step_color(idx, layout.upcoming)),
        ));
    }

    Line::from(spans)
}

/// Build the step number line below the cells.
///
/// Shows actual step index in the pattern (1-indexed), not always starting at 1.
/// For Random mode upcoming cells, shows "?" since order is non-deterministic.
fn build_number_line(
    layout: &GridLayout,
    pattern_pos: usize,
    expanded_len: usize,
    mode: ArpMode,
) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled("  ", Style::new().fg(DIM)));

    let has_current = layout.current > 0;
    let total_visible = layout.past + layout.current + layout.upcoming;
    let is_down = mode == ArpMode::Down;

    for cell_idx in 0..total_visible {
        let is_past = cell_idx < layout.past;
        let is_current = has_current && cell_idx == layout.past;

        let color = if is_current {
            HOT
        } else if is_past {
            DIM
        } else {
            let upcoming_idx = cell_idx - layout.past - layout.current;
            step_color(upcoming_idx, layout.upcoming)
        };

        let num_str = if expanded_len == 0 {
            "\u{00b7}".to_string() // middle dot
        } else if mode == ArpMode::Random && !is_past && !is_current {
            "?".to_string()
        } else {
            let pos = step_position(pattern_pos, expanded_len, cell_idx, layout.past, is_down);
            format!("{}", pos + 1)
        };

        spans.push(Span::styled(
            format!("  {num_str:<3}"),
            Style::new().fg(color),
        ));
    }

    Line::from(spans)
}

/// Compute the pattern position for a grid cell using only unsigned arithmetic.
///
/// - `pattern_pos`: the position of the current (most recently played) note.
/// - `expanded_len`: total steps in one pattern cycle.
/// - `cell_idx`: index of this cell in the visible grid.
/// - `current_cell`: index of the "current" cell in the visible grid.
/// - `is_down`: whether the arp traverses in reverse (Down mode).
const fn step_position(
    pattern_pos: usize,
    expanded_len: usize,
    cell_idx: usize,
    current_cell: usize,
    is_down: bool,
) -> usize {
    if cell_idx == current_cell {
        return pattern_pos % expanded_len;
    }

    if cell_idx < current_cell {
        // Past cell: steps_back positions before current.
        let steps_back = current_cell - cell_idx;
        if is_down {
            (pattern_pos + steps_back) % expanded_len
        } else {
            (pattern_pos + expanded_len - (steps_back % expanded_len)) % expanded_len
        }
    } else {
        // Upcoming cell: steps_fwd positions after current.
        let steps_fwd = cell_idx - current_cell;
        if is_down {
            (pattern_pos + expanded_len - (steps_fwd % expanded_len)) % expanded_len
        } else {
            (pattern_pos + steps_fwd) % expanded_len
        }
    }
}

/// Color for a step at the given position, fading from bright blue to dim.
fn step_color(pos: usize, total: usize) -> Color {
    if total <= 1 {
        return BLUE;
    }

    let fraction = pos as f32 / (total - 1) as f32;

    // Blue channel: 255 -> 70
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let blue = (255.0 - fraction * 185.0) as u8;
    // Green channel: 140 -> 60
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let green = (140.0 - fraction * 80.0) as u8;
    // Red channel: 60 -> 50
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let red = (60.0 - fraction * 10.0) as u8;

    Color::Rgb(red, green, blue)
}
