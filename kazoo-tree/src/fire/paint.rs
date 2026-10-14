//! The fire drawn over the tree's own cells.
//!
//! Each cell lights the dots whose heat beats a dither threshold that turns
//! every frame (at the display's refresh the eye blends it into smooth
//! gradients), joined (OR) with the limb's own dots there; where the fire
//! is hot its colour takes the cell, where it is cool the limb keeps its
//! own. The cell's background glows with its mean heat. The selected
//! moment's own (reversed) cell is left as it is. A cell holding a
//! letter (a glyph, a label) keeps it and its colour, and only glows behind
//! it, more softly, so every word stays readable. Only the picture's own
//! area is drawn on: never the details panel, never the footer.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

use super::field::Field;
use super::place::Placement;

/// Each dot's bit in a braille cell, by row then column.
const DOT: [[u32; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
/// Each dot's place in the 2 × 4 ordered dither, `0..8`.
const RANK: [[u32; 2]; 4] = [[0, 4], [6, 2], [1, 5], [7, 3]];
/// Heat to colour, a blackbody's: from the dark through red and orange to
/// a yellowish white.
const STOPS: [(f32, [f32; 3]); 6] = [
    (0.00, [0.0, 0.0, 0.0]),
    (0.18, [70.0, 4.0, 0.0]),
    (0.38, [200.0, 30.0, 0.0]),
    (0.58, [255.0, 110.0, 5.0]),
    (0.80, [255.0, 200.0, 50.0]),
    (1.00, [255.0, 250.0, 215.0]),
];
/// The lit heat at and past which the fire's colour takes a cell from the
/// limb drawn in it.
const TAKES: f32 = 0.3;
/// How bright a background glows, as a share of the heat's colour.
const GLOW: f32 = 0.45;
/// How bright a background glows behind a letter: less, to keep it read.
const GLOW_BEHIND_TEXT: f32 = 0.3;
/// The dimmest glow worth drawing, by its brightest channel: below it the
/// terminal's own background is left alone.
const GLOW_LEAST: f32 = 10.0;

/// The colour of `heat` (`0.0..=1.0`; anything else is held to it).
#[must_use]
pub fn ramp(heat: f32) -> [f32; 3] {
    let heat = if heat.is_finite() {
        heat.clamp(0.0, 1.0)
    } else {
        0.0
    };
    for pair in STOPS.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if heat <= b {
            let t = (heat - a) / (b - a);
            return [0, 1, 2].map(|i| (cb[i] - ca[i]).mul_add(t, ca[i]));
        }
    }
    STOPS[STOPS.len() - 1].1
}

fn rgb(c: [f32; 3], share: f32) -> Color {
    let [r, g, b] = c.map(|v| (v * share).clamp(0.0, 255.0) as u8);
    Color::Rgb(r, g, b)
}

/// Glow behind `cell` as `colour`, unless it already glows brighter (where
/// two fires overlap, the brighter glow stays).
fn brighten_bg(cell: &mut ratatui::buffer::Cell, colour: Color) {
    let level = |c: Color| match c {
        Color::Rgb(r, g, b) => u16::from(r) + u16::from(g) + u16::from(b),
        _ => 0,
    };
    if level(colour) > level(cell.bg) {
        cell.set_bg(colour);
    }
}

/// The threshold a dot's heat must beat to be lit, at dither place `rank`.
fn threshold(rank: u32) -> f32 {
    ((rank as f32 + 0.5) / 8.0).mul_add(0.85, 0.08)
}

/// A cell's dots lit by the fire, their mean heat, and the cell's mean.
///
/// The dither turns by moving the 2 × 4 pattern across the cell, one of its
/// eight places a frame: every frame is an evenly spread pattern, and over
/// eight frames each dot takes every threshold once.
fn cell_heat(field: &Field, origin: (i32, i32), col: i32, row: i32, turn: u32) -> (u32, f32, f32) {
    let (ox, oy) = ((turn % 2) as usize, (turn / 2 % 4) as usize);
    let (mut bits, mut lit, mut lit_n, mut sum) = (0, 0.0, 0.0, 0.0);
    for ((dy, y), dot_bits) in (0..4usize).zip(0i32..).zip(&DOT) {
        for ((dx, x), &bit) in (0..2usize).zip(0i32..).zip(dot_bits) {
            let v = field.at(col * 2 + x - origin.0, row * 4 + y - origin.1);
            sum += v;
            if v > threshold(RANK[(dy + oy) % 4][(dx + ox) % 2]) {
                bits |= bit;
                lit += v;
                lit_n += 1.0;
            }
        }
    }
    let lit = if lit_n > 0.0 { lit / lit_n } else { 0.0 };
    (bits, lit, sum / 8.0)
}

/// Draw `field`, placed at `place` on the picture filling `area`, over
/// what is already drawn there.
pub fn paint(field: &Field, place: &Placement, area: Rect, buf: &mut Buffer) {
    let shape = field.shape();
    let (wide, tall) = (
        i32::try_from(shape.w).unwrap_or(i32::MAX),
        i32::try_from(shape.h).unwrap_or(i32::MAX),
    );
    let (left, top) = place.origin;
    let cols = i32::from(area.width);
    let rows = i32::from(area.height);
    let (c0, c1) = (
        left.div_euclid(2).max(0),
        (left + wide - 1).div_euclid(2).min(cols - 1),
    );
    let (r0, r1) = (
        top.div_euclid(4).max(0),
        (top + tall - 1).div_euclid(4).min(rows - 1),
    );
    let turn = field.frame() % 8;
    for row in r0..=r1 {
        for col in c0..=c1 {
            let (bits, lit, mean) = cell_heat(field, place.origin, col, row, turn);
            let glow = ramp(mean * 0.75);
            let glows = glow.iter().fold(0.0f32, |a, &b| a.max(b)) * GLOW >= GLOW_LEAST;
            if bits == 0 && !glows {
                continue;
            }
            // Inside the area (the range was clipped to it above).
            let (x, y) = (area.x + col as u16, area.y + row as u16);
            if col > 0
                && buf
                    .cell((x - 1, y))
                    .is_some_and(|before| before.symbol().width() > 1)
            {
                // The right half of a wide letter: it shows nothing.
                continue;
            }
            // Off the buffer, there is nothing to draw on.
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            if cell.modifier.contains(Modifier::REVERSED) {
                // The selected moment's own cell keeps its look whole.
                continue;
            }
            let mut letters = cell.symbol().chars();
            let dots = match (letters.next(), letters.next()) {
                (Some(' ') | None, None) => Some(0),
                (Some(c @ '\u{2800}'..='\u{28ff}'), None) => Some(u32::from(c) - 0x2800),
                _ => None,
            };
            if let Some(old) = dots {
                if bits != 0 {
                    // Every code point from U+2800 to U+28FF is a braille
                    // letter, so the cell always takes its new dots.
                    cell.set_char(char::from_u32(0x2800 | ((old | bits) & 0xff)).unwrap_or(' '));
                    if lit >= TAKES || old == 0 {
                        cell.set_fg(rgb(ramp(lit + 0.08), 1.0));
                    }
                }
                if glows {
                    brighten_bg(cell, rgb(glow, GLOW));
                }
            } else if glows {
                brighten_bg(cell, rgb(glow, GLOW_BEHIND_TEXT));
            }
        }
    }
}
