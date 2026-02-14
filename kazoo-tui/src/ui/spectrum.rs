//! FFT spectrum analyzer display.
//!
//! Draws a frequency spectrum with logarithmic frequency scale (20 Hz to
//! 20 kHz) using the Ratatui [`Canvas`] with Braille markers. Each column
//! is colored by level using the theme's meter color ramp (green / yellow /
//! red). Frequency reference lines are drawn at 100 Hz, 1 kHz, and 10 kHz.

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};

use crate::app::{App, FocusedPanel};
use crate::theme;

/// Minimum displayed frequency in Hz (log scale lower bound).
const MIN_FREQ: f64 = 20.0;

/// Maximum displayed frequency in Hz (log scale upper bound).
const MAX_FREQ: f64 = 20_000.0;

/// Minimum displayed magnitude in dB (bottom of the display).
const DB_MIN: f64 = -100.0;

/// Maximum displayed magnitude in dB (top of the display).
const DB_MAX: f64 = 0.0;

/// Frequencies at which to draw vertical reference lines.
const REFERENCE_FREQS: [f64; 3] = [100.0, 1_000.0, 10_000.0];

/// Render the spectrum analyzer panel into the given area.
///
/// Features:
/// - X-axis: logarithmic frequency scale (`MIN_FREQ` to `MAX_FREQ`).
/// - Y-axis: magnitude in dB (`DB_MIN` to `DB_MAX`).
/// - Bars from bottom up, colored by level (green / yellow / red).
/// - Frequency reference lines at 100 Hz, 1 kHz, and 10 kHz in dimmed color.
/// - Displays "No spectrum data" in dimmed text when magnitudes are empty.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = super::panel_block(" Spectrum ", FocusedPanel::Spectrum, app);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let magnitudes = &app.display.spectrum_magnitudes;
    if magnitudes.is_empty() {
        let empty = Paragraph::new("  No spectrum data").style(theme::style_text_dimmed());
        frame.render_widget(empty, inner);
        return;
    }

    let width = inner.width as usize;
    let height = inner.height as usize;
    if width == 0 || height == 0 {
        return;
    }

    let num_bins = magnitudes.len();
    let sample_rate = app.display.transport.position.sample_rate;

    // Pre-compute per-column magnitude and color so the paint closure can
    // own the data without borrowing into `app`.
    let column_data = compute_column_data(magnitudes, num_bins, sample_rate, width);

    let dimmed_color = theme::FG_DIMMED;
    let w = width as f64;
    let log_min = MIN_FREQ.ln();
    let log_max = MAX_FREQ.ln();

    let canvas = Canvas::default()
        .marker(symbols::Marker::Braille)
        .x_bounds([0.0, w])
        .y_bounds([DB_MIN, DB_MAX])
        .background_color(theme::BG_PRIMARY)
        .paint(move |ctx| {
            // Draw frequency reference lines (dimmed vertical lines).
            for &freq in &REFERENCE_FREQS {
                let t = (freq.ln() - log_min) / (log_max - log_min);
                let x = t * w;
                ctx.draw(&CanvasLine {
                    x1: x,
                    y1: DB_MIN,
                    x2: x,
                    y2: DB_MAX,
                    color: dimmed_color,
                });
            }

            // Draw spectrum bars: one vertical line per column from DB_MIN
            // up to the interpolated magnitude.
            for (col, &(mag_db, color)) in column_data.iter().enumerate() {
                ctx.draw(&CanvasLine {
                    x1: col as f64 + 0.5,
                    y1: DB_MIN,
                    x2: col as f64 + 0.5,
                    y2: mag_db,
                    color,
                });
            }
        });

    frame.render_widget(canvas, inner);
}

/// Pre-compute (`magnitude_db`, `color`) for each display column.
///
/// Maps each column to a logarithmic frequency, finds the corresponding
/// FFT bin via linear interpolation, and determines the meter color.
fn compute_column_data(
    magnitudes: &[f32],
    num_bins: usize,
    sample_rate: u32,
    width: usize,
) -> Vec<(f64, Color)> {
    let log_min = MIN_FREQ.ln();
    let log_max = MAX_FREQ.ln();
    let sr = f64::from(sample_rate);
    let db_range = DB_MAX - DB_MIN;

    let mut data = Vec::with_capacity(width);
    for col in 0..width {
        let t = col as f64 / width as f64;
        let log_freq = t.mul_add(log_max - log_min, log_min);
        let freq = log_freq.exp();

        // Map frequency to fractional FFT bin index.
        // bin = freq * (num_bins * 2) / sample_rate
        let bin_f = freq * (num_bins as f64 * 2.0) / sr;
        let bin_low = (bin_f as usize).min(num_bins.saturating_sub(1));
        let bin_high = (bin_low + 1).min(num_bins.saturating_sub(1));
        let frac = (bin_f - bin_low as f64) as f32;

        let mag_low = magnitudes.get(bin_low).copied().unwrap_or(DB_MIN as f32);
        let mag_high = magnitudes.get(bin_high).copied().unwrap_or(DB_MIN as f32);
        let mag_db = if bin_low == bin_high {
            mag_low
        } else {
            frac.mul_add(mag_high, (1.0 - frac) * mag_low)
        };

        // Clamp and defend against NaN.
        let mag_db = if mag_db.is_finite() {
            f64::from(mag_db).clamp(DB_MIN, DB_MAX)
        } else {
            DB_MIN
        };

        // Compute the 0..1 ratio for coloring.
        let ratio = ((mag_db - DB_MIN) / db_range) as f32;
        let color = theme::meter_color(ratio);

        data.push((mag_db, color));
    }
    data
}
