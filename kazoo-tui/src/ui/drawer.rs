//! Synth control drawer: bottom panel with visual slider bars.
//!
//! The drawer renders a full-width bottom panel showing the selected track's
//! synth parameters with visual slider bars. It replaces the inspector panel
//! and compresses the waveform/spectrum area when open.
//!
//! Layout:
//! ```text
//! Track 1 — Pitch Tracked Synth                [t] cycle
//!
//! Shape       [████████░░░░] Saw
//! Detune      [░░░░░░██░░░░] 0 cents
//! Cutoff      [██████████░░] 5000 Hz
//! Filter Q    [██░░░░░░░░░░] 0.71
//! Portamento  [█░░░░░░░░░░░] 20 ms
//! Env Sens    [█████░░░░░░░] 0.50
//!
//! ↑↓ select  ←→ adjust  t cycle synth  Esc close
//! ```

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, DrawerSection, TrackInfo};
use crate::theme;
use kazoo_core::Db;
use kazoo_core::synthesis::SynthesisMode;

/// Width (in characters) of the slider bar.
const SLIDER_WIDTH: usize = 16;

/// Draw the synth control drawer into the given area.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::style_panel_border(true))
        .title(" Synth Controls ")
        .title_style(theme::style_drawer_header());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 20 || inner.height < 4 {
        return;
    }

    let Some(track) = app.selected_track_info() else {
        let empty = Paragraph::new("  No track selected").style(theme::style_text_dimmed());
        frame.render_widget(empty, inner);
        return;
    };

    // Split inner: header (1), blank (1), params (variable), hint (1).
    let sections = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(1), // blank
        Constraint::Min(1),    // params
        Constraint::Length(1), // hint bar
    ])
    .split(inner);

    draw_header(frame, app, track, sections[0]);
    draw_params(frame, app, track, sections[2]);
    draw_hint_bar(frame, sections[3]);
}

/// Render the drawer header: track name + synth mode + section tabs.
fn draw_header(frame: &mut Frame, app: &App, track: &TrackInfo, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let idx = app.selected_track;
    let selected_mode = track
        .layers
        .get(track.selected_layer)
        .map_or(track.synthesis_mode, |l| l.mode);
    let mode_name = synth_mode_name(selected_mode);

    let section_style = |section: DrawerSection| -> Style {
        if section == app.drawer_section {
            Style::new()
                .fg(theme::ACCENT_FOCUS)
                .add_modifier(Modifier::BOLD)
        } else {
            theme::style_text_dimmed()
        }
    };

    let spans = vec![
        Span::raw(" "),
        Span::styled(&track.name, theme::style_track_name(idx)),
        Span::styled(format!(" — {mode_name}"), theme::style_text_secondary()),
        Span::raw("    "),
        Span::styled("Synth", section_style(DrawerSection::SynthSelector)),
        Span::styled(" | ", theme::style_text_dimmed()),
        Span::styled("Params", section_style(DrawerSection::Parameters)),
        Span::styled(" | ", theme::style_text_dimmed()),
        Span::styled("Effects", section_style(DrawerSection::Effects)),
    ];

    let header = Paragraph::new(Line::from(spans));
    frame.render_widget(header, area);
}

/// Render the parameter list with visual slider bars.
fn draw_params(frame: &mut Frame, app: &App, track: &TrackInfo, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    match app.drawer_section {
        DrawerSection::Parameters | DrawerSection::SynthSelector => {
            if track.layers.len() > 1 {
                // Reserve space for layer list at the top.
                let layer_rows = (track.layers.len() + 2).min(area.height as usize) as u16;
                let chunks = Layout::vertical([Constraint::Length(layer_rows), Constraint::Min(1)])
                    .split(area);
                draw_layer_list(frame, track, chunks[0]);
                draw_synth_params(frame, app, track, chunks[1]);
            } else {
                draw_synth_params(frame, app, track, area);
            }
        }
        DrawerSection::Effects => {
            draw_effect_section(frame, app, track, area);
        }
    }
}

/// Render the layer list at the top of the drawer.
fn draw_layer_list(frame: &mut Frame, track: &TrackInfo, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(Line::from(Span::styled(
        "  Layers:",
        theme::style_text_secondary(),
    )));

    for (i, layer) in track.layers.iter().enumerate() {
        let is_selected = i == track.selected_layer;
        let marker = if is_selected { "> " } else { "  " };
        let enabled_str = if layer.enabled { "ON" } else { "OFF" };
        let gain_str = format_layer_gain(layer.gain);
        let mode_short = short_mode_name(layer.mode);

        let marker_style = if is_selected {
            theme::style_drawer_header()
        } else {
            theme::style_text_dimmed()
        };
        let name_style = if is_selected {
            theme::style_drawer_header()
        } else if layer.enabled {
            theme::style_text()
        } else {
            theme::style_text_dimmed()
        };
        let enabled_style = if layer.enabled {
            Style::new().fg(theme::ACCENT_PLAY)
        } else {
            theme::style_text_dimmed()
        };

        lines.push(Line::from(vec![
            Span::styled(marker, marker_style),
            Span::styled(format!("[{}] ", i + 1), theme::style_text_dimmed()),
            Span::styled(format!("{mode_short:<16}"), name_style),
            Span::styled(format!("{gain_str:>6}  "), name_style),
            Span::styled(format!("[{enabled_str}]"), enabled_style),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Format a layer gain value for display.
fn format_layer_gain(gain: Db) -> String {
    let v = gain.value();
    if (v - 0.0).abs() < 0.05 {
        "0dB".into()
    } else {
        format!("{v:+.0}dB")
    }
}

/// Short synthesis mode name for the layer list.
const fn short_mode_name(mode: SynthesisMode) -> &'static str {
    match mode {
        SynthesisMode::PitchTracked => "Pitch Tracked",
        SynthesisMode::Wavetable => "Wavetable",
        SynthesisMode::Granular => "Granular",
        SynthesisMode::Vocoder => "Vocoder",
        SynthesisMode::PhaseVocoder => "Phase Vocoder",
    }
}

/// Render synth parameters with slider bars.
fn draw_synth_params(frame: &mut Frame, app: &App, track: &TrackInfo, area: Rect) {
    // Use the selected layer's param data (not the layer 0 shortcuts).
    let (param_infos, param_values, layer_mode) = track.layers.get(track.selected_layer).map_or(
        (
            &track.synth_param_infos,
            &track.synth_param_values,
            track.synthesis_mode,
        ),
        |layer| (&layer.param_infos, &layer.param_values, layer.mode),
    );

    if param_infos.is_empty() {
        let empty = Paragraph::new("  No parameters").style(theme::style_text_dimmed());
        frame.render_widget(empty, area);
        return;
    }

    let available_height = area.height as usize;

    // Determine scroll window so the selected param is visible.
    let selected = app.drawer_param_index;
    let total = param_infos.len();
    let scroll_start = if selected >= available_height {
        selected - available_height + 1
    } else {
        0
    };

    let mut lines: Vec<Line<'_>> = Vec::with_capacity(available_height);
    let visible_count = total.saturating_sub(scroll_start).min(available_height);

    for (i, info) in param_infos
        .iter()
        .enumerate()
        .skip(scroll_start)
        .take(visible_count)
    {
        let value = param_values.get(i).copied().unwrap_or(0.0);
        let is_selected = i == selected;

        // Normalized value for slider (0.0..1.0).
        let range = info.max - info.min;
        let normalized = if range > f32::EPSILON {
            ((value - info.min) / range).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // Build slider bar.
        let slider = render_slider(normalized, SLIDER_WIDTH);

        // Format value with unit using the selected layer's mode.
        let formatted = layer_mode.format_param_value(i, value);
        let unit = if info.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", info.unit)
        };

        // Truncate param name to fit (Unicode-safe).
        let max_name_len = 14;
        let name: String = if info.name.chars().count() > max_name_len {
            info.name.chars().take(max_name_len).collect()
        } else {
            info.name.clone()
        };

        let marker = if is_selected { "> " } else { "  " };
        let name_style = if is_selected {
            theme::style_drawer_header()
        } else {
            theme::style_text_secondary()
        };
        let value_style = if is_selected {
            Style::new()
                .fg(theme::FG_PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else {
            theme::style_text()
        };

        let mut spans = vec![
            Span::styled(marker, name_style),
            Span::styled(format!("{name:<14}"), name_style),
            Span::raw(" "),
        ];
        spans.extend(slider);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{formatted}{unit}"), value_style));

        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Render effect chain section in the drawer.
fn draw_effect_section(frame: &mut Frame, _app: &App, track: &TrackInfo, area: Rect) {
    if track.effect_names.is_empty() {
        let empty = Paragraph::new("  No effects in chain").style(theme::style_text_dimmed());
        frame.render_widget(empty, area);
        return;
    }

    let lines: Vec<Line<'_>> = track
        .effect_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let bypassed = track.effect_bypassed.get(i).copied().unwrap_or(false);
            let indicator = if bypassed { "\u{25cb}" } else { "\u{25cf}" };
            let color = if bypassed {
                theme::FG_DIMMED
            } else {
                theme::ACCENT_PLAY
            };
            let name_style = if bypassed {
                theme::style_text_dimmed()
            } else {
                theme::style_text()
            };

            Line::from(vec![
                Span::raw("  "),
                Span::styled(indicator, Style::new().fg(color)),
                Span::raw(" "),
                Span::styled(name.as_str(), name_style),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Render the hint bar at the bottom.
fn draw_hint_bar(frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let hint = if area.width >= 80 {
        " \u{2191}\u{2193} select  \u{2190}\u{2192} adjust  t cycle  n/x layer  [/] layer  Esc close"
    } else if area.width >= 60 {
        " \u{2191}\u{2193} select  \u{2190}\u{2192} adjust  n/x layer  Esc close"
    } else if area.width >= 40 {
        " \u{2191}\u{2193} \u{2190}\u{2192}  n/x layer  Esc close"
    } else {
        " \u{2191}\u{2193} \u{2190}\u{2192}  Esc close"
    };
    let para = Paragraph::new(hint).style(theme::style_text_dimmed());
    frame.render_widget(para, area);
}

/// Render a slider bar as a sequence of styled spans.
///
/// `ratio` is clamped to `[0.0, 1.0]`. The slider uses filled blocks (`\u{2588}`)
/// for the filled portion and light shade (`\u{2591}`) for the empty portion.
fn render_slider(ratio: f32, width: usize) -> Vec<Span<'static>> {
    let ratio = ratio.clamp(0.0, 1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let filled = (ratio * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);

    let filled_str: String = "\u{2588}".repeat(filled);
    let empty_str: String = "\u{2591}".repeat(empty);

    vec![
        Span::styled("[", theme::style_text_dimmed()),
        Span::styled(filled_str, theme::style_slider_filled()),
        Span::styled(empty_str, theme::style_slider_empty()),
        Span::styled("]", theme::style_text_dimmed()),
    ]
}

/// Get the display name for a synthesis mode (full name for the drawer).
const fn synth_mode_name(mode: SynthesisMode) -> &'static str {
    match mode {
        SynthesisMode::PitchTracked => "Pitch Tracked Synth",
        SynthesisMode::Wavetable => "Wavetable Synth",
        SynthesisMode::Granular => "Granular Synth",
        SynthesisMode::Vocoder => "Vocoder",
        SynthesisMode::PhaseVocoder => "Phase Vocoder",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_slider_empty() {
        let spans = render_slider(0.0, 12);
        // [............] — bracket + 0 filled + 12 empty + bracket
        assert_eq!(spans.len(), 4);
    }

    #[test]
    fn render_slider_full() {
        let spans = render_slider(1.0, 12);
        assert_eq!(spans.len(), 4);
    }

    #[test]
    fn render_slider_half() {
        let spans = render_slider(0.5, 12);
        assert_eq!(spans.len(), 4);
    }

    #[test]
    fn render_slider_clamps_above_one() {
        let spans = render_slider(1.5, 12);
        assert_eq!(spans.len(), 4);
    }

    #[test]
    fn render_slider_clamps_below_zero() {
        let spans = render_slider(-0.5, 12);
        assert_eq!(spans.len(), 4);
    }

    #[test]
    fn render_slider_zero_width() {
        let spans = render_slider(0.5, 0);
        assert_eq!(spans.len(), 4);
    }

    #[test]
    fn synth_mode_names_are_not_empty() {
        for mode in [
            SynthesisMode::PitchTracked,
            SynthesisMode::Wavetable,
            SynthesisMode::Granular,
            SynthesisMode::Vocoder,
            SynthesisMode::PhaseVocoder,
        ] {
            assert!(!synth_mode_name(mode).is_empty());
        }
    }
}
