//! Braille-rendered waveform display widget.
//!
//! Draws an oscilloscope view of recent audio samples using the Ratatui
//! [`Canvas`] with Braille markers for high-resolution rendering. Supports
//! zoom and scroll via [`App::waveform_zoom`] and [`App::waveform_scroll`].

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};

use crate::app::{App, FocusedPanel};
use crate::theme;

/// Render the waveform oscilloscope panel into the given area.
///
/// Features:
/// - Center line at y=0 (zero-crossing reference) drawn in dimmed color.
/// - Waveform drawn as vertical min/max pairs per column using the first
///   track color.
/// - Zoom and scroll controlled by `app.tracking_state.waveform_zoom` and
///   `app.tracking_state.waveform_scroll`.
/// - Playback cursor (green vertical line when playing).
/// - Recording indicator (red/pink cursor when recording).
/// - Displays "No signal" in dimmed text when the waveform buffer is empty.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = super::panel_block(" Waveform ", FocusedPanel::Waveform, app);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let waveform = &app.display.waveform;
    if waveform.is_empty() {
        let empty = Paragraph::new("  No signal").style(theme::style_text_dimmed());
        frame.render_widget(empty, inner);
        return;
    }

    let width = inner.width as usize;
    let height = inner.height as usize;
    if width == 0 || height == 0 {
        return;
    }

    // Calculate the visible window based on zoom and scroll.
    let total_samples = waveform.len();
    let visible_samples = ((total_samples as f32 / app.tracking_state.waveform_zoom).max(1.0)
        as usize)
        .min(total_samples)
        .max(1);
    let max_offset = total_samples.saturating_sub(visible_samples);
    let offset =
        ((app.tracking_state.waveform_scroll * max_offset as f32) as usize).min(max_offset);

    let visible = &waveform[offset..offset + visible_samples];

    // Pre-compute per-column min/max pairs so the paint closure can own
    // the data without borrowing `visible`.
    let samples_per_col = visible_samples as f64 / width as f64;
    let col_ranges = compute_column_ranges(visible, width, samples_per_col);

    // Determine transport state for cursor drawing.
    let is_playing = matches!(
        app.display.transport.state,
        kazoo_core::transport::TransportState::Playing
    );
    let is_recording = matches!(
        app.display.transport.state,
        kazoo_core::transport::TransportState::Recording
    );

    let wave_color = theme::track_color(0);
    let dimmed_color = theme::FG_DIMMED;
    let play_color = theme::ACCENT_PLAY;
    let record_color = theme::ACCENT_RECORD;
    let w = width as f64;

    let canvas = Canvas::default()
        .marker(symbols::Marker::Braille)
        .x_bounds([0.0, w])
        .y_bounds([-1.0, 1.0])
        .background_color(theme::BG_PRIMARY)
        .paint(move |ctx| {
            // Draw center line (zero-crossing reference).
            ctx.draw(&CanvasLine {
                x1: 0.0,
                y1: 0.0,
                x2: w,
                y2: 0.0,
                color: dimmed_color,
            });

            // Draw waveform as vertical lines (min-to-max per column).
            for (col, &(min_val, max_val)) in col_ranges.iter().enumerate() {
                ctx.draw(&CanvasLine {
                    x1: col as f64 + 0.5,
                    y1: min_val,
                    x2: col as f64 + 0.5,
                    y2: max_val,
                    color: wave_color,
                });
            }

            // Draw playback / recording cursor as a vertical line at the
            // right edge (the most recent sample position).
            if is_playing || is_recording {
                let cursor_x = w - 1.0;
                let cursor_color = if is_recording {
                    record_color
                } else {
                    play_color
                };
                ctx.draw(&CanvasLine {
                    x1: cursor_x,
                    y1: -1.0,
                    x2: cursor_x,
                    y2: 1.0,
                    color: cursor_color,
                });
            }
        });

    frame.render_widget(canvas, inner);
}

/// Compute per-column (min, max) pairs from the visible waveform slice.
///
/// Each column maps to a range of samples determined by `samples_per_col`.
/// Samples are clamped to [-1, 1] and non-finite values are skipped.
fn compute_column_ranges(visible: &[f32], width: usize, samples_per_col: f64) -> Vec<(f64, f64)> {
    let mut ranges = Vec::with_capacity(width);
    for col in 0..width {
        let start = (col as f64 * samples_per_col) as usize;
        let end = (((col + 1) as f64 * samples_per_col) as usize).min(visible.len());
        if start >= end {
            ranges.push((0.0, 0.0));
            continue;
        }
        let slice = &visible[start..end];
        let mut min_val = f32::INFINITY;
        let mut max_val = f32::NEG_INFINITY;
        for &s in slice {
            let s = s.clamp(-1.0, 1.0);
            if s < min_val {
                min_val = s;
            }
            if s > max_val {
                max_val = s;
            }
        }
        // NaN defense: treat non-finite columns as zero-height lines to
        // maintain correct x-coordinate alignment (must push every column).
        if !min_val.is_finite() || !max_val.is_finite() {
            ranges.push((0.0, 0.0));
            continue;
        }
        ranges.push((f64::from(min_val), f64::from(max_val)));
    }
    ranges
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for f32→f64 comparison (f32 has ~7 decimal digits).
    const TOL: f64 = 1e-6;

    #[test]
    fn column_ranges_basic_sine() {
        // 4 samples, 2 columns => 2 samples per column.
        let visible = [0.5, -0.5, 0.8, -0.8];
        let ranges = compute_column_ranges(&visible, 2, 2.0);
        assert_eq!(ranges.len(), 2);
        assert!((ranges[0].0 - (-0.5)).abs() < TOL);
        assert!((ranges[0].1 - 0.5).abs() < TOL);
        assert!((ranges[1].0 - (-0.8)).abs() < TOL);
        assert!((ranges[1].1 - 0.8).abs() < TOL);
    }

    #[test]
    fn column_ranges_clamps_out_of_range_samples() {
        // Samples outside [-1, 1] are clamped.
        let visible = [2.0, -3.0];
        let ranges = compute_column_ranges(&visible, 1, 2.0);
        assert_eq!(ranges.len(), 1);
        assert!((ranges[0].0 - (-1.0)).abs() < TOL);
        assert!((ranges[0].1 - 1.0).abs() < TOL);
    }

    #[test]
    fn column_ranges_nan_defense() {
        // NaN samples should produce (0.0, 0.0) for that column.
        let visible = [f32::NAN, f32::NAN];
        let ranges = compute_column_ranges(&visible, 1, 2.0);
        assert_eq!(ranges.len(), 1);
        assert!((ranges[0].0).abs() < TOL);
        assert!((ranges[0].1).abs() < TOL);
    }

    #[test]
    fn column_ranges_infinity_defense() {
        let visible = [f32::INFINITY, f32::NEG_INFINITY];
        let ranges = compute_column_ranges(&visible, 1, 2.0);
        assert_eq!(ranges.len(), 1);
        // Infinities get clamped to ±1.0, so result should be (-1.0, 1.0).
        assert!((ranges[0].0 - (-1.0)).abs() < TOL);
        assert!((ranges[0].1 - 1.0).abs() < TOL);
    }

    #[test]
    fn column_ranges_empty_visible() {
        let visible: [f32; 0] = [];
        let ranges = compute_column_ranges(&visible, 3, 1.0);
        assert_eq!(ranges.len(), 3);
        // All columns should be (0.0, 0.0) since start >= end.
        for &(min, max) in &ranges {
            assert!(min.abs() < TOL);
            assert!(max.abs() < TOL);
        }
    }

    #[test]
    fn column_ranges_zero_width() {
        let visible = [0.5, -0.5];
        let ranges = compute_column_ranges(&visible, 0, 2.0);
        assert!(ranges.is_empty());
    }

    #[test]
    fn column_ranges_more_columns_than_samples() {
        // 2 samples, 4 columns, 0.5 samples_per_col.
        // Due to integer truncation: col 0 start=0 end=0 (empty),
        // col 1 start=0 end=1, col 2 start=1 end=1 (empty), col 3 start=1 end=2.
        let visible = [0.3, -0.7];
        let ranges = compute_column_ranges(&visible, 4, 0.5);
        assert_eq!(ranges.len(), 4);
        // Columns 0 and 2 are empty (start >= end).
        assert!(ranges[0].0.abs() < TOL);
        assert!(ranges[0].1.abs() < TOL);
        // Column 1 gets sample[0] = 0.3.
        assert!((ranges[1].0 - 0.3).abs() < TOL);
        assert!((ranges[1].1 - 0.3).abs() < TOL);
        // Column 2 is empty.
        assert!(ranges[2].0.abs() < TOL);
        assert!(ranges[2].1.abs() < TOL);
        // Column 3 gets sample[1] = -0.7.
        assert!((ranges[3].0 - (-0.7)).abs() < TOL);
        assert!((ranges[3].1 - (-0.7)).abs() < TOL);
    }

    #[test]
    fn column_ranges_single_sample_per_column() {
        let visible = [0.1, 0.2, 0.3];
        let ranges = compute_column_ranges(&visible, 3, 1.0);
        assert_eq!(ranges.len(), 3);
        for (i, &(min, max)) in ranges.iter().enumerate() {
            let expected = f64::from(visible[i]);
            assert!((min - expected).abs() < TOL);
            assert!((max - expected).abs() < TOL);
        }
    }

    #[test]
    fn column_ranges_mixed_nan_and_valid() {
        // Mix of NaN and valid samples.
        // NaN.clamp(-1.0, 1.0) = NaN in Rust, and NaN comparisons return false,
        // so NaN samples are effectively skipped in the min/max scan.
        let visible = [f32::NAN, 0.5, -0.3, f32::NAN];
        let ranges = compute_column_ranges(&visible, 2, 2.0);
        assert_eq!(ranges.len(), 2);
        // Column 0 (samples: NaN, 0.5): only 0.5 participates.
        assert!((ranges[0].0 - 0.5).abs() < TOL);
        assert!((ranges[0].1 - 0.5).abs() < TOL);
        // Column 1 (samples: -0.3, NaN): only -0.3 participates.
        assert!((ranges[1].0 - (-0.3)).abs() < TOL);
        assert!((ranges[1].1 - (-0.3)).abs() < TOL);
    }
}
