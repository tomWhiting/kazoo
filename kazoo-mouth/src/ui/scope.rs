use kazoo_core::sanitize_sample;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Points};

pub(super) const VOICE: Color = Color::Green;
pub(super) const SYNTH: Color = Color::Magenta;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Both,
    Voice,
    Synth,
}

impl Mode {
    pub(crate) const fn next(self) -> Self {
        match self {
            Self::Both => Self::Voice,
            Self::Voice => Self::Synth,
            Self::Synth => Self::Both,
        }
    }
}

fn window(samples: &[f32], rate: u32, hz: Option<f32>) -> (f64, f64) {
    let duration = hz
        .filter(|hz| hz.is_finite() && *hz > 0.0)
        .map_or(0.020, |hz| 4.0 / f64::from(hz));
    let length = (duration * f64::from(rate))
        .min(samples.len().saturating_sub(1) as f64)
        .max(1.0);
    let nominal = (samples.len() as f64 - length - 1.0).max(0.0);
    // Searching backwards keeps the whole four-period window inside the capture.
    let crossing = (0..=nominal.floor() as usize).rev().find(|&index| {
        sanitize_sample(samples[index]) <= 0.0 && sanitize_sample(samples[index + 1]) > 0.0
    });
    let start = crossing.map_or(nominal, |index| {
        let left = f64::from(sanitize_sample(samples[index]));
        let right = f64::from(sanitize_sample(samples[index + 1]));
        index as f64 - left / (right - left)
    });
    (start, length)
}

pub(super) fn points(
    samples: &[f32],
    trigger: &[f32],
    rate: u32,
    hz: Option<f32>,
    width: usize,
) -> Vec<(f64, f64)> {
    if samples.len() < 2 || trigger.len() != samples.len() {
        return Vec::new();
    }
    let (start, length) = window(trigger, rate, hz);
    (0..width.max(2))
        .map(|column| {
            let position = start + column as f64 * length / (width.max(2) - 1) as f64;
            let index = position.floor() as usize;
            let fraction = position - index as f64;
            let left = f64::from(sanitize_sample(samples[index]));
            let right = f64::from(sanitize_sample(samples[(index + 1).min(samples.len() - 1)]));
            let sample = (right - left).mul_add(fraction, left);
            let sample = if sample.abs() < 1.0e-6 { 0.0 } else { sample };
            (column as f64 / (width.max(2) - 1) as f64, sample)
        })
        .collect()
}

pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, app: &crate::controls::App, hz: Option<f32>) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let width = usize::from(area.width) * 2;
    let trigger = if app.plots.mode == Mode::Synth {
        &app.scope_synth
    } else {
        &app.scope_voice
    };
    let voice = points(&app.scope_voice, trigger, app.loop_sample_rate, hz, width);
    let synth = points(&app.scope_synth, trigger, app.loop_sample_rate, hz, width);
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, 1.0])
        .y_bounds([-1.0, 1.0])
        .paint(|context| {
            let mut labels = Vec::new();
            if app.plots.mode != Mode::Synth {
                labels.push(Span::styled("voice ", Style::default().fg(VOICE)));
            }
            if app.plots.mode != Mode::Voice {
                labels.push(Span::styled("synth", Style::default().fg(SYNTH)));
            }
            context.print(0.0, 1.0, Line::from(labels));
            if app.plots.mode != Mode::Synth {
                context.draw(&Points {
                    coords: &voice,
                    color: VOICE,
                });
            }
            if app.plots.mode != Mode::Voice {
                context.layer();
                context.draw(&Points {
                    coords: &synth,
                    color: SYNTH,
                });
            }
        });
    frame.render_widget(canvas, area);
}

pub(super) fn strip(
    frame: &mut Frame<'_>,
    plots: Rect,
    app: &crate::controls::App,
    display: &super::Display,
) {
    let scope_width = if plots.width >= 80 {
        (plots.width / 2).max(40)
    } else {
        plots.width
    };
    let trail_width = plots.width.saturating_sub(scope_width);
    if trail_width > 0 {
        super::trail::draw(
            frame,
            Rect::new(plots.x, plots.y, trail_width, plots.height),
            display,
            app.accepted.quantise,
        );
    }
    let scope_area = Rect::new(plots.x + trail_width, plots.y, scope_width, plots.height);
    let hz = display
        .latest
        .filter(|voice| voice.voiced)
        .map(|voice| voice.hz);
    let spectrum_height = if app.plots.spectrum_visible && scope_area.height >= 3 {
        2
    } else {
        0
    };
    draw(
        frame,
        Rect::new(
            scope_area.x,
            scope_area.y,
            scope_area.width,
            scope_area.height - spectrum_height,
        ),
        app,
        hz,
    );
    if spectrum_height > 0 {
        super::spectrum::draw(
            frame,
            Rect::new(
                scope_area.x,
                scope_area.bottom() - spectrum_height,
                scope_area.width,
                spectrum_height,
            ),
            &app.spectrum,
            hz,
        );
    }
}

#[derive(Debug, Default)]
pub struct View {
    pub mode: Mode,
    pub spectrum_visible: bool,
    pub latency_open: bool,
}

impl View {
    pub const fn key(&mut self, key: crossterm::event::KeyCode) -> bool {
        match key {
            crossterm::event::KeyCode::Char('o') => self.mode = self.mode.next(),
            crossterm::event::KeyCode::Char('k') => self.spectrum_visible = !self.spectrum_visible,
            crossterm::event::KeyCode::Char('L') => self.latency_open = !self.latency_open,
            _ => return false,
        }
        true
    }
}
