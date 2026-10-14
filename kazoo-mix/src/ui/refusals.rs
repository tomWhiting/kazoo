use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

pub(super) fn area(screen: Rect, count: usize) -> Rect {
    let rows = u16::try_from(count)
        .unwrap_or(u16::MAX)
        .min(screen.height.saturating_sub(12));
    Rect::new(
        screen.x,
        screen.bottom().saturating_sub(rows + 1),
        screen.width,
        rows,
    )
}

pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, lines: &[String]) {
    for (row, line) in (area.y..area.bottom()).zip(lines) {
        frame.render_widget(
            Paragraph::new(line.as_str()).style(Style::new().fg(super::AMBER)),
            Rect::new(area.x, row, area.width, 1),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn refused_reasons_render_below_the_strips() {
        let mut terminal = Terminal::new(TestBackend::new(140, 34)).unwrap();
        let mut desk = crate::desk::Desk::new();
        let shared = crate::shared::SharedState::with_tempo(120.0);
        let status = super::super::StatusInfo {
            sample_rate: 24_000,
            channels: 2,
            buffer_frames: None,
            note: String::new(),
            note_alert: false,
            stream_errors: 0,
            engine_faults: 0,
            mouse: false,
            hub_priority: super::super::HubPriority::Asking,
        };
        let lines = [
            "refused: mouth (48000 Hz, the desk runs at 24000 Hz)".to_owned(),
            "refused: wall (desk full)".to_owned(),
        ];
        terminal
            .draw(|frame| {
                super::super::draw_with_refusals(frame, &mut desk, &shared, &status, &lines);
            })
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        for line in &lines {
            assert!(text.contains(line));
        }
        let refused = area(Rect::new(0, 0, 140, 34), lines.len());
        assert!(
            desk.hits
                .iter()
                .all(|hit| hit.area.intersection(refused).is_empty())
        );
    }
}
