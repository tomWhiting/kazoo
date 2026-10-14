//! The tree drawn in braille.
//!
//! Each terminal cell is 2 dots wide and 4 tall; limbs are thick lines of
//! dots coloured by how they sounded, and the marks hang off them as glyphs
//! with labels. What is drawn depends on how far in you are ([`Level`]).

use unicode_width::UnicodeWidthChar;

use crate::growth::{Growth, ROOT_DEPTH, ROOT_REACH, Step};
use crate::history::{History, Sound};

/// How far in the view is: each level shows more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// The whole tree: limbs, marks, and where the wall is.
    Far,
    /// Marks' names hang off the limbs.
    Near,
    /// Gestures: runs of one seat's same change, each one dot.
    Close,
    /// Every moment a dot, with its words where there is room.
    Every,
}

impl Level {
    /// The level for `scale` dots per world unit.
    #[must_use]
    pub fn of(scale: f64) -> Self {
        if scale < 0.75 {
            Self::Far
        } else if scale < 2.5 {
            Self::Near
        } else if scale < 6.0 {
            Self::Close
        } else {
            Self::Every
        }
    }

    /// Its name, as the footer says it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Far => "far: limbs and marks",
            Self::Near => "near: names",
            Self::Close => "close: gestures",
            Self::Every => "every moment",
        }
    }
}

/// What part of the world the picture shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// The world point at the picture's centre.
    pub centre: (f64, f64),
    /// Braille dots per world unit.
    pub scale: f64,
}

impl View {
    /// Dots from the picture's top left for a world point.
    #[must_use]
    pub fn to_dots(&self, (x, y): (f64, f64), cols: u16, rows: u16) -> (f64, f64) {
        let w = f64::from(cols) * 2.0;
        let h = f64::from(rows) * 4.0;
        (
            (x - self.centre.0).mul_add(self.scale, w / 2.0),
            (y - self.centre.1).mul_add(-self.scale, h / 2.0),
        )
    }

    /// The world point under a cell's middle.
    #[must_use]
    pub fn to_world(&self, col: u16, row: u16, cols: u16, rows: u16) -> (f64, f64) {
        let w = f64::from(cols) * 2.0;
        let h = f64::from(rows) * 4.0;
        let dx = f64::from(col).mul_add(2.0, 1.0);
        let dy = f64::from(row).mul_add(4.0, 2.0);
        (
            (dx - w / 2.0) / self.scale + self.centre.0,
            (h / 2.0 - dy) / self.scale + self.centre.1,
        )
    }

    /// The view that shows all of `bounds` in `cols` × `rows`.
    #[must_use]
    pub fn fit(bounds: (f64, f64, f64, f64), cols: u16, rows: u16) -> Self {
        let width = (bounds.2 - bounds.0).max(1.0);
        let height = (bounds.3 - bounds.1).max(1.0);
        let scale = (f64::from(cols.max(1)) * 2.0 / width)
            .min(f64::from(rows.max(1)) * 4.0 / height)
            * 0.95;
        Self {
            centre: (
                f64::midpoint(bounds.0, bounds.2),
                f64::midpoint(bounds.1, bounds.3),
            ),
            scale,
        }
    }

    /// The world rectangle the picture shows, `(min_x, min_y, max_x,
    /// max_y)`, widened by `pad` dots all round.
    #[must_use]
    pub fn world(&self, cols: u16, rows: u16, pad: f64) -> (f64, f64, f64, f64) {
        let half_w = (f64::from(cols) + pad) / self.scale;
        let half_h = f64::from(rows).mul_add(2.0, pad) / self.scale;
        (
            self.centre.0 - half_w,
            self.centre.1 - half_h,
            self.centre.0 + half_w,
            self.centre.1 + half_h,
        )
    }
}

/// Whether `(x, y)` is inside `rect`.
fn inside(rect: (f64, f64, f64, f64), (x, y): (f64, f64)) -> bool {
    x >= rect.0 && x <= rect.2 && y >= rect.1 && y <= rect.3
}

/// The furthest out the view goes: never so far that the whole tree
/// cannot be seen, however large it grows, and at least this far.
pub const MIN_SCALE: f64 = 0.005;

/// The furthest out the view goes for a tree of `bounds` in `cols` ×
/// `rows`: [`MIN_SCALE`], or further when the tree needs it to be seen
/// whole.
#[must_use]
pub fn min_scale(bounds: (f64, f64, f64, f64), cols: u16, rows: u16) -> f64 {
    View::fit(bounds, cols, rows).scale.min(MIN_SCALE)
}
/// The furthest in.
pub const MAX_SCALE: f64 = 40.0;
/// The share of its width, in dots, a limb keeps however far out.
const FAR_THICKNESS: f64 = 0.35;
/// The widest a limb is ever drawn, in dots, so close in a thick limb does
/// not bury its own marks.
const MAX_DOTS_WIDE: f64 = 8.0;

/// What a glyph is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glyph {
    /// Where the wall is.
    Here,
    /// A bookmark made by hand.
    Mark,
    /// A save's mark.
    Save,
    /// A gesture's end, or a moment.
    Moment,
    /// The first moment of a new day on a limb.
    Day,
}

impl Glyph {
    /// The character drawn.
    #[must_use]
    pub const fn char(self) -> char {
        match self {
            Self::Here => '◉',
            Self::Mark => '◆',
            Self::Save => '▪',
            Self::Moment => '•',
            Self::Day => '╴',
        }
    }
}

/// One terminal cell of the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    /// Braille dots lit, as the low byte of U+2800.
    pub dots: u8,
    /// The colour of what is drawn here.
    pub colour: Option<(u8, u8, u8)>,
    /// A glyph or a label's letter, drawn instead of the dots.
    pub text: Option<char>,
    /// Drawn bright: on the wall's own line, or selected.
    pub bright: bool,
    /// The selected moment's own cell.
    pub selected: bool,
    /// The right half of a wide letter in the cell before: shows nothing
    /// of its own.
    pub tail: bool,
}

impl Cell {
    /// The character this cell shows.
    #[must_use]
    pub fn char(&self) -> char {
        if self.tail {
            return ' ';
        }
        self.text.unwrap_or_else(|| {
            if self.dots == 0 {
                ' '
            } else {
                char::from_u32(0x2800 + u32::from(self.dots)).unwrap_or(' ')
            }
        })
    }
}

/// A drawn picture of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
    /// Row by row.
    pub cells: Vec<Cell>,
}

const BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

impl Picture {
    /// An empty picture.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); usize::from(cols) * usize::from(rows)],
        }
    }

    fn index(&self, col: i32, row: i32) -> Option<usize> {
        // Off the picture to the left or above is simply not drawn.
        if col < 0 || row < 0 || col >= i32::from(self.cols) || row >= i32::from(self.rows) {
            return None;
        }
        let (col, row) = (col.unsigned_abs() as usize, row.unsigned_abs() as usize);
        Some(row * usize::from(self.cols) + col)
    }

    /// The cell at `col`, `row`, if inside.
    #[must_use]
    pub fn at(&self, col: u16, row: u16) -> Option<&Cell> {
        self.index(i32::from(col), i32::from(row))
            .map(|i| &self.cells[i])
    }

    /// The picture as text, row by row: what a seat or a test reads.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::new();
        for row in 0..self.rows {
            let line: String = (0..self.cols)
                .filter_map(|col| self.at(col, row))
                .filter(|cell| !cell.tail)
                .map(Cell::char)
                .collect();
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    fn dot(&mut self, x: f64, y: f64, colour: (u8, u8, u8), bright: bool) {
        if !(x.is_finite() && y.is_finite()) {
            return;
        }
        let (x, y) = (x.floor(), y.floor());
        if x < 0.0 || y < 0.0 {
            return;
        }
        let (xi, yi) = (x as i32, y as i32);
        if let Some(i) = self.index(xi / 2, yi / 4) {
            let cell = &mut self.cells[i];
            cell.dots |= BITS[(xi % 2) as usize][(yi % 4) as usize];
            cell.colour = Some(colour);
            cell.bright |= bright;
        }
    }

    /// A thick line from `from` to `to`, in dots.
    fn line(
        &mut self,
        from: (f64, f64),
        to: (f64, f64),
        wide: f64,
        colour: (u8, u8, u8),
        bright: bool,
    ) {
        let width = f64::from(self.cols) * 2.0;
        let height = f64::from(self.rows) * 4.0;
        let pad = wide + 2.0;
        if from.0.max(to.0) < -pad
            || from.0.min(to.0) > width + pad
            || from.1.max(to.1) < -pad
            || from.1.min(to.1) > height + pad
        {
            return;
        }
        let length = libm::hypot(to.0 - from.0, to.1 - from.1);
        let samples = (length * 2.0).ceil().clamp(1.0, 100_000.0) as u32;
        let radius = (wide / 2.0).max(0.0);
        let reach = radius.ceil() as i32;
        for i in 0..=samples {
            let along = f64::from(i) / f64::from(samples);
            let x = (to.0 - from.0).mul_add(along, from.0);
            let y = (to.1 - from.1).mul_add(along, from.1);
            if reach == 0 {
                self.dot(x, y, colour, bright);
                continue;
            }
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    let (off_x, off_y) = (f64::from(dx), f64::from(dy));
                    if libm::hypot(off_x, off_y) <= radius {
                        self.dot(x + off_x, y + off_y, colour, bright);
                    }
                }
            }
        }
    }

    /// Put a glyph at the cell holding dot point `p`. Answers its cell.
    fn glyph(
        &mut self,
        p: (f64, f64),
        glyph: Glyph,
        colour: (u8, u8, u8),
        selected: bool,
    ) -> Option<(i32, i32)> {
        if !(p.0.is_finite() && p.1.is_finite()) || p.0 < 0.0 || p.1 < 0.0 {
            return None;
        }
        let (col, row) = ((p.0 / 2.0).floor() as i32, (p.1 / 4.0).floor() as i32);
        let i = self.index(col, row)?;
        if self.cells[i].tail {
            // A glyph over a wide letter's right half takes its left half.
            if let Some(left) = self.index(col - 1, row) {
                self.cells[left].text = Some(' ');
            }
        }
        let cell = &mut self.cells[i];
        cell.tail = false;
        cell.text = Some(glyph.char());
        cell.colour = Some(colour);
        cell.bright = glyph != Glyph::Day;
        cell.selected |= selected;
        Some((col, row))
    }

    /// Write `text` from `col`, `row` if every cell it needs is free of
    /// glyphs and other labels, cut with `…` at the picture's edge. A wide
    /// letter takes two cells; a letter with no width is left out. Answers
    /// whether it was written.
    fn label(&mut self, col: i32, row: i32, text: &str, taken: &mut [bool]) -> bool {
        let letters: Vec<(char, i32)> = text
            .chars()
            .filter_map(|ch| match ch.width() {
                Some(1) => Some((ch, 1)),
                Some(2) => Some((ch, 2)),
                _ => None,
            })
            .collect();
        let width: i32 = letters.iter().map(|&(_, w)| w).sum();
        let room = i32::from(self.cols) - col;
        if room < 4 {
            return false;
        }
        let shown = width.min(room);
        for c in (col - 1)..=(col + shown) {
            if let Some(i) = self.index(c, row)
                && taken[i]
            {
                return false;
            }
        }
        let mut c = col;
        for (k, &(ch, w)) in letters.iter().enumerate() {
            let last = k + 1 == letters.len();
            // Cut: the last cell that fits says `…` when more did not fit.
            if c + w > col + shown || (!last && c + w == col + shown && shown < width) {
                if let Some(i) = self.index(c, row)
                    && c < col + shown
                {
                    self.write(i, '…', taken);
                }
                break;
            }
            if let Some(i) = self.index(c, row) {
                self.write(i, ch, taken);
                if w == 2
                    && let Some(j) = self.index(c + 1, row)
                {
                    self.cells[j].text = None;
                    self.cells[j].tail = true;
                    self.cells[j].colour = Some(LABEL);
                    taken[j] = true;
                }
            }
            c += w;
        }
        true
    }

    fn write(&mut self, i: usize, ch: char, taken: &mut [bool]) {
        let cell = &mut self.cells[i];
        cell.text = Some(ch);
        cell.tail = false;
        cell.colour = Some(LABEL);
        cell.bright = false;
        taken[i] = true;
    }
}

/// The colour labels are drawn in.
pub const LABEL: (u8, u8, u8) = (200, 200, 190);
/// Unheard: a limb whose sound was not kept. Grey, never black: not
/// knowing how it sounded is not silence.
pub const UNHEARD: (u8, u8, u8) = (140, 140, 136);
/// Day marks: faint, beside the growth.
const DAY: (u8, u8, u8) = (120, 120, 116);
/// How long growth counts as new and is drawn a little lit: an hour
/// before the newest moment.
const NEWEST_SECONDS: i64 = 3_600;
/// Roots.
const ROOT: (u8, u8, u8) = (110, 90, 70);
/// The wall's own glyph.
const HERE: (u8, u8, u8) = (255, 240, 160);
/// Marks.
const MARK: (u8, u8, u8) = (240, 200, 120);

/// A limb's colour from how it sounded: warm (amber) where the sound sat
/// low, cool (blue) where it was bright; lighter when louder; more vivid
/// when busier.
#[must_use]
pub fn colour_of(sound: Option<Sound>) -> (u8, u8, u8) {
    let Some(sound) = sound else {
        return UNHEARD;
    };
    let centroid = if sound.centroid_hz.is_finite() {
        f64::from(sound.centroid_hz)
    } else {
        500.0
    };
    let bright = ((centroid.max(1.0).log2() - 100_f64.log2()) / (4000_f64.log2() - 100_f64.log2()))
        .clamp(0.0, 1.0);
    let hue = bright.mul_add(190.0, 30.0);
    let rms = if sound.rms_db.is_finite() {
        f64::from(sound.rms_db)
    } else {
        -60.0
    };
    let light = ((rms + 50.0) / 40.0).clamp(0.0, 1.0).mul_add(0.35, 0.35);
    let onsets = if sound.onsets.is_finite() {
        f64::from(sound.onsets)
    } else {
        0.0
    };
    let saturation = (onsets / 6.0).clamp(0.0, 1.0).mul_add(0.55, 0.35);
    hsl(hue, saturation, light)
}

fn hsl(hue: f64, saturation: f64, light: f64) -> (u8, u8, u8) {
    let chroma = (1.0 - 2.0_f64.mul_add(light, -1.0).abs()) * saturation;
    let sector = hue / 60.0;
    let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (red, green, blue) = match sector as u32 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let base = light - chroma / 2.0;
    let to = |v: f64| ((v + base).clamp(0.0, 1.0) * 255.0).round() as u8;
    (to(red), to(green), to(blue))
}

/// A gesture: a run of one seat's moments doing the same thing on one limb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gesture {
    /// Its last moment.
    pub last: usize,
    /// How many moments.
    pub count: usize,
    /// Its words: what the run did.
    pub words: String,
}

/// A word that is a number, with any unit or punctuation stuck to it
/// left off: `720`, `-3.5`, `1.8,`.
fn is_number(word: &str) -> bool {
    word.trim_end_matches(|c: char| !c.is_ascii_digit())
        .parse::<f64>()
        .is_ok()
}

/// What a moment does, for grouping: its words with every number left
/// out, so `Cassio turned vcf1 cutoff to 720 Hz` and `… to 1800 Hz` are one
/// gesture while `… lfo1.out to vcf1.cutoff` and `… to vco2.pitch` are two.
/// Only the seats' changes group; the wall's own moments (a start, a gap,
/// a go) each stand alone.
fn subject(moment: &crate::history::Moment) -> Option<String> {
    (moment.kind == "change").then(|| {
        moment
            .summary
            .split_whitespace()
            .map(|word| if is_number(word) { "#" } else { word })
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Every gesture in the tree, limb by limb.
#[must_use]
pub fn gestures(history: &History) -> Vec<Gesture> {
    let mut out = Vec::new();
    for limb in &history.limbs {
        let mut run: Option<(Option<String>, usize, usize, usize)> = None;
        for &m in &limb.moments {
            let what = subject(&history.moments[m]);
            match &mut run {
                Some((Some(words), _, last, count)) if what.as_ref() == Some(words) => {
                    *last = m;
                    *count += 1;
                }
                _ => {
                    if let Some((_, first, last, count)) = run.take() {
                        out.push(gesture(history, first, last, count));
                    }
                    run = Some((what, m, m, 1));
                }
            }
        }
        if let Some((_, first, last, count)) = run {
            out.push(gesture(history, first, last, count));
        }
    }
    out
}

/// A gesture's words: the last moment's, with each number that moved
/// over the run said as where it went from and to, and how many changes:
/// `Cassio turned vcf1 cutoff to 720 → 1800 Hz (12 changes)`.
fn gesture(history: &History, first: usize, last: usize, count: usize) -> Gesture {
    let end = &history.moments[last].summary;
    let words = if count > 1 {
        let start = &history.moments[first].summary;
        let moved: Vec<String> = start
            .split_whitespace()
            .zip(end.split_whitespace())
            .map(|(a, b)| {
                if a == b {
                    b.to_owned()
                } else {
                    format!("{a} → {b}")
                }
            })
            .collect();
        format!("{} ({count} changes)", moved.join(" "))
    } else {
        end.clone()
    };
    Gesture { last, count, words }
}

/// A day mark: the first moment on a limb made on a new day, where the
/// day is read in `zone` (the viewer's own, in the app).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Day {
    /// The moment.
    pub moment: usize,
    /// The day, as `Wed 14 Oct`.
    pub words: String,
}

/// Every day mark in the tree, limb by limb.
///
/// A day mark is each moment whose day is not
/// the day of the moment before it on its limb (for a limb's first moment,
/// the moment it grew from). The trunk's first moment is always one.
#[must_use]
pub fn days<Z: chrono::TimeZone>(history: &History, zone: &Z) -> Vec<Day>
where
    Z::Offset: std::fmt::Display,
{
    let day_of = |m: usize| {
        chrono::DateTime::from_timestamp(history.moments[m].time, 0)
            .map(|utc| utc.with_timezone(zone).date_naive())
    };
    let mut out = Vec::new();
    for limb in &history.limbs {
        let mut before = limb.from.and_then(day_of);
        for &m in &limb.moments {
            let day = day_of(m);
            if day.is_some() && day != before {
                out.push(Day {
                    moment: m,
                    words: day.map_or_else(String::new, |d| d.format("%a %-d %b").to_string()),
                });
            }
            before = day;
        }
    }
    out
}

/// Everything a picture is drawn from.
#[derive(Debug, Clone, Copy)]
pub struct Scene<'a> {
    /// The tree read.
    pub history: &'a History,
    /// Where it grew.
    pub growth: &'a Growth,
    /// Its gestures.
    pub gestures: &'a [Gesture],
    /// Its day marks.
    pub days: &'a [Day],
    /// The moments on the wall's own line, root to where it is.
    pub line: &'a [bool],
    /// The selected moment.
    pub selected: Option<usize>,
    /// The newest moment's time: growth within an hour of it is lit.
    pub newest: i64,
}

/// The newest moment's time, or 0 for an empty tree.
#[must_use]
pub fn newest(history: &History) -> i64 {
    history.moments.iter().map(|m| m.time).max().unwrap_or(0)
}

/// The moments from the root to `here`, as a mask over every moment.
#[must_use]
pub fn line_to(history: &History, here: Option<usize>) -> Vec<bool> {
    let mut mask = vec![false; history.moments.len()];
    let mut at = here;
    while let Some(m) = at {
        if mask[m] {
            break;
        }
        mask[m] = true;
        at = history.moments[m].parent;
    }
    mask
}

/// What a glyph hangs with: found only for glyphs on screen, so a tree of
/// a million moments makes words for the few dozen in view.
#[derive(Debug, Clone, Copy)]
enum Words {
    /// The moment's own summary, cut short.
    Summary,
    /// A gesture's words.
    Gesture(usize),
    /// A mark's name.
    Mark(usize),
    /// A day mark's day.
    Day(usize),
    /// The wall's place.
    Here,
}

/// Draw `scene` as seen through `view` in `cols` × `rows`. Only what is in
/// view is looked at: steps by their extent, glyphs by their place.
#[must_use]
pub fn draw(scene: &Scene<'_>, view: &View, cols: u16, rows: u16) -> Picture {
    let mut picture = Picture::new(cols, rows);
    let seen = view.world(cols, rows, 2.0 * MAX_DOTS_WIDE);
    let to = |p| view.to_dots(p, cols, rows);
    let root_width = (ROOT_REACH * 0.04 * view.scale).clamp(1.0, 3.0);
    for &(x, y) in &scene.growth.roots {
        if x + ROOT_REACH < seen.0
            || x - ROOT_REACH > seen.2
            || y < seen.1
            || y - ROOT_DEPTH > seen.3
        {
            continue;
        }
        for (a, b) in root_lines((x, y)) {
            picture.line(to(a), to(b), root_width, ROOT, false);
        }
    }
    for step in &scene.growth.steps {
        let pad = step.widest;
        let (lo_x, hi_x) = (
            step.start.0.min(step.end.0) - pad,
            step.start.0.max(step.end.0) + pad,
        );
        let (lo_y, hi_y) = (
            step.start.1.min(step.end.1) - pad,
            step.start.1.max(step.end.1) + pad,
        );
        if hi_x < seen.0 || lo_x > seen.2 || hi_y < seen.1 || lo_y > seen.3 {
            continue;
        }
        draw_step(&mut picture, scene, view, step, cols, rows);
    }
    let level = Level::of(view.scale);
    let on_screen = |m: usize| {
        scene
            .growth
            .at
            .get(m)
            .copied()
            .flatten()
            .filter(|&p| inside(seen, p))
    };
    let hang = hangings(scene, level, &on_screen);
    // Glyphs first, everywhere, so labels never cover one; then labels,
    // the selected moment's first, then the wall's and marks', then the
    // rest, each only where it fits.
    let mut taken = vec![false; picture.cells.len()];
    let mut placed: Vec<(i32, i32, usize, Glyph, Words)> = Vec::new();
    for (m, glyph, words) in hang {
        let Some(p) = on_screen(m) else {
            continue;
        };
        let colour = match glyph {
            Glyph::Here => HERE,
            Glyph::Mark | Glyph::Save => MARK,
            Glyph::Moment => LABEL,
            Glyph::Day => DAY,
        };
        if let Some((col, row)) = picture.glyph(to(p), glyph, colour, scene.selected == Some(m)) {
            if let Some(i) = picture.index(col, row) {
                taken[i] = true;
            }
            placed.push((col, row, m, glyph, words));
        }
    }
    if let Some(sel) = scene.selected
        && let Some(p) = on_screen(sel)
    {
        hang_selected(&mut picture, scene, sel, to(p), &mut taken);
    }
    placed.sort_by_key(|&(_, _, m, glyph, _)| {
        let rank = match glyph {
            Glyph::Here => 0,
            Glyph::Mark | Glyph::Save => 1,
            Glyph::Moment => 2,
            Glyph::Day => 3,
        };
        (rank, std::cmp::Reverse(m))
    });
    for (col, row, m, glyph, words) in placed {
        if Some(m) == scene.selected {
            continue;
        }
        let show = match glyph {
            Glyph::Here => true,
            Glyph::Mark | Glyph::Save | Glyph::Day => level >= Level::Near,
            Glyph::Moment => level >= Level::Close,
        };
        if !show {
            continue;
        }
        let text = match words {
            Words::Summary => short(&scene.history.moments[m].summary),
            Words::Gesture(i) => short(&scene.gestures[i].words),
            Words::Mark(i) => short(&scene.history.marks[i].name),
            Words::Day(i) => scene.days[i].words.clone(),
            Words::Here => "the wall is here".to_owned(),
        };
        picture.label(col + 2, row, &text, &mut taken);
    }
    picture
}

/// Mark the selected moment's cell (at dot point `(x, y)`) and hang its
/// words, before anything else's.
fn hang_selected(
    picture: &mut Picture,
    scene: &Scene<'_>,
    sel: usize,
    (x, y): (f64, f64),
    taken: &mut [bool],
) {
    if x >= 0.0 && y >= 0.0 {
        let (col, row) = ((x / 2.0).floor() as i32, (y / 4.0).floor() as i32);
        if let Some(i) = picture.index(col, row) {
            let cell = &mut picture.cells[i];
            if cell.text.is_none() || cell.tail {
                cell.text = Some(Glyph::Moment.char());
                cell.tail = false;
            }
            cell.selected = true;
            taken[i] = true;
            let words = short(&scene.history.moments[sel].summary);
            picture.label(col + 2, row, &words, taken);
        }
    }
}

/// Everything that hangs a glyph at this level and is on screen: days
/// first, so any other glyph on the same cell is drawn over one.
fn hangings(
    scene: &Scene<'_>,
    level: Level,
    on_screen: &impl Fn(usize) -> Option<(f64, f64)>,
) -> Vec<(usize, Glyph, Words)> {
    let mut hang: Vec<(usize, Glyph, Words)> = Vec::new();
    for (i, day) in scene.days.iter().enumerate() {
        if on_screen(day.moment).is_some() {
            hang.push((day.moment, Glyph::Day, Words::Day(i)));
        }
    }
    match level {
        Level::Every => {
            for step in &scene.growth.steps {
                let limb = &scene.history.limbs[step.limb];
                for &m in &limb.moments[step.first..=step.last] {
                    if on_screen(m).is_some() {
                        hang.push((m, Glyph::Moment, Words::Summary));
                    }
                }
            }
        }
        Level::Close => {
            for (i, g) in scene.gestures.iter().enumerate() {
                if on_screen(g.last).is_some() {
                    hang.push((g.last, Glyph::Moment, Words::Gesture(i)));
                }
            }
        }
        Level::Far | Level::Near => {}
    }
    for (i, mark) in scene.history.marks.iter().enumerate() {
        if on_screen(mark.moment).is_some() {
            let glyph = if mark.save { Glyph::Save } else { Glyph::Mark };
            hang.push((mark.moment, glyph, Words::Mark(i)));
        }
    }
    if let Some(here) = scene.history.here
        && on_screen(here).is_some()
    {
        hang.push((here, Glyph::Here, Words::Here));
    }
    hang
}

/// Draw one step: each moment's stretch, from the moment before it (or
/// the step's start) to it, in that moment's own width and colour, bright
/// where it is on the wall's own line. Stretches drawn alike in a row are
/// one line: a step is straight.
fn draw_step(
    picture: &mut Picture,
    scene: &Scene<'_>,
    view: &View,
    step: &Step,
    cols: u16,
    rows: u16,
) {
    let limb = &scene.history.limbs[step.limb];
    let mut from = step.start;
    let mut run: Option<Run> = None;
    for &m in &limb.moments[step.first..=step.last] {
        let (Some(at), Some(look)) = (scene.growth.at[m], scene.growth.look[m]) else {
            continue;
        };
        // Far out a limb keeps some of its thickness, so the trunk still
        // reads as a trunk; close in it is drawn true to scale, up to a
        // limit.
        let wide = (look.width * view.scale)
            .max(look.width * FAR_THICKNESS)
            .clamp(1.0, MAX_DOTS_WIDE);
        let mut colour = colour_of(look.sound);
        if scene.newest - scene.history.moments[m].time < NEWEST_SECONDS {
            colour = lit(colour);
        }
        let bright = scene.line.get(m).copied().unwrap_or(false);
        let style = Style {
            wide,
            colour,
            bright,
        };
        match &mut run {
            Some((_, end, same)) if *same == style => *end = at,
            _ => {
                if let Some((a, b, done)) = run.take() {
                    picture.line(
                        view.to_dots(a, cols, rows),
                        view.to_dots(b, cols, rows),
                        done.wide,
                        done.colour,
                        done.bright,
                    );
                }
                run = Some((from, at, style));
            }
        }
        from = at;
    }
    if let Some((a, b, done)) = run {
        picture.line(
            view.to_dots(a, cols, rows),
            view.to_dots(b, cols, rows),
            done.wide,
            done.colour,
            done.bright,
        );
    }
}

/// A run of stretches drawn alike: from, to, and how.
type Run = ((f64, f64), (f64, f64), Style);

/// How a stretch of limb is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Style {
    wide: f64,
    colour: (u8, u8, u8),
    bright: bool,
}

/// A colour a quarter of the way to white: new growth, a little lit.
fn lit((r, g, b): (u8, u8, u8)) -> (u8, u8, u8) {
    let up = |c: u8| c.saturating_add((255 - c) / 4);
    (up(r), up(g), up(b))
}

/// The words a moment hangs with: its summary, at most 48 characters.
#[must_use]
pub fn short(summary: &str) -> String {
    let mut out: String = summary.chars().take(48).collect();
    if summary.chars().count() > 48 {
        out.pop();
        out.push('…');
    }
    out
}

/// A tree's roots: five spreading down and out from its foot, always the
/// same size (they stand for where a tree began, and never grow).
#[must_use]
pub fn root_lines(foot: (f64, f64)) -> Vec<((f64, f64), (f64, f64))> {
    let mut lines = Vec::new();
    for (k, reach) in [
        (-1.0_f64, 1.0_f64),
        (-0.5, 0.7),
        (0.0, 0.4),
        (0.5, 0.75),
        (1.0, 0.95),
    ] {
        let mut at = foot;
        let segments = 5;
        for s in 1..=segments {
            let t = f64::from(s) / f64::from(segments);
            let next = (
                (k * ROOT_REACH * reach).mul_add(t, foot.0),
                (ROOT_DEPTH * reach * t.sqrt()).mul_add(-0.4_f64.mul_add(-k.abs(), 1.0), foot.1),
            );
            lines.push((at, next));
            at = next;
        }
    }
    lines
}

#[cfg(test)]
mod tests;
