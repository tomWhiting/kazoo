use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use super::super::app::{App, RowKind, Tone};
use super::super::theme::{BRASS, INK, PANEL, TEXT, TEXT_DIM, bold, plain};
use super::{overlay, truncate};

pub(super) fn mark_prompt(app: &App, text: &str, now: std::time::Instant) -> (String, String) {
    (
        format!("mark this moment as: {text}█"),
        app.status(now)
            .filter(|status| status.tone == Tone::Trouble)
            .map_or_else(
                || "   1 to 64 of A-Z a-z 0-9 space _ . - · ⏎ marks it (marks never move) · Esc cancels".to_string(),
                |status| format!("   {}", status.text),
            ),
    )
}

pub fn tree(buf: &mut Buffer, area: Rect, app: &App, index: usize) {
    let list = app.tree();
    let mut rows = Vec::new();
    let mut position = 0;
    let mut section = None;
    for row in app.tree_rows() {
        let heading = match row.kind {
            RowKind::Branch => Some("branches"),
            RowKind::Mark => Some("marks"),
            RowKind::Recent => Some("recent"),
            _ => None,
        };
        if heading.is_some() && heading != section {
            rows.push((None, heading.unwrap_or_default().to_string(), true));
            section = heading;
        }
        let selected = if row.kind.selectable() {
            let current = position;
            position += 1;
            Some(current)
        } else {
            None
        };
        rows.push((selected, row.label.clone(), false));
    }
    if let Some(problem) = &list.problem {
        rows.insert(0, (None, problem.clone(), false));
    } else if list.result.is_none() {
        let words = if list.fetching {
            "asking the wall…"
        } else {
            "the tree has not arrived; r asks again"
        };
        rows.push((None, words.into(), false));
    }
    let wanted = u16::try_from(rows.len())
        .unwrap_or(u16::MAX - 5)
        .saturating_add(5);
    let rect = overlay::popup(buf, area, 96, wanted);
    let visible = usize::from(rect.height.saturating_sub(5)).max(1);
    let chosen = rows
        .iter()
        .position(|(selected, _, _)| *selected == Some(index))
        .unwrap_or(0);
    let first = (chosen + 2)
        .saturating_sub(visible)
        .min(chosen.saturating_sub(1));
    let width = usize::from(rect.width.saturating_sub(2));
    let mut lines = vec![Line::from("")];
    lines.extend(
        rows.into_iter()
            .skip(first)
            .take(visible)
            .map(|(selected, label, header)| {
                let style = if selected == Some(index) {
                    bold(INK, BRASS)
                } else if header {
                    bold(BRASS, PANEL)
                } else if selected.is_none() {
                    plain(TEXT_DIM, PANEL)
                } else {
                    plain(TEXT, PANEL)
                };
                let lead = if selected == Some(index) {
                    " ▸ "
                } else {
                    "   "
                };
                Line::from(Span::styled(
                    truncate(&format!("{lead}{label}"), width),
                    style,
                ))
            }),
    );
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " j k ↑ ↓ move · ⏎ go · y goes · n stays · r again · B Esc q close · b marks after closing",
        plain(TEXT_DIM, PANEL),
    )));
    Paragraph::new(lines)
        .block(overlay::frame("tree", BRASS))
        .render(rect, buf);
}

pub fn confirm_go(buf: &mut Buffer, area: Rect, label: &str) {
    // Give the question a row for each wrapped part so its keys remain visible.
    let question: Vec<char> = format!("go to {label}?").chars().collect();
    let mut lines: Vec<String> = question
        .chunks(50)
        .map(|part| part.iter().collect())
        .collect();
    lines.push("the wall as it is is kept first; z undoes it".into());
    overlay::question(buf, area, "go", &lines, "y goes · n stays");
}
