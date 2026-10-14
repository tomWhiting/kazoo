//! `kazoo tree`'s state and keys: a view panned and zoomed over the grown
//! tree, a selected moment and what it says, and the help card. Reading the
//! tree never changes the music.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::growth::Growth;
use crate::history::{History, HistoryError};
use crate::render::{self, Gesture, Level, MAX_SCALE, MIN_SCALE, View};

/// Zoom per key press or wheel notch.
const ZOOM: f32 = 1.5;
/// The details panel's width, when the terminal is wide enough for it.
pub const PANEL: u16 = 44;
/// Moments per `{` / `}` jump.
const JUMP: isize = 16;

/// The whole app.
#[derive(Debug)]
pub struct App {
    /// Where the tree is read from.
    pub dir: PathBuf,
    /// The tree.
    pub history: History,
    /// Where it grew.
    pub growth: Growth,
    /// Its gestures.
    pub gestures: Vec<Gesture>,
    /// The wall's own line, root to where it is.
    pub line: Vec<bool>,
    /// What is in view; `None` until the first frame fits the tree.
    pub view: Option<View>,
    /// The selected moment.
    pub selected: Option<usize>,
    /// The help card is open.
    pub help: bool,
    /// The last word to show in the footer: what just happened, or why not.
    pub said: String,
    /// The picture's size at the last frame.
    pub area: (u16, u16),
    /// Where a mouse drag started: the cell and the view's centre then.
    drag: Option<((u16, u16), (f32, f32))>,
    /// Quit at the next turn of the loop.
    pub quit: bool,
}

impl App {
    /// Read the tree at `dir` and grow it.
    ///
    /// # Errors
    ///
    /// When the tree cannot be read.
    pub fn open(dir: PathBuf) -> Result<Self, HistoryError> {
        let history = History::read(&dir)?;
        let mut app = Self {
            dir,
            history: History::default(),
            growth: Growth::default(),
            gestures: Vec::new(),
            line: Vec::new(),
            view: None,
            selected: None,
            help: false,
            said: String::new(),
            area: (0, 0),
            drag: None,
            quit: false,
        };
        app.take(history);
        app.selected = app
            .history
            .here
            .or_else(|| app.history.moments.len().checked_sub(1));
        Ok(app)
    }

    /// Take a freshly read tree, keeping the view and the selection (by
    /// commit, which never moves: the past is never redrawn).
    pub fn take(&mut self, history: History) {
        let selected_id = self.selected.map(|m| self.history.moments[m].id.clone());
        self.growth = Growth::grow(&history);
        self.gestures = render::gestures(&history);
        self.line = render::line_to(&history, history.here);
        self.selected = selected_id.and_then(|id| history.moments.iter().position(|m| m.id == id));
        self.history = history;
    }

    /// Read the tree again.
    pub fn reload(&mut self) {
        match History::read(&self.dir) {
            Ok(history) => {
                let before = self.history.moments.len();
                self.take(history);
                let grown = self.history.moments.len().saturating_sub(before);
                self.said = if grown == 0 {
                    "read again: nothing new".to_owned()
                } else {
                    format!("read again: {grown} new moments")
                };
            }
            Err(err) => self.said = format!("the tree could not be read: {err}"),
        }
    }

    /// The view, fitted to the tree the first time it is asked for.
    pub fn view(&mut self, cols: u16, rows: u16) -> View {
        *self
            .view
            .get_or_insert_with(|| View::fit(self.growth.bounds, cols, rows))
    }

    fn zoom_about(&mut self, factor: f32, cell: Option<(u16, u16)>) {
        let (cols, rows) = self.area;
        let mut view = self.view(cols, rows);
        let scale = (view.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        if let Some((col, row)) = cell {
            // Keep the point under the cursor where it is.
            let world = view.to_world(col, row, cols, rows);
            let ratio = view.scale / scale;
            view.centre = (
                (view.centre.0 - world.0).mul_add(ratio, world.0),
                (view.centre.1 - world.1).mul_add(ratio, world.1),
            );
        }
        view.scale = scale;
        self.view = Some(view);
    }

    fn pan(&mut self, cols_share: f32, rows_share: f32) {
        let (cols, rows) = self.area;
        let mut view = self.view(cols, rows);
        view.centre.0 += cols_share * f32::from(cols) * 2.0 / view.scale;
        view.centre.1 += rows_share * f32::from(rows) * 4.0 / view.scale;
        self.view = Some(view);
    }

    /// Put the selected moment in the middle of the view.
    fn centre_on_selected(&mut self) {
        let (cols, rows) = self.area;
        if let Some(p) = self
            .selected
            .and_then(|m| self.growth.at.get(m).copied().flatten())
        {
            let mut view = self.view(cols, rows);
            view.centre = p;
            self.view = Some(view);
        }
    }

    /// Select `moment` and bring it into view.
    fn select(&mut self, moment: usize) {
        self.selected = Some(moment);
        self.centre_on_selected();
    }

    /// The marks, the wall's place, in moment order: what Tab walks.
    fn stops(&self) -> Vec<usize> {
        let mut stops: Vec<usize> = self.history.marks.iter().map(|m| m.moment).collect();
        stops.extend(self.history.here);
        stops.sort_unstable();
        stops.dedup();
        stops
    }

    fn next_stop(&mut self, forward: bool) {
        let stops = self.stops();
        if stops.is_empty() {
            "no marks yet: nothing to jump to".clone_into(&mut self.said);
            return;
        }
        let at = self.selected.unwrap_or(0);
        let next = if forward {
            stops.iter().copied().find(|&m| m > at).unwrap_or(stops[0])
        } else {
            stops
                .iter()
                .rev()
                .copied()
                .find(|&m| m < at)
                .unwrap_or(stops[stops.len() - 1])
        };
        self.select(next);
    }

    /// Move `by` moments along the selected moment's limb: back through
    /// the moments it grew from, forward along its own limb.
    fn walk(&mut self, by: isize) {
        let Some(at) = self.selected else {
            return;
        };
        let mut moment = at;
        if by < 0 {
            for _ in 0..by.unsigned_abs() {
                match self.history.moments[moment].parent {
                    Some(parent) => moment = parent,
                    None => break,
                }
            }
        } else if let Some((limb, place)) = self.history.place_of(moment) {
            let moments = &self.history.limbs[limb].moments;
            let to = (place + by.unsigned_abs()).min(moments.len() - 1);
            moment = moments[to];
        }
        self.select(moment);
    }

    /// The moment nearest a cell, within three cells, among those drawn
    /// at this level (every moment when close; marks and the wall's place
    /// otherwise, and step ends so a limb can be picked).
    fn nearest(&self, col: u16, row: u16) -> Option<usize> {
        let (cols, rows) = self.area;
        let view = self.view?;
        let level = Level::of(view.scale);
        let target = (
            f32::from(col).mul_add(2.0, 1.0),
            f32::from(row).mul_add(4.0, 2.0),
        );
        let mut best: Option<(f32, usize)> = None;
        let mut consider = |m: usize| {
            if let Some(p) = self.growth.at.get(m).copied().flatten() {
                let (x, y) = view.to_dots(p, cols, rows);
                let d = (x - target.0).hypot((y - target.1) * 0.5);
                if d <= 6.0 && best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, m));
                }
            }
        };
        if level >= Level::Every {
            (0..self.history.moments.len()).for_each(&mut consider);
        } else {
            self.history
                .marks
                .iter()
                .for_each(|mark| consider(mark.moment));
            self.history.here.into_iter().for_each(&mut consider);
            for step in &self.growth.steps {
                consider(self.history.limbs[step.limb].moments[step.last]);
            }
            if level >= Level::Close {
                self.gestures.iter().for_each(|g| consider(g.last));
            }
        }
        best.map(|(_, m)| m)
    }

    /// A key.
    pub fn key(&mut self, key: KeyEvent) {
        if self.help {
            if matches!(key.code, KeyCode::Char('?' | 'q') | KeyCode::Esc) {
                self.help = false;
            }
            return;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let step = if shift { 0.5 } else { 0.125 };
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Left | KeyCode::Char('h') => self.pan(-step, 0.0),
            KeyCode::Right | KeyCode::Char('l') => self.pan(step, 0.0),
            KeyCode::Up | KeyCode::Char('k') => self.pan(0.0, step),
            KeyCode::Down | KeyCode::Char('j') => self.pan(0.0, -step),
            KeyCode::Char('+' | '=') => self.zoom_about(ZOOM, None),
            KeyCode::Char('-' | '_') => self.zoom_about(1.0 / ZOOM, None),
            KeyCode::Char('0') => {
                let (cols, rows) = self.area;
                self.view = Some(View::fit(self.growth.bounds, cols, rows));
            }
            KeyCode::Tab => self.next_stop(true),
            KeyCode::BackTab => self.next_stop(false),
            KeyCode::Char('[') => self.walk(-1),
            KeyCode::Char(']') => self.walk(1),
            KeyCode::Char('{') => self.walk(-JUMP),
            KeyCode::Char('}') => self.walk(JUMP),
            KeyCode::Char('.') => match self.history.here {
                Some(here) => self.select(here),
                None => "the tree has no moment for the wall yet".clone_into(&mut self.said),
            },
            KeyCode::Enter | KeyCode::Char('c') => self.centre_on_selected(),
            KeyCode::Char('r') => self.reload(),
            _ => {}
        }
    }

    /// A mouse event over the picture (`col`, `row` inside it).
    pub fn mouse(&mut self, event: MouseEvent) {
        let (cols, rows) = self.area;
        let (col, row) = (event.column, event.row);
        let inside = col < cols && row < rows;
        match event.kind {
            MouseEventKind::ScrollUp if inside => self.zoom_about(ZOOM, Some((col, row))),
            MouseEventKind::ScrollDown if inside => self.zoom_about(1.0 / ZOOM, Some((col, row))),
            MouseEventKind::Down(MouseButton::Left) if inside => {
                if let Some(m) = self.nearest(col, row) {
                    self.selected = Some(m);
                }
                let centre = self.view(cols, rows).centre;
                self.drag = Some(((col, row), centre));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(((c0, r0), centre)) = self.drag {
                    let mut view = self.view(cols, rows);
                    let dx = (f32::from(col) - f32::from(c0)) * 2.0 / view.scale;
                    let dy = (f32::from(row) - f32::from(r0)) * 4.0 / view.scale;
                    view.centre = (centre.0 - dx, centre.1 + dy);
                    self.view = Some(view);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            _ => {}
        }
    }

    /// What the details panel says about the selected moment, line by line.
    #[must_use]
    pub fn details(&self) -> Vec<(String, String)> {
        let Some(m) = self.selected else {
            return vec![(String::new(), "nothing selected".to_owned())];
        };
        let moment = &self.history.moments[m];
        let mut lines = vec![(String::new(), moment.summary.clone())];
        lines.push(("who".to_owned(), moment.seat.clone()));
        lines.push(("when".to_owned(), local_time(moment.time)));
        if let Some(beat) = moment.beat {
            let bar = (beat / 4.0).floor() + 1.0;
            let in_bar = beat.rem_euclid(4.0) + 1.0;
            lines.push((
                "bar".to_owned(),
                format!("{bar:.0}, beat {in_bar:.2} (in 4)"),
            ));
        }
        if let Some(tempo) = moment.tempo {
            lines.push(("tempo".to_owned(), format!("{tempo:.1} BPM")));
        }
        lines.push(("alive".to_owned(), format!("{} modules", moment.modules)));
        let what = match moment.seq {
            Some(seq) if moment.kind.is_empty() => format!("change {seq}"),
            Some(seq) => format!("{} {seq}", moment.kind),
            None if moment.kind.is_empty() => "a moment".to_owned(),
            None => moment.kind.clone(),
        };
        lines.push(("kind".to_owned(), what));
        if let Some((limb, place)) = self.history.place_of(m) {
            let limb = &self.history.limbs[limb];
            lines.push((
                "limb".to_owned(),
                format!("{} ({} of {})", limb.name, place + 1, limb.moments.len()),
            ));
        }
        let sound = moment.sound.map_or_else(
            || "not kept (moments keep their sound from KZ-T08c on)".to_owned(),
            |s| {
                format!(
                    "{:.1} dB, {:.0} Hz centre, low {:.2} mid {:.2} high {:.2}, {:.1} onsets/s",
                    s.rms_db, s.centroid_hz, s.low, s.mid, s.high, s.onsets
                )
            },
        );
        lines.push(("sound".to_owned(), sound));
        for mark in self.history.marks.iter().filter(|mark| mark.moment == m) {
            let kind = if mark.save { "save" } else { "mark" };
            lines.push((kind.to_owned(), mark.name.clone()));
        }
        if self.history.here == Some(m) {
            lines.push((String::new(), "◉ the wall is here".to_owned()));
        }
        lines.push(("commit".to_owned(), moment.id.chars().take(12).collect()));
        lines
    }
}

fn local_time(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0).map_or_else(
        || format!("{unix} (not a time)"),
        |utc| {
            utc.with_timezone(&chrono::Local)
                .format("%a %-d %b %Y, %H:%M:%S")
                .to_string()
        },
    )
}

/// The help card, grouped, each line `key: what it does`.
pub const HELP: &[(&str, &[(&str, &str)])] = &[
    (
        "look around",
        &[
            ("arrows, h j k l", "pan (Shift: further)"),
            ("drag", "pan with the mouse"),
            ("+ - or wheel", "zoom in and out"),
            ("0", "the whole tree"),
        ],
    ),
    (
        "find a moment",
        &[
            ("click", "select the nearest moment"),
            ("Tab, Shift-Tab", "next or previous mark"),
            ("[ ]", "one moment back or on"),
            ("{ }", "sixteen moments back or on"),
            (".", "where the wall is"),
            ("Enter, c", "centre on the selected moment"),
        ],
    ),
    (
        "the tree",
        &[
            ("r", "read the tree again"),
            ("?", "this card"),
            ("q, Esc", "quit"),
        ],
    ),
    (
        "what you see",
        &[
            ("◉", "where the wall is"),
            ("◆ ▪", "a mark, a save's mark"),
            ("•", "a gesture or a moment (zoomed in)"),
            ("bright limb", "the wall's own line"),
            ("thickness", "modules alive"),
            ("colour", "sound: amber low, blue bright"),
            ("bark colour", "sound not kept for it"),
        ],
    ),
];

#[cfg(test)]
mod tests;
