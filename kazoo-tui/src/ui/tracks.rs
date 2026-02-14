//! Track list and arrangement view.
//!
//! Renders a scrollable list of tracks in a 26-column panel. Each track
//! line shows a numeric index, a color bar, the track name (truncated),
//! synthesis mode abbreviation, mute/solo/arm indicators, and a mini
//! level bar derived from the engine's meter data.

use ratatui::prelude::*;
use ratatui::widgets::{List, ListItem, Paragraph};

use crate::app::{App, FocusedPanel};
use crate::theme;
use kazoo_core::synthesis::SynthesisMode;

/// Draw the track list into the given area.
///
/// Uses `render_stateful_widget` with the app's `track_list_state` so that
/// ratatui handles scroll offset and selection highlighting. Because this
/// requires `&mut ListState`, the top-level `draw` function passes
/// `&mut App`.
pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = super::panel_block(" Tracks ", FocusedPanel::Tracks, app);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.tracks.is_empty() {
        let empty = Paragraph::new("  No tracks\n  Press 'n'").style(theme::style_text_dimmed());
        frame.render_widget(empty, inner);
        return;
    }

    let items: Vec<ListItem<'_>> = app
        .tracks
        .iter()
        .enumerate()
        .map(|(i, track)| {
            let color = theme::track_color(i);

            // Track index (1-based).
            let idx = format!("{}", i + 1);

            // Truncate name to 6 characters for the narrow panel.
            // Use char_indices for safe UTF-8 boundary handling.
            let name: &str = match track.name.char_indices().nth(6) {
                Some((byte_idx, _)) => &track.name[..byte_idx],
                None => &track.name,
            };

            let mode_str = match track.synthesis_mode {
                SynthesisMode::PitchTracked => "Pt",
                SynthesisMode::Wavetable => "Wt",
                SynthesisMode::Granular => "Gr",
                SynthesisMode::Vocoder => "Vc",
                SynthesisMode::PhaseVocoder => "Pv",
            };

            let mut spans: Vec<Span<'_>> = vec![
                Span::styled(idx, theme::style_text_dimmed()),
                Span::raw(" "),
                Span::styled("\u{2588}\u{2588}", Style::new().fg(color)),
                Span::raw(" "),
                Span::styled(format!("{name:<6}"), Style::new().fg(color)),
                Span::raw(" "),
                Span::styled(format!("{mode_str:<2}"), theme::style_text_secondary()),
                Span::raw(" "),
            ];

            // Mute / Solo / Arm indicators.
            if track.muted {
                spans.push(Span::styled("M", theme::style_muted()));
            } else {
                spans.push(Span::styled("\u{00b7}", theme::style_text_dimmed()));
            }
            if track.soloed {
                spans.push(Span::styled("S", theme::style_soloed()));
            } else {
                spans.push(Span::styled("\u{00b7}", theme::style_text_dimmed()));
            }
            if track.armed {
                spans.push(Span::styled("R", theme::style_armed()));
            } else {
                spans.push(Span::styled("\u{00b7}", theme::style_text_dimmed()));
            }

            // Mini level bar from meter data.
            if let Some(meter) = app.display.mixer.track_meters.get(i) {
                let peak = meter.peak_db[0].max(meter.peak_db[1]);
                // Map dB to ratio: -60 dB = empty, 0 dB = full.
                let ratio = ((peak + 60.0) / 60.0).clamp(0.0, 1.0);
                let bar_width: usize = 4;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let filled = (ratio * bar_width as f32) as usize;
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    "\u{2588}".repeat(filled),
                    Style::new().fg(theme::meter_color(ratio)),
                ));
                spans.push(Span::styled(
                    "\u{2591}".repeat(bar_width.saturating_sub(filled)),
                    theme::style_text_dimmed(),
                ));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items).highlight_style(theme::style_selected());
    frame.render_stateful_widget(list, inner, &mut app.track_list_state);
}
