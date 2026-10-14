use std::sync::atomic::Ordering;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::controls::App;
use crate::engine::looper::{LoopGauges, State};

pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let Some(gauges) = app.loop_gauges() else {
        frame.render_widget(
            Paragraph::new("◐◑  blank: r records  free  layers 0  vol 80%"),
            area,
        );
        return;
    };
    let rate = f64::from(app.loop_sample_rate);
    let n = gauges.n.load(Ordering::Relaxed);
    let head = gauges.head.load(Ordering::Relaxed);
    let layers = gauges.layers.load(Ordering::Relaxed);
    let bars = gauges.bars.load(Ordering::Relaxed);
    let bpm = f64::from_bits(gauges.loop_bpm.load(Ordering::Relaxed));
    let glyphs = ['◐', '◓', '◑', '◒'];
    let step = if n > 0 { head.saturating_mul(8) / n } else { 0 };
    let reels = format!("{}{}", glyphs[step % 4], glyphs[(step + 2) % 4]);
    let status = status(gauges, rate);
    let mode = if bars == 0 {
        "free".into()
    } else {
        format!(
            "{bars} bars ♩{:.1}",
            if bpm > 0.0 {
                bpm
            } else {
                app.desk_bpm.unwrap_or(app.accepted.own_bpm)
            }
        )
    };
    let mut spans = vec![Span::raw(format!("{reels}  {status}  {mode}"))];
    if gauges.off_tempo.load(Ordering::Relaxed) {
        spans.push(Span::styled(
            format!(
                " · desk {:.1}",
                app.desk_bpm.unwrap_or(app.accepted.own_bpm)
            ),
            Style::default().fg(Color::Yellow),
        ));
    }
    spans.push(Span::raw(format!(
        "  layers {layers}  vol {:.0}%",
        app.accepted.loop_volume * 100.0
    )));
    if gauges.merging.load(Ordering::Relaxed) {
        spans.push(Span::raw("  merging"));
    }
    if gauges.saving.load(Ordering::Relaxed) {
        spans.push(Span::raw("  saving"));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(area.x, area.y, area.width, 1),
    );
    coverage(frame, area, gauges);
}

fn status(gauges: &LoopGauges, rate: f64) -> String {
    let state = gauges.state.load(Ordering::Relaxed);
    let taken = gauges.taken.load(Ordering::Relaxed);
    let place = format!(
        "{:.2} / {:.2} s",
        gauges.head.load(Ordering::Relaxed) as f64 / rate,
        gauges.n.load(Ordering::Relaxed) as f64 / rate
    );
    if state == State::Blank as u8 {
        "blank: r records".into()
    } else if state == State::Armed as u8 {
        let beat = f64::from_bits(gauges.beat_in_bar.load(Ordering::Relaxed));
        format!(
            "armed: next bar {:.0}.{}",
            (beat / 4.0).floor() + 1.0,
            (beat.rem_euclid(4.0).floor() as u8) + 1
        )
    } else if state == State::Taking as u8 {
        format!("● rec {:.2} s", taken as f64 / rate)
    } else if state == State::Playing as u8 {
        format!("▶ {place}")
    } else if state == State::Dubbing as u8 {
        format!("● dub {place}")
    } else if state == State::Waiting as u8 {
        "waiting: merging".into()
    } else if state == State::Stopped as u8 {
        "■ stopped".into()
    } else {
        "no tape: the loop could not get its memory".into()
    }
}

fn coverage(frame: &mut Frame<'_>, area: Rect, gauges: &LoopGauges) {
    let state = gauges.state.load(Ordering::Relaxed);
    let bars = gauges.bars.load(Ordering::Relaxed);
    let n = gauges.n.load(Ordering::Relaxed);
    let head = gauges.head.load(Ordering::Relaxed);
    let taken = gauges.taken.load(Ordering::Relaxed);
    let layers = gauges.layers.load(Ordering::Relaxed);
    let taking = state == State::Taking as u8;
    let span = if taking && bars == 0 {
        gauges.capacity.load(Ordering::Relaxed)
    } else {
        n
    };
    if span == 0 || area.height < 2 {
        return;
    }
    let arcs: [(usize, usize); 4] = std::array::from_fn(|index| {
        (
            gauges.arcs[index][0].load(Ordering::Relaxed) as usize,
            gauges.arcs[index][1].load(Ordering::Relaxed) as usize,
        )
    });
    for column in 0..area.width {
        let cell = (usize::from(column) * 2 + 1) * span / (usize::from(area.width) * 2);
        let count = if taking {
            usize::from(cell < taken)
        } else {
            arcs.iter()
                .take(usize::from(layers))
                .filter(|(start, written)| {
                    *written >= span || (cell + span - (*start % span)) % span < *written
                })
                .count()
        };
        let symbol = [" ", "░", "▒", "▓", "█"][count.min(4)];
        let position = if taking {
            taken.min(span - 1)
        } else {
            head.min(span - 1)
        };
        let reversed = usize::from(column) == position * usize::from(area.width) / span;
        let style = if reversed {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(symbol).style(style),
            Rect::new(area.x + column, area.y + 1, 1, 1),
        );
    }
}
