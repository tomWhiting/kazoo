use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

/// The rows the strips need to show their controls; refusals never take them.
const STRIPS_NEED: u16 = super::STRIP_TOP_ROWS + 3 + 2;

pub(super) fn area(screen: Rect, count: usize) -> Rect {
    // Header and footer, then the strips, then as many refusals as fit.
    let rows = u16::try_from(count)
        .unwrap_or(u16::MAX)
        .min(screen.height.saturating_sub(2 + STRIPS_NEED));
    Rect::new(
        screen.x,
        screen.bottom().saturating_sub(rows + 1),
        screen.width,
        rows,
    )
}

pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, lines: &[String]) {
    for (row, line) in (area.y..area.bottom()).zip(fitted(lines, usize::from(area.height))) {
        frame.render_widget(
            Paragraph::new(line).style(Style::new().fg(super::AMBER)),
            Rect::new(area.x, row, area.width, 1),
        );
    }
}

/// The lines for `rows` rows, newest first. When they do not all fit, the
/// last row counts the ones not shown; with one row, the newest keeps its
/// reason and carries the count.
fn fitted(lines: &[String], rows: usize) -> Vec<String> {
    if lines.len() <= rows {
        return lines.to_vec();
    }
    match rows {
        0 => Vec::new(),
        1 => vec![format!(
            "{} (and {} more refused)",
            lines[0],
            lines.len() - 1
        )],
        _ => {
            let shown = rows - 1;
            let mut fitted = lines[..shown].to_vec();
            fitted.push(format!("and {} more refused", lines.len() - shown));
            fitted
        }
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

    #[test]
    fn refusals_never_take_the_rows_the_strips_need() {
        let mut terminal = Terminal::new(TestBackend::new(140, super::super::MIN_HEIGHT)).unwrap();
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
            "refused: wall (48000 Hz, the desk runs at 24000 Hz)".to_owned(),
            "refused: dx (48000 Hz, the desk runs at 24000 Hz)".to_owned(),
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
        assert!(
            text.contains(
                "refused: mouth (48000 Hz, the desk runs at 24000 Hz) (and 2 more refused)"
            ),
            "{text}"
        );
        assert!(
            desk.hits
                .iter()
                .any(|hit| matches!(hit.target, crate::desk::HitTarget::Strip { .. })),
            "the strips keep their controls"
        );
        let mut terminal =
            Terminal::new(TestBackend::new(140, super::super::MIN_HEIGHT + 1)).unwrap();
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
        assert!(text.contains(&lines[0]), "{text}");
        assert!(!text.contains(&lines[1]), "{text}");
        assert!(text.contains("and 2 more refused"), "{text}");
    }
}
