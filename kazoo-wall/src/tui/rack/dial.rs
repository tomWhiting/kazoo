//! A knob drawn as a round dial in braille: an arc (or, for a stepped knob,
//! a ring of ticks like a rotary switch's detents) sweeping from 7 o'clock
//! to 5 o'clock, a pointer from the centre to the knob's position, and a
//! marker where a glide is heading.
//!
//! A dial is [`COLS`] × [`ROWS`] cells, each cell 2 × 4 braille dots, so
//! the dot grid is square and the pointer's angle reads true. The compact
//! rack draws a smaller one ([`small`]), [`SMALL_COLS`] × [`SMALL_ROWS`]
//! cells: 8 × 8 dots, still round.

/// Cells across.
pub const COLS: usize = 6;

/// Cells down.
pub const ROWS: usize = 3;

/// Cells across a small dial.
pub const SMALL_COLS: usize = 4;

/// Cells down a small dial.
pub const SMALL_ROWS: usize = 2;

/// The most dots across or down any dial has.
const MOST_DOTS: usize = 12;

const _: () = assert!(COLS * 2 <= MOST_DOTS && ROWS * 4 <= MOST_DOTS);
const _: () = assert!(SMALL_COLS * 2 <= MOST_DOTS && SMALL_ROWS * 4 <= MOST_DOTS);

/// The sweep's start, in degrees anticlockwise from 3 o'clock: 7 o'clock.
const START: f64 = 240.0;

/// The sweep's length, clockwise: 7 o'clock round to 5 o'clock.
const SWEEP: f64 = 300.0;

/// The proportions of a dial, in dots.
#[derive(Debug, Clone, Copy)]
struct Size {
    /// Dots across and down.
    dots_x: usize,
    dots_y: usize,
    /// Radius of the arc.
    ring: f64,
    /// Length of the pointer.
    pointer: f64,
    /// Where the pointer starts, from the centre: the dot grid's centre
    /// falls between dots, and starting just off it keeps the pointer on
    /// its own side of the dial.
    hub: f64,
    /// Most detents drawn as separate ticks; more read as an arc.
    most_ticks: usize,
}

/// The full dial.
const FULL: Size = Size {
    dots_x: COLS * 2,
    dots_y: ROWS * 4,
    ring: 5.2,
    pointer: 4.4,
    hub: 1.0,
    most_ticks: 24,
};

/// The small dial: fewer dots round the ring, so fewer ticks read apart.
const SMALL: Size = Size {
    dots_x: SMALL_COLS * 2,
    dots_y: SMALL_ROWS * 4,
    ring: 3.4,
    pointer: 3.0,
    hub: 0.5,
    most_ticks: 12,
};

/// What a cell of the dial shows most.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// Nothing.
    Blank,
    /// The arc or ticks.
    Ring,
    /// The glide's destination.
    Marker,
    /// The pointer.
    Pointer,
}

/// One cell: its braille character and what it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// The braille character (U+2800 for none).
    pub glyph: char,
    /// What it shows, for its colour.
    pub part: Part,
}

/// A grid of dots, as big as the biggest dial (a smaller dial uses its
/// top-left).
type Grid = [[bool; MOST_DOTS]; MOST_DOTS];

/// The dots of a dial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Dots {
    ring: Grid,
    marker: Grid,
    pointer: Grid,
}

/// The braille bit for dot `(dx, dy)` of a cell.
#[must_use]
pub const fn braille_bit(dx: usize, dy: usize) -> u8 {
    const BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
    BITS[dy % 4][dx % 2]
}

/// The braille character for a cell's dot bits.
#[must_use]
pub fn braille(bits: u8) -> char {
    char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ')
}

/// The angle of `position` (0 to 1) along the sweep, in radians
/// anticlockwise from 3 o'clock.
#[must_use]
pub fn angle(position: f64) -> f64 {
    let position = if position.is_finite() {
        position.clamp(0.0, 1.0)
    } else {
        0.0
    };
    SWEEP.mul_add(-position, START).to_radians()
}

fn set(grid: &mut Grid, size: Size, x: f64, y: f64) {
    let (x, y) = (x.round(), y.round());
    if x >= 0.0 && y >= 0.0 && (x as usize) < size.dots_x && (y as usize) < size.dots_y {
        grid[y as usize][x as usize] = true;
    }
}

/// The dot at `radius` along `angle` from the centre of a dial of `size`.
fn at(size: Size, angle: f64, radius: f64) -> (f64, f64) {
    let centre_x = (size.dots_x as f64 - 1.0) / 2.0;
    let centre_y = (size.dots_y as f64 - 1.0) / 2.0;
    (
        radius.mul_add(angle.cos(), centre_x),
        (-radius).mul_add(angle.sin(), centre_y),
    )
}

/// A dial at `position` (0 to 1). `steps` is the number of positions of a
/// stepped knob (drawn as ticks when there are few enough); `target` is
/// where a glide is heading, if it is not where the knob is.
#[must_use]
pub fn dial(position: f64, steps: Option<usize>, target: Option<f64>) -> [[Cell; COLS]; ROWS] {
    cells(&dots(FULL, position, steps, target))
}

/// The compact rack's small dial, drawn as [`dial`] is.
#[must_use]
pub fn small(
    position: f64,
    steps: Option<usize>,
    target: Option<f64>,
) -> [[Cell; SMALL_COLS]; SMALL_ROWS] {
    cells(&dots(SMALL, position, steps, target))
}

fn dots(size: Size, position: f64, steps: Option<usize>, target: Option<f64>) -> Dots {
    let mut dots = Dots {
        ring: [[false; MOST_DOTS]; MOST_DOTS],
        marker: [[false; MOST_DOTS]; MOST_DOTS],
        pointer: [[false; MOST_DOTS]; MOST_DOTS],
    };
    match steps.filter(|steps| (2..=size.most_ticks).contains(steps)) {
        Some(steps) => {
            for step in 0..steps {
                let (x, y) = at(size, angle(step as f64 / (steps - 1) as f64), size.ring);
                set(&mut dots.ring, size, x, y);
            }
        }
        None => {
            for degree in 0..=(SWEEP as u32 / 3) {
                let (x, y) = at(size, angle(f64::from(degree) * 3.0 / SWEEP), size.ring);
                set(&mut dots.ring, size, x, y);
            }
        }
    }
    if let Some(target) = target {
        let (x, y) = at(size, angle(target), size.ring);
        set(&mut dots.marker, size, x, y);
    }
    let pointer = angle(position);
    for quarter in ((size.hub * 4.0) as u32)..=((size.pointer * 4.0) as u32) {
        let (x, y) = at(size, pointer, f64::from(quarter) / 4.0);
        set(&mut dots.pointer, size, x, y);
    }
    dots
}

fn cells<const C: usize, const R: usize>(dots: &Dots) -> [[Cell; C]; R] {
    let mut out = [[Cell {
        glyph: braille(0),
        part: Part::Blank,
    }; C]; R];
    for (row, line) in out.iter_mut().enumerate() {
        for (col, cell) in line.iter_mut().enumerate() {
            let mut bits = 0_u8;
            let mut part = Part::Blank;
            for dy in 0..4 {
                for dx in 0..2 {
                    let (x, y) = (col * 2 + dx, row * 4 + dy);
                    let layers = [
                        (dots.ring[y][x], Part::Ring),
                        (dots.marker[y][x], Part::Marker),
                        (dots.pointer[y][x], Part::Pointer),
                    ];
                    for (on, layer) in layers {
                        if on {
                            bits |= braille_bit(dx, dy);
                            part = strongest(part, layer);
                        }
                    }
                }
            }
            *cell = Cell {
                glyph: braille(bits),
                part,
            };
        }
    }
    out
}

/// The part a cell is coloured by when it shows several.
const fn strongest(a: Part, b: Part) -> Part {
    const fn rank(part: Part) -> u8 {
        match part {
            Part::Blank => 0,
            Part::Ring => 1,
            Part::Marker => 2,
            Part::Pointer => 3,
        }
    }
    if rank(b) > rank(a) { b } else { a }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cells holding the pointer.
    fn pointer_cells<const C: usize, const R: usize>(
        cells: &[[Cell; C]; R],
    ) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        for (row, line) in cells.iter().enumerate() {
            for (col, cell) in line.iter().enumerate() {
                if cell.part == Part::Pointer {
                    found.push((col, row));
                }
            }
        }
        found
    }

    #[test]
    fn the_pointer_sweeps_from_seven_to_five_oclock() {
        let low = pointer_cells(&dial(0.0, None, None));
        assert!(low.contains(&(1, 2)), "7 o'clock points down-left: {low:?}");
        assert!(!low.iter().any(|&(col, _)| col >= 3));
        let high = pointer_cells(&dial(1.0, None, None));
        assert!(
            high.contains(&(4, 2)),
            "5 o'clock points down-right: {high:?}"
        );
        assert!(!high.iter().any(|&(col, _)| col <= 2));
        let middle = pointer_cells(&dial(0.5, None, None));
        assert!(
            middle.iter().all(|&(_, row)| row <= 1),
            "noon points up: {middle:?}"
        );
        assert!(middle.contains(&(2, 0)) || middle.contains(&(3, 0)));
        // Out of range and not a number stay on the sweep.
        assert_eq!(dial(-3.0, None, None), dial(0.0, None, None));
        assert_eq!(dial(f64::NAN, None, None), dial(0.0, None, None));
    }

    #[test]
    fn the_arc_leaves_the_bottom_open_and_switches_show_detents() {
        let arc = dial(0.5, None, None);
        // The gap between 5 and 7 o'clock: the bottom middle is empty.
        assert_eq!(arc[2][2].part, Part::Blank);
        assert_eq!(arc[2][3].part, Part::Blank);
        assert_eq!(arc[0][0].part, Part::Ring);
        let switch = dial(0.5, Some(4), None);
        let ring_dots: u32 = switch
            .iter()
            .flatten()
            .filter(|cell| cell.part == Part::Ring)
            .map(|cell| (u32::from(cell.glyph) - 0x2800).count_ones())
            .sum();
        assert!(ring_dots <= 4, "four detents, not an arc: {ring_dots}");
    }

    #[test]
    fn a_glide_marks_its_destination() {
        let gliding = dial(0.0, None, Some(1.0));
        assert!(
            gliding
                .iter()
                .flatten()
                .any(|cell| cell.part == Part::Marker)
        );
        assert!(
            !dial(0.0, None, None)
                .iter()
                .flatten()
                .any(|cell| cell.part == Part::Marker)
        );
    }

    #[test]
    fn braille_bits_are_the_unicode_layout() {
        assert_eq!(braille(0), '\u{2800}');
        assert_eq!(braille(0xff), '\u{28ff}');
        assert_eq!(braille_bit(0, 0), 0x01);
        assert_eq!(braille_bit(1, 3), 0x80);
        assert_eq!(braille_bit(0, 3), 0x40);
    }

    #[test]
    fn the_small_dial_is_round_and_reads_the_same_way() {
        let low = pointer_cells(&small(0.0, None, None));
        assert!(low.contains(&(1, 1)), "7 o'clock points down-left: {low:?}");
        assert!(!low.iter().any(|&(col, _)| col >= 3));
        let high = pointer_cells(&small(1.0, None, None));
        assert!(
            high.contains(&(2, 1)),
            "5 o'clock points down-right: {high:?}"
        );
        assert!(!high.iter().any(|&(col, _)| col == 0));
        let middle = pointer_cells(&small(0.5, None, None));
        assert!(
            middle.iter().all(|&(_, row)| row == 0),
            "noon points up: {middle:?}"
        );
        // The ring reaches every corner cell but leaves the bottom open.
        let arc = small(0.5, None, None);
        assert_eq!(arc[0][0].part, Part::Ring);
        assert_eq!(arc[0][3].part, Part::Ring);
        let bottom: u32 = [arc[1][1], arc[1][2]]
            .iter()
            .filter(|cell| cell.part == Part::Ring)
            .map(|cell| (u32::from(cell.glyph) - 0x2800) & 0xC0)
            .sum();
        assert_eq!(bottom, 0, "no ring on the bottom dots between 5 and 7");
        // Detents, a marker, and nonsense held to the sweep.
        let switch = small(0.0, Some(4), None);
        let ring_dots: u32 = switch
            .iter()
            .flatten()
            .filter(|cell| cell.part == Part::Ring)
            .map(|cell| (u32::from(cell.glyph) - 0x2800).count_ones())
            .sum();
        assert!(ring_dots <= 4, "four detents: {ring_dots}");
        assert!(
            small(0.0, None, Some(1.0))
                .iter()
                .flatten()
                .any(|cell| cell.part == Part::Marker)
        );
        assert_eq!(small(f64::NAN, None, None), small(0.0, None, None));
    }
}
