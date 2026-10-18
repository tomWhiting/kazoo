use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::super::rack::fold::Tile;
use super::super::rack::geometry::{Plate, Point};
use super::super::theme::{AMBER, CREAM, INK, INK_DIM, bold, plain};

pub fn tile(buf: &mut Buffer, area: Rect, pan: Point, plate: &Plate, tile: &Tile, selected: bool) {
    let edge = if selected {
        bold(AMBER, CREAM)
    } else {
        plain(INK_DIM, CREAM)
    };
    let inner = plate.width.saturating_sub(2);
    let put = |buf: &mut Buffer, x: u32, y: u32, text: &str, style| {
        for (offset, ch) in text.chars().enumerate() {
            let point = Point {
                x: x + u32::try_from(offset).unwrap_or(u32::MAX / 2),
                y,
            };
            if let (Some(x), Some(y)) = (point.x.checked_sub(pan.x), point.y.checked_sub(pan.y))
                && x < u32::from(area.width)
                && y < u32::from(area.height)
            {
                buf[(area.x + x as u16, area.y + y as u16)]
                    .set_char(ch)
                    .set_style(style);
            }
        }
    };
    let rule = "─".repeat(usize::from(inner));
    let middle = format!("│{}│", "⠀".repeat(usize::from(inner)));
    for row in 0..plate.height {
        let line = if row == 0 {
            format!("┌{rule}┐")
        } else if row + 1 == plate.height {
            format!("└{rule}┘")
        } else {
            middle.clone()
        };
        put(buf, plate.at.x, plate.at.y + u32::from(row), &line, edge);
    }
    let count = format!("{} modules", tile.members.len());
    for (row, text) in [
        (1, tile.group.as_str()),
        (2, count.as_str()),
        (4, "Tab unfolds"),
    ] {
        put(
            buf,
            plate.at.x + 2,
            plate.at.y + row,
            text,
            plain(INK, CREAM),
        );
    }
    put(buf, plate.at.x, plate.at.y + 3, "●", edge);
    put(
        buf,
        plate.at.x + u32::from(plate.width) - 1,
        plate.at.y + 3,
        "●",
        edge,
    );
}
