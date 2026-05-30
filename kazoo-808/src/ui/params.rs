//! Voice parameter editor widget.
//!
//! Shows the available parameters for the currently selected voice
//! with their current values and a visual bar reflecting the actual
//! parameter state from the synth.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;

use crate::app::{App, Focus};
use crate::synth::VoiceParam;

/// Voice parameter panel widget.
pub struct ParamsWidget<'a> {
    app: &'a App,
}

impl<'a> ParamsWidget<'a> {
    #[must_use]
    pub const fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for ParamsWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let voice = self.app.selected_voice_index();
        let params = VoiceParam::for_voice(voice);
        let is_focused = self.app.focus == Focus::Params;

        // Header: voice name.
        let header = format!(" {} Parameters ", voice.display_name());
        let header_style = if is_focused {
            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        for (i, ch) in header.chars().enumerate() {
            let x = area.x + i as u16;
            if x < area.x + area.width {
                buf[(x, area.y)].set_char(ch).set_style(header_style);
            }
        }

        // Parameter rows.
        for (idx, param) in params.iter().enumerate() {
            let y = area.y + 1 + idx as u16;
            if y >= area.y + area.height {
                break;
            }

            let is_selected = is_focused && idx == self.app.selected_param;
            let label = param.label();

            let label_style = if is_selected {
                Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else if is_focused {
                Style::new().fg(Color::White)
            } else {
                Style::new().fg(Color::DarkGray)
            };

            // Cursor indicator.
            let cursor_ch = if is_selected { '>' } else { ' ' };
            if area.x < area.x + area.width {
                buf[(area.x, y)].set_char(cursor_ch).set_style(label_style);
            }

            // Label.
            let label_text = format!(" {label:<9}");
            for (i, ch) in label_text.chars().enumerate() {
                let x = area.x + 1 + i as u16;
                if x < area.x + area.width {
                    buf[(x, y)].set_char(ch).set_style(label_style);
                }
            }

            // Value bar — reflects actual parameter state.
            let bar_start = area.x + 12;
            let bar_width = area.width.saturating_sub(18).min(20);
            let value_x = bar_start + bar_width + 1;

            if bar_width > 0 && bar_start < area.x + area.width {
                let normalized = self.app.param_normalized(self.app.selected_voice, idx);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let filled = (normalized * f32::from(bar_width)).round() as u16;

                let bar_fg = if is_selected {
                    Color::Cyan
                } else if is_focused {
                    Color::White
                } else {
                    Color::DarkGray
                };

                for i in 0..bar_width {
                    let x = bar_start + i;
                    if x < area.x + area.width {
                        let (ch, style) = if i < filled {
                            ('█', Style::new().fg(bar_fg))
                        } else {
                            ('░', Style::new().fg(Color::DarkGray))
                        };
                        buf[(x, y)].set_char(ch).set_style(style);
                    }
                }

                // Percentage value.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let pct = (normalized * 100.0).round() as u32;
                let pct_str = format!("{pct:>3}%");
                let pct_style = if is_selected {
                    Style::new().fg(Color::Cyan)
                } else {
                    Style::new().fg(Color::DarkGray)
                };
                for (i, ch) in pct_str.chars().enumerate() {
                    let x = value_x + i as u16;
                    if x < area.x + area.width {
                        buf[(x, y)].set_char(ch).set_style(pct_style);
                    }
                }
            }
        }
    }
}
