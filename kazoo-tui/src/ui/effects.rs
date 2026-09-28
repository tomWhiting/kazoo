//! Effects chain inspector panel.
//!
//! Shows the synth and effects chain for the currently selected track in a
//! 36-column inspector area on the right side of the UI. The panel is divided
//! into four sections:
//!
//! 1. **Track header** -- name, synthesis mode, M/S/R indicators.
//! 2. **Item list** -- synth entry followed by effects with bypass state.
//! 3. **Parameter section** -- parameters of the selected synth or effect.
//! 4. **Hint bar** -- keyboard shortcut hints.

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::{App, FocusedPanel, InputMode, TrackInfo};
use crate::theme;
use kazoo_core::synthesis::SynthesisMode;

/// Draw the effects inspector panel into the given area.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = super::panel_block(" Effects ", FocusedPanel::Effects, app);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Check if we have a selected track.
    let Some(track) = app.selected_track_info() else {
        let empty = Paragraph::new("  No track selected").style(theme::style_text_dimmed());
        frame.render_widget(empty, inner);
        return;
    };

    // Split inner into: header (2), item list (variable), params (rest), hint (1).
    let sections = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(item_list_height(track)),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(inner);

    draw_track_header(frame, app, track, sections[0]);
    draw_item_list(frame, app, track, sections[1]);
    draw_param_section(frame, app, track, sections[2]);
    draw_hint_bar(frame, app, sections[3]);
}

/// Calculate the height needed for the item list (synth + effects).
fn item_list_height(track: &TrackInfo) -> u16 {
    // 1 for synth entry + effect count, min 2 to avoid collapse.
    let count = 1 + track.effects.len();
    (count as u16).clamp(2, 6)
}

/// Render the track header: name, synthesis mode abbreviation, and M/S/R flags.
fn draw_track_header(frame: &mut Frame, app: &App, track: &TrackInfo, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let idx = app.selected_track;
    let name_style = Style::new().fg(theme::track_color(idx));
    let mode_str = match track.synthesis_mode {
        SynthesisMode::Passthrough => "Raw",
        SynthesisMode::PitchTracked => "Pitch",
        SynthesisMode::Wavetable => "Wave",
        SynthesisMode::Granular => "Gran",
        SynthesisMode::Vocoder => "Voc",
        SynthesisMode::PhaseVocoder => "PhVoc",
    };

    let mut indicators: Vec<Span<'_>> = Vec::new();
    if track.muted {
        indicators.push(Span::styled(" M", theme::style_muted()));
    }
    if track.soloed {
        indicators.push(Span::styled(" S", theme::style_soloed()));
    }
    if track.armed {
        indicators.push(Span::styled(" R", theme::style_armed()));
    }

    let mut spans: Vec<Span<'_>> = vec![
        Span::styled(&track.name, name_style),
        Span::styled(format!(" [{mode_str}]"), theme::style_text_secondary()),
    ];
    spans.extend(indicators);

    let header = Paragraph::new(Line::from(spans));
    frame.render_widget(header, area);
}

/// Render the unified item list: synth entry at top, then effects.
fn draw_item_list(frame: &mut Frame, app: &App, track: &TrackInfo, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let focused = app.is_focused(FocusedPanel::Effects);
    let mut lines: Vec<Line<'_>> = Vec::new();

    // Synth entry.
    let synth_selected = app.synth_state.synth_selected && focused;
    let synth_style = if synth_selected {
        theme::style_selected()
    } else {
        theme::style_text()
    };
    let marker = if app.synth_state.synth_selected {
        "\u{25b6}"
    } else {
        " "
    };
    lines.push(Line::from(vec![
        Span::styled(marker, synth_style),
        Span::raw(" "),
        Span::styled(track.synthesis_mode.display_name(), synth_style),
    ]));

    // Effect entries.
    for (i, effect) in track.effects.iter().enumerate() {
        let bypassed = effect.bypassed;
        let selected = !app.synth_state.synth_selected && i == app.synth_state.selected_effect;

        let bypass_indicator = if bypassed { "\u{25cb}" } else { "\u{25cf}" };
        let bypass_color = if bypassed {
            theme::FG_DIMMED
        } else {
            theme::ACCENT_PLAY
        };

        let name_style = if selected && focused {
            theme::style_selected()
        } else if bypassed {
            theme::style_text_dimmed()
        } else {
            theme::style_text()
        };

        let marker = if selected { "\u{25b6}" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(marker, name_style),
            Span::styled(bypass_indicator, Style::new().fg(bypass_color)),
            Span::raw(" "),
            Span::styled(effect.name.as_str(), name_style),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Render the parameter section for the currently selected item: the
/// synth's parameters or the selected effect's parameters, plus the numeric
/// edit buffer while a value is being typed.
fn draw_param_section(frame: &mut Frame, app: &App, track: &TrackInfo, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let focused = app.is_focused(FocusedPanel::Effects);
    let mut lines = if app.synth_state.synth_selected {
        param_lines(
            &track.synth_param_infos,
            &track.synth_param_values,
            app.synth_state.selected_synth_param,
            focused,
            |i, value| track.synthesis_mode.format_param_value(i, value),
        )
    } else if let Some(effect) = track.effects.get(app.synth_state.selected_effect) {
        param_lines(
            &effect.param_infos,
            &effect.param_values,
            app.synth_state.selected_param,
            focused,
            |_, value| format_effect_value(value),
        )
    } else {
        vec![Line::from(Span::styled(
            "  No effects \u{2014} A to add",
            theme::style_text_dimmed(),
        ))]
    };

    // Show the value being typed so the user can see what Enter will apply.
    if app.input_mode == InputMode::ParameterEdit {
        lines.push(Line::from(vec![
            Span::styled("  Value: ", theme::style_text_secondary()),
            Span::styled(
                format!("{}_", app.param_edit_buffer),
                theme::style_selected(),
            ),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Maximum characters of a parameter name shown in the sidebar.
const PARAM_NAME_WIDTH: usize = 12;

/// Build one line per parameter: marker, name, formatted value and unit.
fn param_lines<'a>(
    infos: &'a [kazoo_core::ParamInfo],
    values: &[f32],
    selected: usize,
    focused: bool,
    format_value: impl Fn(usize, f32) -> String,
) -> Vec<Line<'a>> {
    if infos.is_empty() {
        return vec![Line::from(Span::styled(
            "  No parameters",
            theme::style_text_dimmed(),
        ))];
    }

    infos
        .iter()
        .zip(values)
        .enumerate()
        .map(|(i, (info, &value))| {
            let is_selected = i == selected;
            let formatted = format_value(i, value);
            let unit = if info.unit.is_empty() {
                String::new()
            } else {
                format!(" {}", info.unit)
            };
            // Truncate by characters, never by bytes: names may contain
            // multi-byte characters.
            let name: String = info.name.chars().take(PARAM_NAME_WIDTH).collect();

            let marker = if is_selected { "\u{25b6}" } else { " " };
            let (name_style, value_style) = if is_selected && focused {
                (theme::style_selected(), theme::style_selected())
            } else {
                (theme::style_text_secondary(), theme::style_text())
            };

            Line::from(vec![
                Span::styled(marker, name_style),
                Span::styled(format!("{name:<PARAM_NAME_WIDTH$}"), name_style),
                Span::raw(" "),
                Span::styled(format!("{formatted}{unit}"), value_style),
            ])
        })
        .collect()
}

/// Format an effect parameter value with precision suited to its magnitude.
fn format_effect_value(value: f32) -> String {
    let magnitude = value.abs();
    if magnitude >= 100.0 {
        format!("{value:.0}")
    } else if magnitude >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

/// Render the hint bar at the bottom of the panel.
fn draw_hint_bar(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let hint = if app.synth_state.synth_selected {
        "\u{2191}\u{2193} nav  h/l param  \u{2190}\u{2192} adj  t synth"
    } else {
        "\u{2191}\u{2193} nav  h/l param  \u{2190}\u{2192} adj"
    };

    let para = Paragraph::new(hint).style(theme::style_text_dimmed());
    frame.render_widget(para, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn info(name: &'static str) -> kazoo_core::ParamInfo {
        kazoo_core::ParamInfo {
            name,
            min: 0.0,
            max: 1.0,
            default: 0.5,
            unit: "",
        }
    }

    #[test]
    fn param_lines_truncate_multibyte_names_without_panicking() {
        // 13 multi-byte characters: byte slicing at 12 would split a char.
        let infos = [info(
            "\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}\u{00e9}",
        )];
        let lines = param_lines(&infos, &[0.5], 0, true, |_, v| format!("{v}"));
        assert_eq!(lines.len(), 1);
        let name = lines[0].spans[1].content.to_string();
        assert_eq!(name.chars().count(), PARAM_NAME_WIDTH);
    }

    #[test]
    fn param_lines_empty_shows_placeholder() {
        let lines = param_lines(&[], &[], 0, true, |_, v| format!("{v}"));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans[0].content.contains("No parameters"));
    }

    #[test]
    fn format_effect_value_precision() {
        assert_eq!(format_effect_value(1234.56), "1235");
        assert_eq!(format_effect_value(12.345), "12.3");
        assert_eq!(format_effect_value(0.707), "0.71");
    }
}
