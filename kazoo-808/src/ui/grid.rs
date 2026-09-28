//! Step sequencer grid renderer.
//!
//! Renders the 10-row x 16-step grid with cursor, active steps,
//! accents, and playback position indicator. Vertical separators
//! every 4 steps visually group beats.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use crate::app::{App, Focus};
use kazoo_808::sequencer::{STEPS_PER_PATTERN, Step};
use kazoo_808::synth::{VOICE_COUNT, VoiceIndex};

/// Width of the voice label column.
const LABEL_WIDTH: u16 = 5;
/// Width of each step cell (including border chars).
const CELL_WIDTH: u16 = 3;
/// Extra pixel for the beat separator after every 4 steps.
const BEAT_SEPARATOR_WIDTH: u16 = 1;
/// Number of steps per beat group.
const STEPS_PER_BEAT: usize = 4;

/// Calculate the x offset for a step, accounting for beat group separators.
const fn step_x_offset(step: usize) -> u16 {
    let base = step as u16 * CELL_WIDTH;
    let separators = (step / STEPS_PER_BEAT) as u16;
    base + separators * BEAT_SEPARATOR_WIDTH
}

/// The step sequencer grid widget.
pub struct GridWidget<'a> {
    app: &'a App,
}

impl<'a> GridWidget<'a> {
    #[must_use]
    pub const fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl GridWidget<'_> {
    /// Draw one voice row: label, 16 step cells and beat separators.
    fn render_row(&self, voice_row: usize, voice_idx: VoiceIndex, area: Rect, buf: &mut Buffer) {
        let y = area.y + voice_row as u16;
        let is_selected_row = voice_row == self.app.selected_voice;
        let pattern = self.app.sequencer.current_pattern_ref();

        // Voice label.
        let label_style = if is_selected_row {
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        for (i, ch) in voice_idx.short_label().chars().enumerate() {
            let x = area.x + i as u16;
            if x < area.x + LABEL_WIDTH {
                buf[(x, y)].set_char(ch).set_style(label_style);
            }
        }

        // Step cells.
        for step in 0..STEPS_PER_PATTERN {
            let x = area.x + LABEL_WIDTH + step_x_offset(step);
            if x + CELL_WIDTH > area.x + area.width {
                break;
            }
            let is_cursor =
                is_selected_row && step == self.app.cursor_step && self.app.focus == Focus::Grid;
            let is_playhead = self.app.sequencer.playing && step == self.app.playback_step;
            let (content, style) =
                step_cell(pattern.steps[voice_row][step], is_cursor, is_playhead);

            // Render: [X] or [.] or [A]
            buf[(x, y)]
                .set_char('[')
                .set_style(Style::new().fg(Color::DarkGray));
            buf[(x + 1, y)].set_char(content).set_style(style);
            buf[(x + 2, y)]
                .set_char(']')
                .set_style(Style::new().fg(Color::DarkGray));
        }

        render_beat_separators(area, y, buf);
    }
}

/// Character and style for one step cell.
const fn step_cell(step: Step, is_cursor: bool, is_playhead: bool) -> (char, Style) {
    let bg = if is_cursor {
        Color::Yellow
    } else if is_playhead {
        Color::DarkGray
    } else {
        Color::Reset
    };
    let (ch, fg) = if step.active {
        let ch = if step.accent { 'A' } else { 'X' };
        let fg = if is_cursor {
            Color::Black
        } else if is_playhead {
            Color::White
        } else if step.accent {
            Color::Red
        } else {
            Color::Cyan
        };
        (ch, fg)
    } else {
        // Inactive step — show accent marker if somehow set.
        let ch = if step.accent {
            'a' // Distinct marker: accented but inactive.
        } else if is_playhead {
            '|'
        } else {
            '.'
        };
        let fg = if is_cursor {
            Color::Black
        } else if is_playhead && !step.accent {
            Color::White
        } else {
            // Dim accent marker on inactive step, or a plain empty step.
            Color::DarkGray
        };
        (ch, fg)
    };
    (ch, Style::new().fg(fg).bg(bg))
}

/// Thin vertical bars between beat groups on row `y`.
fn render_beat_separators(area: Rect, y: u16, buf: &mut Buffer) {
    for beat in 1..4 {
        let sep_step = beat * STEPS_PER_BEAT;
        let sep_x = area.x + LABEL_WIDTH + step_x_offset(sep_step) - BEAT_SEPARATOR_WIDTH;
        if sep_x < area.x + area.width {
            buf[(sep_x, y)]
                .set_char('│')
                .set_style(Style::new().fg(Color::DarkGray));
        }
    }
}

/// Step numbers (1-16) under the grid on row `y`.
fn render_step_numbers(area: Rect, y: u16, buf: &mut Buffer) {
    let style = Style::new().fg(Color::DarkGray);
    for step in 0..STEPS_PER_PATTERN {
        let base_x = area.x + LABEL_WIDTH + step_x_offset(step);
        let step_num = step + 1;
        if step_num < 10 {
            // Single digit: center in cell.
            let x = base_x + 1;
            if x < area.x + area.width {
                buf[(x, y)]
                    .set_char(char::from(b'0' + step_num as u8))
                    .set_style(style);
            }
        } else {
            // Two digits: left-aligned in cell.
            let tens = char::from(b'0' + (step_num / 10) as u8);
            let ones = char::from(b'0' + (step_num % 10) as u8);
            if base_x < area.x + area.width {
                buf[(base_x, y)].set_char(tens).set_style(style);
            }
            if base_x + 1 < area.x + area.width {
                buf[(base_x + 1, y)].set_char(ones).set_style(style);
            }
        }
    }
    render_beat_separators(area, y, buf);
}

impl Widget for GridWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (voice_row, voice_idx) in VoiceIndex::ALL.iter().enumerate() {
            if voice_row as u16 >= area.height {
                break;
            }
            self.render_row(voice_row, *voice_idx, area, buf);
        }

        // Step numbers along the bottom if there's room.
        let numbers_y = area.y + VOICE_COUNT as u16;
        if numbers_y < area.y + area.height {
            render_step_numbers(area, numbers_y, buf);
        }
    }
}
