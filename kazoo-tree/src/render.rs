//! The tree drawn in braille.
//!
//! Each terminal cell is 2 dots wide and 4 tall; limbs are thick lines of
//! dots coloured by how they sounded, and the marks hang off them as glyphs
//! with labels. What is drawn depends on how far in you are ([`Level`]).

use crate::growth::{Growth, Step};
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
    pub fn of(scale: f32) -> Self {
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
    pub centre: (f32, f32),
    /// Braille dots per world unit.
    pub scale: f32,
}

impl View {
    /// Dots from the picture's top left for a world point.
    #[must_use]
    pub fn to_dots(&self, (x, y): (f32, f32), cols: u16, rows: u16) -> (f32, f32) {
        let w = f32::from(cols) * 2.0;
        let h = f32::from(rows) * 4.0;
        (
            (x - self.centre.0).mul_add(self.scale, w / 2.0),
            (y - self.centre.1).mul_add(-self.scale, h / 2.0),
        )
    }

    /// The world point under a cell's middle.
    #[must_use]
    pub fn to_world(&self, col: u16, row: u16, cols: u16, rows: u16) -> (f32, f32) {
        let w = f32::from(cols) * 2.0;
        let h = f32::from(rows) * 4.0;
        let dx = f32::from(col).mul_add(2.0, 1.0);
        let dy = f32::from(row).mul_add(4.0, 2.0);
        (
            (dx - w / 2.0) / self.scale + self.centre.0,
            (h / 2.0 - dy) / self.scale + self.centre.1,
        )
    }

    /// The view that shows all of `bounds` in `cols` × `rows`.
    #[must_use]
    pub fn fit(bounds: (f32, f32, f32, f32), cols: u16, rows: u16) -> Self {
        let width = (bounds.2 - bounds.0).max(1.0);
        let height = (bounds.3 - bounds.1).max(1.0);
        let scale = (f32::from(cols.max(1)) * 2.0 / width)
            .min(f32::from(rows.max(1)) * 4.0 / height)
            * 0.95;
        Self {
            centre: (
                f32::midpoint(bounds.0, bounds.2),
                f32::midpoint(bounds.1, bounds.3),
            ),
            scale: scale.max(MIN_SCALE),
        }
    }
}

/// The furthest out the view goes.
pub const MIN_SCALE: f32 = 0.005;
/// The furthest in.
pub const MAX_SCALE: f32 = 40.0;
/// The share of its width, in dots, a limb keeps however far out.
const FAR_THICKNESS: f32 = 0.35;
/// The widest a limb is ever drawn, in dots, so close in a thick limb does
/// not bury its own marks.
const MAX_DOTS_WIDE: f32 = 8.0;

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
}

impl Cell {
    /// The character this cell shows.
    #[must_use]
    pub fn char(&self) -> char {
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
                .filter_map(|col| self.at(col, row).map(Cell::char))
                .collect();
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    fn dot(&mut self, x: f32, y: f32, colour: (u8, u8, u8), bright: bool) {
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
        from: (f32, f32),
        to: (f32, f32),
        wide: f32,
        colour: (u8, u8, u8),
        bright: bool,
    ) {
        let width = f32::from(self.cols) * 2.0;
        let height = f32::from(self.rows) * 4.0;
        let pad = wide + 2.0;
        if from.0.max(to.0) < -pad
            || from.0.min(to.0) > width + pad
            || from.1.max(to.1) < -pad
            || from.1.min(to.1) > height + pad
        {
            return;
        }
        let length = (to.0 - from.0).hypot(to.1 - from.1);
        let samples = (length * 2.0).ceil().clamp(1.0, 100_000.0) as u32;
        let radius = (wide / 2.0).max(0.0);
        let reach = radius.ceil() as i32;
        for i in 0..=samples {
            let along = i as f32 / samples as f32;
            let x = (to.0 - from.0).mul_add(along, from.0);
            let y = (to.1 - from.1).mul_add(along, from.1);
            if reach == 0 {
                self.dot(x, y, colour, bright);
                continue;
            }
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    let (off_x, off_y) = (dx as f32, dy as f32);
                    if off_x.hypot(off_y) <= radius {
                        self.dot(x + off_x, y + off_y, colour, bright);
                    }
                }
            }
        }
    }

    /// Put a glyph at the cell holding dot point `p`. Answers its cell.
    fn glyph(
        &mut self,
        p: (f32, f32),
        glyph: Glyph,
        colour: (u8, u8, u8),
        selected: bool,
    ) -> Option<(i32, i32)> {
        if !(p.0.is_finite() && p.1.is_finite()) || p.0 < 0.0 || p.1 < 0.0 {
            return None;
        }
        let (col, row) = ((p.0 / 2.0).floor() as i32, (p.1 / 4.0).floor() as i32);
        let i = self.index(col, row)?;
        let cell = &mut self.cells[i];
        cell.text = Some(glyph.char());
        cell.colour = Some(colour);
        cell.bright = true;
        cell.selected |= selected;
        Some((col, row))
    }

    /// Write `text` from `col`, `row` if every cell it needs is free of
    /// glyphs and other labels. Answers whether it was written.
    fn label(&mut self, col: i32, row: i32, text: &str, taken: &mut [bool]) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let width = i32::try_from(chars.len()).unwrap_or(i32::MAX);
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
        for (k, ch) in chars
            .iter()
            .take(usize::try_from(shown).unwrap_or(0))
            .enumerate()
        {
            let c = col + i32::try_from(k).unwrap_or(i32::MAX);
            if let Some(i) = self.index(c, row) {
                let ch = if k + 1 == usize::try_from(shown).unwrap_or(0) && shown < width {
                    '…'
                } else {
                    *ch
                };
                self.cells[i].text = Some(ch);
                self.cells[i].colour = Some(LABEL);
                self.cells[i].bright = false;
                taken[i] = true;
            }
        }
        true
    }
}

/// The colour labels are drawn in.
pub const LABEL: (u8, u8, u8) = (200, 200, 190);
/// Bark: a limb whose sound was not kept.
pub const BARK: (u8, u8, u8) = (150, 120, 90);
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
        return BARK;
    };
    let centroid = if sound.centroid_hz.is_finite() {
        sound.centroid_hz
    } else {
        500.0
    };
    let bright = ((centroid.max(1.0).log2() - 100_f32.log2()) / (4000_f32.log2() - 100_f32.log2()))
        .clamp(0.0, 1.0);
    let hue = bright.mul_add(190.0, 30.0);
    let rms = if sound.rms_db.is_finite() {
        sound.rms_db
    } else {
        -60.0
    };
    let light = ((rms + 50.0) / 40.0).clamp(0.0, 1.0).mul_add(0.35, 0.35);
    let onsets = if sound.onsets.is_finite() {
        sound.onsets
    } else {
        0.0
    };
    let saturation = (onsets / 6.0).clamp(0.0, 1.0).mul_add(0.55, 0.35);
    hsl(hue, saturation, light)
}

fn hsl(hue: f32, saturation: f32, light: f32) -> (u8, u8, u8) {
    let chroma = (1.0 - 2.0_f32.mul_add(light, -1.0).abs()) * saturation;
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
    let to = |v: f32| ((v + base).clamp(0.0, 1.0) * 255.0).round() as u8;
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

/// What a moment does, for grouping: the summary's first four words
/// (`Cassio turned vcf6 cutoff`), which name the seat, the act and what it
/// acted on.
fn subject(summary: &str) -> String {
    summary
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every gesture in the tree, limb by limb.
#[must_use]
pub fn gestures(history: &History) -> Vec<Gesture> {
    let mut out = Vec::new();
    for limb in &history.limbs {
        let mut run: Option<(String, usize, usize)> = None;
        for &m in &limb.moments {
            let what = subject(&history.moments[m].summary);
            match &mut run {
                Some((words, last, count)) if *words == what => {
                    *last = m;
                    *count += 1;
                }
                _ => {
                    if let Some((words, last, count)) = run.take() {
                        out.push(gesture(words, last, count));
                    }
                    run = Some((what, m, 1));
                }
            }
        }
        if let Some((words, last, count)) = run {
            out.push(gesture(words, last, count));
        }
    }
    out
}

fn gesture(words: String, last: usize, count: usize) -> Gesture {
    let words = if count > 1 {
        format!("{words} ×{count}")
    } else {
        words
    };
    Gesture { last, count, words }
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
    /// The moments on the wall's own line, root to where it is.
    pub line: &'a [bool],
    /// The selected moment.
    pub selected: Option<usize>,
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

/// Draw `scene` as seen through `view` in `cols` × `rows`.
#[must_use]
pub fn draw(scene: &Scene<'_>, view: &View, cols: u16, rows: u16) -> Picture {
    let mut picture = Picture::new(cols, rows);
    let to = |p| view.to_dots(p, cols, rows);
    let (reach, depth) = (scene.growth.root_reach, scene.growth.root_depth());
    for &(x, y) in &scene.growth.roots {
        for (a, b) in root_lines((x, y), reach, depth) {
            picture.line(
                to(a),
                to(b),
                (reach * 0.04 * view.scale).clamp(1.0, 3.0),
                ROOT,
                false,
            );
        }
    }
    for step in &scene.growth.steps {
        draw_step(&mut picture, scene, view, step, cols, rows);
    }
    let level = Level::of(view.scale);
    let mut taken = vec![false; picture.cells.len()];
    let mut hang: Vec<(usize, Glyph, String)> = Vec::new();
    match level {
        Level::Every => {
            for m in 0..scene.history.moments.len() {
                hang.push((m, Glyph::Moment, short(&scene.history.moments[m].summary)));
            }
        }
        Level::Close => {
            for g in scene.gestures {
                hang.push((g.last, Glyph::Moment, g.words.clone()));
            }
        }
        Level::Far | Level::Near => {}
    }
    for mark in &scene.history.marks {
        let glyph = if mark.save { Glyph::Save } else { Glyph::Mark };
        hang.push((mark.moment, glyph, mark.name.clone()));
    }
    if let Some(here) = scene.history.here {
        hang.push((here, Glyph::Here, "the wall is here".to_owned()));
    }
    // Glyphs first, everywhere, so labels never cover one; then labels,
    // the selected moment's first, then the wall's and marks', then the
    // rest, each only where it fits.
    let mut placed: Vec<(i32, i32, usize, Glyph, String)> = Vec::new();
    for (m, glyph, words) in hang {
        let Some(p) = scene.growth.at.get(m).copied().flatten() else {
            continue;
        };
        let colour = match glyph {
            Glyph::Here => HERE,
            Glyph::Mark | Glyph::Save => MARK,
            Glyph::Moment => LABEL,
        };
        if let Some((col, row)) = picture.glyph(to(p), glyph, colour, scene.selected == Some(m)) {
            if let Some(i) = picture.index(col, row) {
                taken[i] = true;
            }
            placed.push((col, row, m, glyph, words));
        }
    }
    if let Some(sel) = scene.selected
        && let Some(p) = scene.growth.at.get(sel).copied().flatten()
    {
        let (x, y) = to(p);
        if x >= 0.0 && y >= 0.0 {
            let (col, row) = ((x / 2.0).floor() as i32, (y / 4.0).floor() as i32);
            if let Some(i) = picture.index(col, row) {
                let cell = &mut picture.cells[i];
                if cell.text.is_none() {
                    cell.text = Some(Glyph::Moment.char());
                }
                cell.selected = true;
                taken[i] = true;
                let words = short(&scene.history.moments[sel].summary);
                picture.label(col + 2, row, &words, &mut taken);
            }
        }
    }
    placed.sort_by_key(|&(_, _, m, glyph, _)| {
        let rank = match glyph {
            Glyph::Here => 0,
            Glyph::Mark | Glyph::Save => 1,
            Glyph::Moment => 2,
        };
        (rank, std::cmp::Reverse(m))
    });
    for (col, row, m, glyph, words) in placed {
        if Some(m) == scene.selected {
            continue;
        }
        let show = match glyph {
            Glyph::Here => true,
            Glyph::Mark | Glyph::Save => level >= Level::Near,
            Glyph::Moment => level >= Level::Close,
        };
        if show {
            picture.label(col + 2, row, &words, &mut taken);
        }
    }
    picture
}

fn draw_step(
    picture: &mut Picture,
    scene: &Scene<'_>,
    view: &View,
    step: &Step,
    cols: u16,
    rows: u16,
) {
    let limb = &scene.history.limbs[step.limb];
    let last = limb.moments[step.last];
    let tip = scene.growth.at[last].unwrap_or(step.end);
    let bright = scene.line.get(last).copied().unwrap_or(false);
    // Far out a limb keeps some of its thickness, so the trunk still reads
    // as a trunk; close in it is drawn true to scale, up to a limit.
    let wide = (step.width * view.scale)
        .max(step.width * FAR_THICKNESS)
        .clamp(1.0, MAX_DOTS_WIDE);
    let from = view.to_dots(step.start, cols, rows);
    let to = view.to_dots(tip, cols, rows);
    picture.line(from, to, wide, colour_of(step.sound), bright);
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

/// A root's lines: five roots spreading down and out from its foot.
#[must_use]
pub fn root_lines(foot: (f32, f32), reach_x: f32, depth: f32) -> Vec<((f32, f32), (f32, f32))> {
    let mut lines = Vec::new();
    for (k, reach) in [
        (-1.0_f32, 1.0_f32),
        (-0.5, 0.7),
        (0.0, 0.4),
        (0.5, 0.75),
        (1.0, 0.95),
    ] {
        let mut at = foot;
        let segments = 5;
        for s in 1..=segments {
            let t = s as f32 / segments as f32;
            let next = (
                (k * reach_x * reach).mul_add(t, foot.0),
                (depth * reach * t.sqrt()).mul_add(-0.4_f32.mul_add(-k.abs(), 1.0), foot.1),
            );
            lines.push((at, next));
            at = next;
        }
    }
    lines
}

#[cfg(test)]
mod tests;
