//! Drawing the console: a header with the clock, the seats, the master
//! meter and what the wall sounds like; the wall itself as panels on a
//! rack; the cable list down the right; the live log along the bottom; and
//! a status line that answers every key.
//!
//! In the rack view ([`rack`]) the wall takes the whole width, as
//! faceplates with dials and hanging cables, and the log is shorter (or
//! put away with `f`).
//!
//! Everything is drawn from [`App`] alone, so the same frame can be drawn
//! into ratatui's test backend.

mod overlay;
mod panes;
mod rack;
mod wall;

use std::time::Instant;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use kazoo_wall::protocol::Snapshot;

use super::app::{App, Mode, View};
use super::theme::{AMBER, RACK, TEXT, TEXT_DIM};

/// Narrowest terminal the console draws into.
pub const MIN_WIDTH: u16 = 80;

/// Shortest terminal the console draws into.
pub const MIN_HEIGHT: u16 = 24;

/// Lines of header.
const HEADER: u16 = 2;

/// Draw the whole console. `unix_now` (Unix seconds) dates the log.
pub fn draw(frame: &mut Frame<'_>, app: &mut App, now: Instant, unix_now: Option<i64>) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    buf.set_style(area, Style::new().bg(RACK).fg(TEXT));
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(buf, area);
        return;
    }
    let header = Rect::new(area.x, area.y, area.width, HEADER);
    let status = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    let middle = area.height - HEADER - 1;
    let log_height = match app.view() {
        _ if !app.log_shown() => 0,
        View::List => (middle / 4).clamp(5, 12),
        View::Rack => (middle / 6).clamp(4, 8),
    };
    let log = Rect::new(area.x, status.y - log_height, area.width, log_height);
    let body = Rect::new(area.x, area.y + HEADER, area.width, middle - log_height);
    let side_width = if area.width >= 150 {
        42
    } else if area.width >= 110 {
        34
    } else {
        28
    };
    let side = Rect::new(body.right() - side_width, body.y, side_width, body.height);
    let rack = Rect::new(body.x + 1, body.y, body.width - side_width - 2, body.height);

    panes::header(buf, header, app);
    match app.view() {
        View::List => {
            wall::wall(buf, rack, app);
            panes::cables(buf, side, app);
        }
        View::Rack => rack::rack(buf, body, app),
    }
    if log_height > 0 {
        panes::log(buf, log, app, unix_now);
    }
    panes::status(buf, status, app, now);

    match app.mode() {
        Mode::Help { scroll } => overlay::help(buf, area, *scroll),
        Mode::Add {
            index,
            filter,
            place,
        } => overlay::picker(buf, area, app, (*index, filter, place.as_ref())),
        Mode::Remove { module, cables } => overlay::confirm_remove(buf, area, module, *cables),
        Mode::Stop => overlay::confirm_stop(buf, area),
        Mode::Normal
        | Mode::Name { .. }
        | Mode::Patch(_)
        | Mode::Tempo { .. }
        | Mode::Value { .. }
        | Mode::Jump { .. } => {}
    }
}

/// The peak levels of `out` module `id`, left and right in dBFS. The
/// wall does not report levels per `out` yet (only the master's, in the
/// header), so every `out` meter shows `--` rather than a guess.
const fn out_levels(_snapshot: &Snapshot, _id: &str) -> Option<(f64, f64)> {
    None
}

// ---------------------------------------------------------------------------
// Buffer helpers
// ---------------------------------------------------------------------------

/// Write `text` at `(x, y)`, clipped to `limit`. Returns the column after
/// the last cell written.
fn put(buf: &mut Buffer, limit: Rect, x: u16, y: u16, text: &str, style: Style) -> u16 {
    if y < limit.y || y >= limit.bottom() || x >= limit.right() || x < limit.x {
        return x;
    }
    let width = usize::from(limit.right() - x);
    let (end, _) = buf.set_stringn(x, y, text, width, style);
    end
}

/// Write `text` so it ends at the right edge of `limit` on row `y`.
fn put_right(buf: &mut Buffer, limit: Rect, y: u16, text: &str, style: Style) -> u16 {
    let width = text_width(text).min(limit.width);
    let x = limit.right() - width;
    put(buf, limit, x, y, text, style);
    x
}

/// Write `text` centred in `limit` on row `y`.
fn put_centred(buf: &mut Buffer, limit: Rect, y: u16, text: &str, style: Style) {
    let width = text_width(text).min(limit.width);
    put(
        buf,
        limit,
        limit.x + (limit.width - width) / 2,
        y,
        text,
        style,
    );
}

/// Fill row `y` of `limit` with `style`'s background.
fn fill(buf: &mut Buffer, limit: Rect, y: u16, style: Style) {
    if y >= limit.y && y < limit.bottom() {
        buf.set_style(Rect::new(limit.x, y, limit.width, 1), style);
        for x in limit.x..limit.right() {
            buf[(x, y)].set_symbol(" ");
        }
    }
}

/// Columns `text` takes.
fn text_width(text: &str) -> u16 {
    u16::try_from(text.chars().count()).unwrap_or(u16::MAX)
}

/// `text` cut to `width` columns, ending in `…` when cut.
fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut cut: String = text.chars().take(width - 1).collect();
    cut.push('…');
    cut
}

fn draw_too_small(buf: &mut Buffer, area: Rect) {
    let y = area.y + area.height / 2;
    let lines = [
        (
            format!(
                "the wall needs {MIN_WIDTH}×{MIN_HEIGHT}; this window is {}×{}",
                area.width, area.height
            ),
            Style::new().fg(AMBER).bg(RACK),
        ),
        (
            "make the window bigger to see it".to_string(),
            Style::new().fg(TEXT).bg(RACK),
        ),
        (
            "q quits (the wall keeps playing)".to_string(),
            Style::new().fg(TEXT_DIM).bg(RACK),
        ),
    ];
    for (row, (text, style)) in lines.iter().enumerate() {
        let row = u16::try_from(row).unwrap_or(0);
        let line_y = y.saturating_sub(1) + row;
        if line_y < area.bottom() {
            put_centred(
                buf,
                area,
                line_y,
                &truncate(text, usize::from(area.width)),
                *style,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_is_clean() {
        assert_eq!(truncate("oscillator", 20), "oscillator");
        assert_eq!(truncate("oscillator", 5), "osci…");
        assert_eq!(truncate("oscillator", 1), "…");
        assert_eq!(truncate("oscillator", 0), "");
    }
}
