use std::f64::consts::TAU;

use kazoo_core::sanitize_sample;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::engine::capture::SAMPLES;

pub(super) const BANDS: usize = 32;

pub(super) fn band(hz: f64) -> Option<usize> {
    if !hz.is_finite() || !(50.0..=10_000.0).contains(&hz) {
        return None;
    }
    Some(((hz / 50.0).log(200.0) * BANDS as f64) as usize).map(|index| index.min(BANDS - 1))
}

#[derive(Debug)]
pub struct Spectrum {
    real: Box<[f64]>,
    imaginary: Box<[f64]>,
    pub(super) levels: [f64; BANDS],
}

impl Default for Spectrum {
    fn default() -> Self {
        Self {
            real: vec![0.0; SAMPLES].into_boxed_slice(),
            imaginary: vec![0.0; SAMPLES].into_boxed_slice(),
            levels: [0.0; BANDS],
        }
    }
}

impl Spectrum {
    pub(crate) fn update(&mut self, samples: &[f32; SAMPLES], rate: u32) {
        for (index, sample) in samples.iter().enumerate() {
            let reversed = index.reverse_bits() >> (usize::BITS - SAMPLES.ilog2());
            let window = 0.5_f64.mul_add(-(TAU * index as f64 / (SAMPLES - 1) as f64).cos(), 0.5);
            self.real[reversed] = f64::from(sanitize_sample(*sample)) * window;
            self.imaginary[reversed] = 0.0;
        }
        let mut size = 2;
        while size <= SAMPLES {
            for start in (0..SAMPLES).step_by(size) {
                for offset in 0..size / 2 {
                    let angle = -TAU * offset as f64 / size as f64;
                    let (sin, cos) = angle.sin_cos();
                    let a = start + offset;
                    let b = a + size / 2;
                    let real = cos.mul_add(self.real[b], -sin * self.imaginary[b]);
                    let imaginary = sin.mul_add(self.real[b], cos * self.imaginary[b]);
                    self.real[b] = self.real[a] - real;
                    self.imaginary[b] = self.imaginary[a] - imaginary;
                    self.real[a] += real;
                    self.imaginary[a] += imaginary;
                }
            }
            size *= 2;
        }
        self.levels.fill(0.0);
        for bin in 1..SAMPLES / 2 {
            if let Some(index) = band(bin as f64 * f64::from(rate) / SAMPLES as f64) {
                let amplitude = self.real[bin].hypot(self.imaginary[bin]) * 4.0 / SAMPLES as f64;
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
