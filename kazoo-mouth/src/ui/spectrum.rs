use kazoo_core::analysis::SpectrumAnalyzer;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::engine::capture::spectrum_frames;

pub(super) const BANDS: usize = 32;

pub(super) fn band(hz: f64) -> Option<usize> {
    if !hz.is_finite() || !(50.0..=10_000.0).contains(&hz) {
        return None;
    }
    Some(((hz / 50.0).log(200.0) * BANDS as f64) as usize).map(|index| index.min(BANDS - 1))
}

/// The mic's 32-band spectrum, from kazoo-core's analyser over the newest
/// `spectrum_frames(rate)` captured samples. Runs on the UI thread only.
#[derive(Debug, Default)]
pub struct Spectrum {
    analyzer: Option<(u32, SpectrumAnalyzer)>,
    pub(super) levels: [f64; BANDS],
}

impl Spectrum {
    pub(crate) fn update(&mut self, samples: &[f32], rate: u32) {
        self.levels.fill(0.0);
        if rate == 0 {
            return;
        }
        let analyzer = match &mut self.analyzer {
            Some((built, analyzer)) if *built == rate => analyzer,
            slot => {
                let built = slot.insert((
                    rate,
                    SpectrumAnalyzer::new(spectrum_frames(rate), rate as f32, 0.0),
                ));
                &mut built.1
            }
        };
        let size = analyzer.fft_size();
        let Some(newest) = samples
            .len()
            .checked_sub(size)
            .map(|start| &samples[start..])
        else {
            return;
        };
        analyzer.reset();
        let Some(data) = analyzer.push_samples(newest) else {
            return;
        };
        for (hz, db) in data.bin_frequencies.iter().zip(&data.magnitudes_db).skip(1) {
            if let Some(index) = band(f64::from(*hz)) {
                // The analyser divides by the frame; a Hann-windowed sine's peak
                // is a quarter of its amplitude, so four times it reads the sine.
                let amplitude = 10.0_f64.powf(f64::from(*db) / 20.0) * 4.0;
                self.levels[index] = self.levels[index].max(amplitude);
            }
        }
    }
}

pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, spectrum: &Spectrum, hz: Option<f32>) {
    let marked = hz.and_then(|hz| band(f64::from(hz)));
    let width = usize::from(area.width);
    let count = width.min(BANDS);
    let columns = (0..count)
        .map(|column| {
            let first = column * BANDS / count;
            let last = (column + 1) * BANDS / count;
            let amplitude = spectrum.levels[first..last]
                .iter()
                .copied()
                .fold(0.0, f64::max);
            let height = if amplitude > 0.0 {
                ((20.0_f64.mul_add(amplitude.log10(), 60.0)) / 60.0 * 7.0).clamp(0.0, 7.0) as usize
            } else {
                0
            };
            let glyph = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"][height];
            let color = if marked.is_some_and(|index| (first..last).contains(&index)) {
                Color::Yellow
            } else {
                Color::Green
            };
            Span::styled(
                glyph.repeat((width / count).max(1)),
                Style::default().fg(color),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(columns),
            Line::from(hz.map_or_else(
                || "50 Hz — 10 kHz".into(),
                |hz| format!("50 Hz — 10 kHz · fundamental {hz:.0} Hz"),
            )),
        ]),
        area,
    );
}
