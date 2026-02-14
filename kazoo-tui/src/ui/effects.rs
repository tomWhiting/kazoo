//! Effects chain inspector panel.
//!
//! Shows the effects chain for the currently selected track in a 30-column
//! inspector area on the right side of the UI. The panel is divided into
//! three sections:
//!
//! 1. **Track header** -- name, synthesis mode, M/S/R indicators.
//! 2. **Effect chain list** -- ordered list of effects with bypass state.
//! 3. **Parameter section** -- details and hints for the selected effect.

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

    // Split inner into 3 sections: header (3), effects list (40%), params (rest).
    let sections = Layout::vertical([
        Constraint::Length(3),
        Constraint::Percentage(40),
        Constraint::Min(3),
    ])
    .split(inner);

    draw_track_header(frame, app, track, sections[0]);
    draw_effect_list(frame, app, track, sections[1]);
    draw_param_section(frame, app, track, sections[2]);
}

/// Render the track header: name, synthesis mode abbreviation, and M/S/R flags.
fn draw_track_header(frame: &mut Frame, app: &App, track: &TrackInfo, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let idx = app.selected_track;
    let name_style = theme::style_track_name(idx);
    let mode_str = match track.synthesis_mode {
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

/// Render the effect chain list with bypass indicators and selection highlight.
fn draw_effect_list(frame: &mut Frame, app: &App, track: &TrackInfo, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    if track.effect_names.is_empty() {
        let empty = Paragraph::new("  No effects").style(theme::style_text_dimmed());
        frame.render_widget(empty, area);
        return;
    }

    let focused = app.is_focused(FocusedPanel::Effects);

    // Build lines manually so we can render them as a single Paragraph
    // and handle overflow gracefully within the available height.
    let lines: Vec<Line<'_>> = track
        .effect_names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let bypassed = track.effect_bypassed.get(i).copied().unwrap_or(false);
            let selected = i == app.selected_effect;

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

            Line::from(vec![
                Span::styled(bypass_indicator, Style::new().fg(bypass_color)),
                Span::raw(" "),
                Span::styled(name.as_str(), name_style),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Render the parameter section for the currently selected effect.
///
/// Since [`TrackInfo`] does not carry full parameter metadata (that lives in
/// the engine), this section shows contextual hints and the edit buffer when
/// the user is in parameter-edit mode.
fn draw_param_section(frame: &mut Frame, app: &App, track: &TrackInfo, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let text = if track.effect_names.is_empty() {
        String::from("  Add effects with\n  effect commands")
    } else {
        let fx_name = track
            .effect_names
            .get(app.selected_effect)
            .map_or("\u{2014}", String::as_str);
        format!("  {fx_name}\n  Parameters:\n  (use +/- to adjust)")
    };

    // If in parameter edit mode, append the edit buffer with a cursor.
    let text = if app.input_mode == InputMode::ParameterEdit {
        format!("{text}\n  Value: {}_", app.param_edit_buffer)
    } else {
        text
    };

    let para = Paragraph::new(text).style(theme::style_text_secondary());
    frame.render_widget(para, area);
}
