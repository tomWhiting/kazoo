use std::collections::VecDeque;

use kazoo_core::sanitize_sample;
use kazoo_core::voice::quantise::{QuantiseSettings, SCALES};
use kazoo_core::voice::{VOICE_HOP, VoiceFrame};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::symbols::Marker;
use ratatui::text::Span;
use ratatui::widgets::canvas::{Canvas, Line, Points};

use super::{BRIGHT, DIM, SCALE_LINE, pitch};

#[derive(Debug)]
pub(crate) struct Display {
    pub(super) frames: VecDeque<VoiceFrame>,
    pub(super) latest: Option<VoiceFrame>,
    pub(super) held: Option<VoiceFrame>,
    pub(super) centre: f64,
    centred: bool,
    rate: u32,
    capacity: usize,
    median: Vec<f64>,
}

impl Display {
    pub(crate) fn new(rate: u32) -> Self {
        let rate = rate.max(1);
        let capacity = (rate as usize * 4 / VOICE_HOP).max(1);
        Self {
            frames: VecDeque::with_capacity(capacity),
            latest: None,
            held: None,
            centre: 0.0,
            centred: false,
            rate,
            capacity,
            median: Vec::with_capacity(capacity),
        }
    }

    pub(crate) fn push(&mut self, mut voice: VoiceFrame) {
        voice.voiced &= [voice.volts, voice.snapped_volts, voice.hz, voice.snapped_hz]
            .iter()
            .all(|value| value.is_finite())
            && (-8.0..=8.0).contains(&voice.volts)
            && (-8.0..=8.0).contains(&voice.snapped_volts)
            && voice.hz > 0.0
            && voice.snapped_hz > 0.0;
        for value in [
            &mut voice.loudness,
            &mut voice.breath,
            &mut voice.confidence,
        ] {
            *value = sanitize_sample(*value).clamp(0.0, 1.0);
        }
        self.latest = Some(voice);
        if voice.voiced {
            self.held = Some(voice);
        }
        // Timestamp expiry preserves real gaps when the display ring drops frames.
        let oldest = voice.at.saturating_sub(u64::from(self.rate) * 4);
        while self.frames.front().is_some_and(|frame| frame.at <= oldest)
            || self.frames.len() >= self.capacity
        {
            if self.frames.pop_front().is_none() {
                break;
            }
        }
        self.frames.push_back(voice);
    }

    pub(crate) fn recentre(&mut self) {
        self.median.clear();
        self.median.extend(
            self.frames
                .iter()
                .filter(|voice| voice.voiced)
                .map(|voice| f64::from(voice.volts) * 12.0),
        );
        if self.median.is_empty() {
            return;
        }
        self.median.sort_unstable_by(f64::total_cmp);
        let upper = self.median.len() / 2;
        let lower = (self.median.len() - 1) / 2;
        let centre = (self.median[lower] + self.median[upper]) * 0.5;
        if !self.centred || (centre - self.centre).abs() > 3.0 {
            self.centre = centre;
            self.centred = true;
        }
    }

    pub(super) fn points(&self, snapped: bool) -> Vec<(f64, f64)> {
        let newest = self.latest.map_or(0, |voice| voice.at);
        self.frames
            .iter()
            .filter(|voice| voice.voiced)
            .map(|voice| {
                let seconds_ago = newest.saturating_sub(voice.at) as f64 / f64::from(self.rate);
                let pitch = if snapped {
                    voice.snapped_volts
                } else {
                    voice.volts
                };
                (4.0 - seconds_ago, f64::from(pitch) * 12.0)
            })
            .collect()
    }
}

pub(super) fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    display: &Display,
    settings: QuantiseSettings,
) {
    let low = display.centre - 7.0;
    let high = display.centre + 7.0;
    let direct = display.points(false);
    let snapped = display.points(true);
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, 4.0])
        .y_bounds([low, high])
        .paint(|context| {
            if settings.blend > 0.0 {
                for note in low.ceil() as i32..=high.floor() as i32 {
                    let degree = (note - i32::from(settings.root)).rem_euclid(12) as u8;
                    if SCALES[settings.scale].contains(&degree) {
                        let height = f64::from(note);
                        context.draw(&Line::new(0.0, height, 4.0, height, SCALE_LINE));
                        if degree == 0 {
                            context.print(
                                0.0,
                                height,
                                Span::styled(pitch::note_name(height), Style::default().fg(DIM)),
                            );
                        }
                    }
                }
                context.layer();
            }
            context.draw(&Points::new(&direct, DIM));
            context.layer();
            context.draw(&Points::new(&snapped, BRIGHT));
        })
        .background_color(Color::Black);
    frame.render_widget(canvas, area);
}
