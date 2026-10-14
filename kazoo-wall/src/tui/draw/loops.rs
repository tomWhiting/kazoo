use ratatui::text::Span;

use crate::protocol::{LoopState, LoopView};

use super::super::theme::{AMBER, CREAM, INK, RED, SAGE, TEXT_DIM, plain};

/// The tape's lamp, length, pass, written bars and recording source.
#[must_use]
pub fn loop_line(view: &LoopView, tempo: f64, width: u16) -> Vec<Span<'static>> {
    let colour = match view.state {
        LoopState::Rolling | LoopState::PostRoll => RED,
        LoopState::Armed | LoopState::Loading => AMBER,
        LoopState::Holding => SAGE,
        LoopState::Blank | LoopState::NoTape => TEXT_DIM,
    };
    let bars = if view.beats.is_finite() && tempo.is_finite() && tempo > 0.0 {
        (view.beats / 4.0).ceil().max(0.0) as u64
    } else {
        0
    };
    let digits = bars.to_string().len();
    let place = format!("{:>digits$}/{bars} ", u64::from(bars > 0));
    let label = format!("{bars}b ");
    let pass = format!("p{} ", view.pass);
    let source = view
        .from
        .as_ref()
        .map_or_else(String::new, |from| format!(" ← {from}"));
    let used = 2
        + label.chars().count()
        + place.chars().count()
        + pass.chars().count()
        + source.chars().count();
    let written: String = view
        .written
        .iter()
        .take(usize::from(width).saturating_sub(used))
        .map(|written| if *written { '▮' } else { '▯' })
        .collect();
    vec![
        Span::styled("● ", plain(colour, CREAM)),
        Span::styled(label, plain(INK, CREAM)),
        Span::styled(place, plain(INK, CREAM)),
        Span::styled(pass, plain(INK, CREAM)),
        Span::styled(written, plain(SAGE, CREAM)),
        Span::styled(source, plain(INK, CREAM)),
    ]
}

pub fn loop_line_at(view: &LoopView, tempo: f64, width: u16, beat: f64) -> Vec<Span<'static>> {
    let mut spans = loop_line(view, tempo, width);
    if view.beats.is_finite() && view.beats > 0.0 && view.anchor.is_finite() && beat.is_finite() {
        let bars = (view.beats / 4.0).ceil() as u64;
        let bar = ((beat - view.anchor).rem_euclid(view.beats) / 4.0).floor() as u64 + 1;
        let digits = bars.to_string().len();
        spans[2] = Span::styled(format!("{bar:>digits$}/{bars} "), plain(INK, CREAM));
    }
    spans
}
