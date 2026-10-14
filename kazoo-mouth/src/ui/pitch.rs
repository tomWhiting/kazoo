use kazoo_core::voice::quantise::NOTE_NAMES;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Paragraph;

use super::{BRIGHT, DIM, Display, FONT};

pub(super) fn note_name(semitones: f64) -> String {
    let note = semitones.round() as i32;
    format!(
        "{}{}",
        NOTE_NAMES[note.rem_euclid(12) as usize],
        4 + note.div_euclid(12)
    )
}

pub(super) fn draw(frame: &mut Frame<'_>, area: Rect, display: &Display) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    let voiced = display.latest.is_some_and(|voice| voice.voiced);
    let sung_style = if voiced {
        Style::default().fg(ratatui::style::Color::White)
    } else {
        dim_style()
    };
    let snapped_style = if voiced {
        Style::default().fg(BRIGHT).add_modifier(Modifier::BOLD)
    } else {
        dim_style()
    };
    let (sung, snapped, direct_detail, snapped_detail) = display.held.map_or_else(
        || {
            (
                "--".into(),
                "--".into(),
                "--¢  -- Hz".into(),
                "→ --  -- Hz".into(),
            )
        },
        |voice| {
            let semitones = f64::from(voice.volts) * 12.0;
            let sung = note_name(semitones);
            let snapped = note_name(f64::from(voice.snapped_volts) * 12.0);
            let cents = ((semitones - semitones.round()) * 100.0).round() as i32;
            let direct_detail = format!("{cents:+}¢  {:.1} Hz", voice.hz);
            let snapped_detail = format!("→ {snapped}  {:.1} Hz", voice.snapped_hz);
            (sung, snapped, direct_detail, snapped_detail)
        },
    );
    panel(frame, columns[0], &sung, "sung", &direct_detail, sung_style);
    panel(
        frame,
        columns[1],
        &snapped,
        "snapped",
        &snapped_detail,
        snapped_style,
    );
}

const fn dim_style() -> Style {
    Style::new().fg(DIM).add_modifier(Modifier::DIM)
}

fn panel(frame: &mut Frame<'_>, area: Rect, note: &str, label: &str, detail: &str, style: Style) {
    for (index, character) in note.chars().enumerate() {
        if let Some((_, rows)) = FONT.iter().find(|(glyph, _)| *glyph == character) {
            for (row, bits) in rows.iter().enumerate() {
                for column in 0..3 {
                    if bits & (1 << (2 - column)) != 0 {
                        frame.buffer_mut().set_string(
                            area.x + (index * 4 + column) as u16,
                            area.y + row as u16,
                            "█",
                            style,
                        );
                    }
                }
            }
        }
    }
    let caption = Rect::new(area.x + 18, area.y, area.width - 18, 1);
    frame.render_widget(
        Paragraph::new(format!("{label} {note}")).style(style),
        caption,
    );
    frame.render_widget(
        Paragraph::new(detail).style(style),
        Rect::new(caption.x, area.bottom() - 1, caption.width, 1),
    );
}
