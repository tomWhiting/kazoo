//! Step sequencer grid renderer.
//!
//! Renders the 10-row x 16-step grid with cursor, active steps,
//! accents, and playback position indicator. Vertical separators
//! every 4 steps visually group beats.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use crate::app::App;
use crate::sequencer::STEPS_PER_PATTERN;
use crate::synth::{VOICE_COUNT, VoiceIndex};

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

impl Widget for GridWidget<'_> {
    #[allow(clippy::too_many_lines)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        let pattern = self.app.sequencer.current_pattern_ref();
        let playing = self.app.sequencer.playing;
        let playback_step = self.app.playback_step;

        for (voice_row, voice_idx) in VoiceIndex::ALL.iter().enumerate() {
            let y = area.y + voice_row as u16;
            if y >= area.y + area.height {
                break;
            }

            let is_selected_row = voice_row == self.app.selected_voice;

            // Voice label.
            let label = voice_idx.short_label();
            let label_style = if is_selected_row {
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::DarkGray)
            };
            let label_x = area.x;
            for (i, ch) in label.chars().enumerate() {
                let x = label_x + i as u16;
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

                let step_data = &pattern.steps[voice_row][step];
                let is_cursor = is_selected_row
                    && step == self.app.cursor_step
                    && self.app.focus == crate::app::Focus::Grid;
                let is_playhead = playing && step == playback_step;

                // Determine cell content and style.
                let (content, style) = if step_data.active {
                    let ch = if step_data.accent { 'A' } else { 'X' };
                    let fg = if is_cursor {
                        Color::Black
                    } else if is_playhead {
                        Color::White
                    } else if step_data.accent {
                        Color::Red
                    } else {
                        Color::Cyan
                    };
                    let bg = if is_cursor {
                        Color::Yellow
                    } else if is_playhead {
                        Color::DarkGray
                    } else {
                        Color::Reset
                    };
                    (ch, Style::new().fg(fg).bg(bg))
                } else {
                    // Inactive step — show accent marker if somehow set.
                    let ch = if step_data.accent {
                        'a' // Distinct marker: accented but inactive.
                    } else if is_playhead {
                        '|'
                    } else {
                        '.'
                    };
                    let fg = if is_cursor {
                        Color::Black
                    } else if step_data.accent {
                        Color::DarkGray // Dim accent marker on inactive step.
                    } else if is_playhead {
                        Color::White
                    } else {
                        Color::DarkGray
                    };
                    let bg = if is_cursor {
                        Color::Yellow
                    } else if is_playhead {
                        Color::DarkGray
                    } else {
                        Color::Reset
                    };
                    (ch, Style::new().fg(fg).bg(bg))
                };

                // Render: [X] or [.] or [A]
                buf[(x, y)]
                    .set_char('[')
                    .set_style(Style::new().fg(Color::DarkGray));
                buf[(x + 1, y)].set_char(content).set_style(style);
                buf[(x + 2, y)]
                    .set_char(']')
                    .set_style(Style::new().fg(Color::DarkGray));
            }

            // Beat group separators (thin vertical bars between groups).
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

        // Step numbers along the bottom if there's room.
        let numbers_y = area.y + VOICE_COUNT as u16;
        if numbers_y < area.y + area.height {
            for step in 0..STEPS_PER_PATTERN {
                let base_x = area.x + LABEL_WIDTH + step_x_offset(step);
                let step_num = step + 1;
                let style = Style::new().fg(Color::DarkGray);

                if step_num < 10 {
                    // Single digit: center in cell.
                    let x = base_x + 1;
                    if x < area.x + area.width {
                        #[allow(clippy::cast_possible_truncation)]
                        buf[(x, numbers_y)]
                            .set_char(char::from(b'0' + step_num as u8))
                            .set_style(style);
                    }
                } else {
                    // Two digits: left-aligned in cell.
                    #[allow(clippy::cast_possible_truncation)]
                    let tens = char::from(b'0' + (step_num / 10) as u8);
                    #[allow(clippy::cast_possible_truncation)]
                    let ones = char::from(b'0' + (step_num % 10) as u8);
                    if base_x < area.x + area.width {
                        buf[(base_x, numbers_y)].set_char(tens).set_style(style);
                    }
                    if base_x + 1 < area.x + area.width {
                        buf[(base_x + 1, numbers_y)].set_char(ones).set_style(style);
                    }
                }
            }

            // Beat separators on the numbers row too.
            for beat in 1..4 {
                let sep_step = beat * STEPS_PER_BEAT;
                let sep_x = area.x + LABEL_WIDTH + step_x_offset(sep_step) - BEAT_SEPARATOR_WIDTH;
                if sep_x < area.x + area.width {
                    buf[(sep_x, numbers_y)]
                        .set_char('│')
                        .set_style(Style::new().fg(Color::DarkGray));
                }
            }
        }
    }
}
