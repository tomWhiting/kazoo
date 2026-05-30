//! Waveform monitor rendering — bipolar audio display centered on zero.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::{BORDER_INACTIVE, DIM, PANEL, WAVE_RED};
use crate::app::App;

// Braille character mapping for bipolar waveform display.
// Each braille cell is 2 dots wide × 4 dots tall (8 dots total).
// Unicode braille starts at U+2800, with dot positions encoded as bits:
// Dot 1 (0x01) = top-left,     Dot 4 (0x08) = top-right
// Dot 2 (0x02) = mid-upper-L,  Dot 5 (0x10) = mid-upper-R
// Dot 3 (0x04) = mid-lower-L,  Dot 6 (0x20) = mid-lower-R
// Dot 7 (0x40) = bottom-left,  Dot 8 (0x80) = bottom-right

/// Map a y-position (0=top, 3=bottom) to the left-column braille dot bit.
const fn left_dot(row: u8) -> u8 {
    match row {
        0 => 0x01, // Dot 1
        1 => 0x02, // Dot 2
        2 => 0x04, // Dot 3
        3 => 0x40, // Dot 7
        _ => 0,
    }
}

/// Map a y-position (0=top, 3=bottom) to the right-column braille dot bit.
const fn right_dot(row: u8) -> u8 {
    match row {
        0 => 0x08, // Dot 4
        1 => 0x10, // Dot 5
        2 => 0x20, // Dot 6
        3 => 0x80, // Dot 8
        _ => 0,
    }
}

/// Draw the waveform monitor using braille characters for bipolar display.
///
/// Silence renders as empty space (not half-height bars). The waveform is
/// centered on zero with positive values above center and negative below.
pub fn draw_waveform(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER_INACTIVE))
        .title(Span::styled(" OUTPUT ", Style::new().fg(DIM)))
        .style(Style::new().bg(PANEL));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Linearize the ring buffer: read from write_pos to end, then start to write_pos.
    let raw_samples = app.voice.display_samples();
    let write_pos = app.voice.display_write_pos();
    let len = raw_samples.len();
    let mut linearized: Vec<f32> = Vec::with_capacity(len);
    if len > 0 {
        linearized.extend_from_slice(&raw_samples[write_pos..]);
        linearized.extend_from_slice(&raw_samples[..write_pos]);
    }

    // Each braille character represents 2 horizontal samples and 4 vertical dots.
    // Total vertical resolution = inner.height rows × 4 dots per row.
    let char_width = inner.width as usize;
    let samples_per_char = 2;
    let total_samples_needed = char_width * samples_per_char;

    // Downsample or select from linearized buffer.
    let step = if linearized.len() > total_samples_needed && total_samples_needed > 0 {
        linearized.len() / total_samples_needed
    } else {
        1
    };

    let height_rows = inner.height as usize;
    let total_dots_y = height_rows * 4; // vertical resolution in dots
    let center_dot = total_dots_y / 2; // zero crossing in dot space

    // Build braille grid in column-major order: grid[col][row].
    // Column-major allows efficient per-column iteration for sample processing.
    let mut grid = vec![vec![0u8; height_rows]; char_width];

    for (col, grid_column) in grid.iter_mut().enumerate() {
        for sub in 0..2u8 {
            let sample_idx = (col * samples_per_char + sub as usize) * step;
            let sample = linearized.get(sample_idx).copied().unwrap_or(0.0);

            // Map sample [-1.0, 1.0] to dot row [0, total_dots_y-1].
            // +1.0 -> top (row 0), 0.0 -> center, -1.0 -> bottom.
            let clamped = sample.clamp(-1.0, 1.0);
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let dot_y_f = ((-clamped + 1.0) * 0.5) * (total_dots_y.saturating_sub(1)) as f32;
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            let dot_y = (dot_y_f as usize).min(total_dots_y.saturating_sub(1));

            // Which grid row and which dot within that row?
            let grid_row = dot_y / 4;
            let dot_within = (dot_y % 4) as u8;

            if grid_row < height_rows {
                let bit = if sub == 0 {
                    left_dot(dot_within)
                } else {
                    right_dot(dot_within)
                };
                grid_column[grid_row] |= bit;
            }
        }
    }

    // Also draw the zero line faintly: for each column, set the center dot.
    // Skip if a sample dot is already at center (signal is more important).
    let zero_grid_row = center_dot / 4;
    let zero_dot_within = (center_dot % 4) as u8;
    if zero_grid_row < height_rows {
        for grid_column in &mut grid {
            if grid_column[zero_grid_row] == 0 {
                grid_column[zero_grid_row] |= left_dot(zero_dot_within);
            }
        }
    }

    // Convert column-major grid to row-major lines for rendering.
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(height_rows);
    for row in 0..height_rows {
        let s: String = grid
            .iter()
            .map(|col| {
                let bits = col[row];
                if bits == 0 {
                    ' '
                } else {
                    char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ')
                }
            })
            .collect();
        lines.push(Line::from(Span::styled(s, Style::new().fg(WAVE_RED))));
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

/// Draw the mixer section (5 vertical sliders).
pub fn draw_mixer(f: &mut Frame, area: Rect, app: &App) {
    let is_active = app.section == crate::app::Section::Mixer;
    let border_color = if is_active {
        super::BORDER_ACTIVE
    } else {
        super::BORDER_INACTIVE
    };

    let block = Block::default()
        .title(Span::styled(
            " MIXER ",
            Style::new()
                .fg(if is_active {
                    super::MOOG_RED
                } else {
                    super::CREAM
                })
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::new().fg(border_color))
        .style(Style::new().bg(super::PANEL));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let levels = [
        ("Osc 1", app.voice.mixer.osc1_level),
        ("Osc 2", app.voice.mixer.osc2_level),
        ("Osc 3", app.voice.mixer.osc3_level),
        ("Noise", app.voice.mixer.noise_level),
        ("Ext", app.voice.mixer.ext_level),
    ];

    let mut lines: Vec<Line<'_>> = Vec::new();
    for (i, (name, level)) in levels.iter().enumerate() {
        let selected = is_active && app.param_index == i;
        lines.push(super::vertical_slider_line(name, *level, selected));
        // Add spacing between sliders.
        if i < levels.len() - 1 {
            lines.push(Line::from(""));
        }
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}
