//! `kazoo tree`'s state and keys: a view panned and zoomed over the grown
//! tree, a selected moment and what it says, and the help card.
//!
//! Reading the tree never changes the music unless asked: the arrows,
//! clicks and Tab only look, saying how far the selected moment is from
//! the wall; Enter (or `g`) puts the wall there, and only while following
//! (`f`, off from the start) does the wall go wherever the selection comes
//! to rest. Every go is undoable: the wall keeps itself as it was first.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::growth::Growth;
use crate::history::{History, HistoryError, Snapshot};
use crate::lanes::{self, Lanes};
use crate::ledger::Ledger;
use crate::render::{self, Day, Gesture, Level, MAX_SCALE, View};

/// Zoom per key press or wheel notch.
const ZOOM: f64 = 1.5;
/// The details panel's width, when the terminal is wide enough for it.
pub const PANEL: u16 = 44;
/// How far Shift pans, as a share of the view.
const FAR: f64 = 0.5;
/// Moments per `{` / `}` jump.
const JUMP: isize = 16;
/// How long the selection rests before a follow puts the wall there, so
/// running through moments sends one go, not one a key.
pub const REST: Duration = Duration::from_millis(150);

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
    /// Its day marks, in the viewer's own time zone.
    pub days: Vec<Day>,
    /// The wall's own line, root to where it is.
    pub line: Vec<bool>,
    /// The lanes view: its rows, newest first, and its lanes.
    pub lanes: Lanes,
    /// Which view is shown (`v` turns between them).
    pub shown: Shown,
    /// The lanes view's first row on screen.
    pub lane_top: usize,
    /// What is in view; `None` until the first frame fits the tree.
    pub view: Option<View>,
    /// The selected moment.
    pub selected: Option<usize>,
    /// The help card is open.
    pub help: bool,
    /// The help card's first line on screen, when it is taller than the
    /// screen.
    pub help_top: usize,
    /// Text to put on the terminal's clipboard at the next turn of the
    /// loop (`y`).
    pub yank: Option<String>,
    /// The last word to show in the footer: what just happened, or why not.
    pub said: String,
    /// The picture's size at the last frame.
    pub area: (u16, u16),
    /// Where a mouse drag started: the cell and the view's centre then.
    drag: Option<((u16, u16), (f64, f64))>,
    /// Quit at the next turn of the loop.
    pub quit: bool,
    /// The tree was asked to be read again (`r`): the reader is told.
    pub wants_reading: bool,
    /// Where a go is: asked for, or waiting for the wall.
    pub go: Go,
    /// Whether the wall follows the selection (`f`), and where to.
    pub follow: Follow,
    /// The wall's moment ablaze (`b` puts it out or lights it again).
    pub fire: crate::fire::Blaze,
}

/// Following: the wall goes wherever the selection comes to rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Follow {
    /// `g` alone puts the wall at the selected moment.
    Off,
    /// The selection the user last moved to, by its whole commit, and
    /// when; `None` once it has been sent, or before any move. Only a move
    /// by the user is followed, never a selection the reading kept.
    On(Option<(Arc<str>, Instant)>),
}

/// Where putting the wall at a moment is. Each moment is held by its whole
/// commit, so a tree read again meanwhile cannot change which moment is
/// meant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Go {
    /// Nothing asked.
    #[default]
    Idle,
    /// Asked for (`g`): the loop sends it to the wall at its next turn.
    Send(Arc<str>),
    /// Sent, and the wall has not answered yet.
    Sent,
}

/// Which view is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// The tree, grown.
    Tree,
    /// The lanes: one row a moment, newest first.
    Lanes,
}

/// A tree read and grown, ready to show: made off the screen's thread, so
/// reading and growing a large tree never stops the screen.
#[derive(Debug, Default)]
pub struct Grown {
    /// The tree.
    pub history: History,
    /// Where it grew.
    pub growth: Growth,
    /// Its gestures.
    pub gestures: Vec<Gesture>,
    /// Its day marks, in the viewer's own time zone.
    pub days: Vec<Day>,
    /// The wall's own line.
    pub line: Vec<bool>,
    /// The lanes view: its rows, newest first, and its lanes.
    pub lanes: Lanes,
}

impl Grown {
    /// Grow `history` afresh and find everything drawn from it.
    #[must_use]
    pub fn of(history: History) -> Self {
        let growth = Growth::grow(&history);
        Self::with(history, growth)
    }

    /// Everything drawn from `history`, grown as `growth`.
    #[must_use]
    pub fn with(history: History, growth: Growth) -> Self {
        Self {
            growth,
            gestures: render::gestures(&history),
            days: render::days(&history, &chrono::Local),
            line: render::line_to(&history, history.here),
            lanes: lanes::layout(&history),
            history,
        }
    }
}

/// Reads a tree and grows it on, keeping the order it grew in.
///
/// Each read takes only the commits new since the last, and grows only the
/// steps new since then, in the ledger's order, so nothing grown before
/// ever moves.
#[derive(Debug)]
pub struct Grower {
    dir: PathBuf,
    history: History,
    growth: Growth,
    ledger: Ledger,
}

impl Grower {
    /// A grower for the tree at `dir`, keeping its order in `ledger`.
    #[must_use]
    pub fn new(dir: PathBuf, ledger: Ledger) -> Self {
        Self {
            dir,
            history: History::default(),
            growth: Growth::default(),
            ledger,
        }
    }

    /// Read the tree as it is now and grow on to it, answering what to
    /// show and the snapshot it was read at. The ledger is held throughout,
    /// and read before the tree: another view's growth is followed, never
    /// raced. A read that fails partway is tried again whole (and grown
    /// again whole, in the ledger's order); when that fails too, nothing
    /// held is lost and the next read starts from the last good one.
    ///
    /// # Errors
    ///
    /// When the tree cannot be read, either way, or the ledger cannot be
    /// held.
    pub fn read(&mut self) -> Result<(Grown, Snapshot), HistoryError> {
        let held = self.ledger.hold().map_err(|err| {
            HistoryError::Git(format!(
                "the order the tree grew in could not be held for this view: {err}"
            ))
        })?;
        self.ledger.refresh();
        let snapshot = Snapshot::read(&self.dir)?;
        if let Err(grow) = self.history.grow(&self.dir, &snapshot, &self.ledger.limbs) {
            let mut whole = History::default();
            whole
                .grow(&self.dir, &snapshot, &self.ledger.limbs)
                .map_err(|read| HistoryError::Git(format!("{grow}; and read whole: {read}")))?;
            self.history = whole;
            // Its moments may be numbered differently: grow it again whole,
            // in the ledger's order, which draws it just as before.
            self.growth = Growth::default();
        }
        let grown = self.grow();
        drop(held);
        Ok((grown, snapshot))
    }

    fn grow(&mut self) -> Grown {
        let new_limbs: Vec<String> = self
            .history
            .by_hand
            .iter()
            .filter(|name| !self.ledger.knows_limb(name))
            .cloned()
            .collect();
        let fresh = self.growth.grow_on(&self.history, &self.ledger.steps);
        self.ledger.record(&new_limbs, &fresh);
        let mut history = self.history.clone();
        if let Some(problem) = self.ledger.problem() {
            history.problems.push(problem);
        }
        history.problems.extend(self.growth.refused.iter().cloned());
        if self.ledger.unreadable > 0 {
            history.problems.push(format!(
                "{} lines of the order the tree grew in did not read and were skipped",
                self.ledger.unreadable
            ));
        }
        Grown::with(history, self.growth.clone())
    }
}

impl App {
    /// Read the tree at `dir` and grow it.
    ///
    /// # Errors
    ///
    /// When the tree cannot be read.
    pub fn open(dir: PathBuf, ledger: Ledger) -> Result<Self, HistoryError> {
        let (grown, _) = Grower::new(dir.clone(), ledger).read()?;
        Ok(Self::showing(dir, grown))
    }

    /// The app showing `grown`, read from `dir`, the wall's place selected.
    #[must_use]
    pub fn showing(dir: PathBuf, grown: Grown) -> Self {
        let mut app = Self {
            dir,
            history: History::default(),
            growth: Growth::default(),
            gestures: Vec::new(),
            days: Vec::new(),
            line: Vec::new(),
            lanes: Lanes::default(),
            shown: Shown::Tree,
            lane_top: 0,
            view: None,
            selected: None,
            help: false,
            help_top: 0,
            yank: None,
            said: String::new(),
            area: (0, 0),
            drag: None,
            quit: false,
            wants_reading: false,
            go: Go::Idle,
            follow: Follow::Off,
            fire: crate::fire::Blaze::default(),
        };
        app.show(grown);
        app.selected = app.history.here.or_else(|| {
            (0..app.history.moments.len()).max_by_key(|&m| (app.history.moments[m].time, m))
        });
        app
    }

    /// Take a freshly read tree, growing it here: for tests and `--print`.
    pub fn take(&mut self, history: History) {
        self.show(Grown::of(history));
    }

    /// Show a tree read and grown, keeping the view and the selection (by
    /// commit, which never moves: the past is never redrawn). Says how
    /// many moments are new.
    pub fn show(&mut self, grown: Grown) {
        let selected_id = self.selected.map(|m| self.history.moments[m].id.clone());
        // The lanes keep the row at their top where it is when rows arrive
        // above it, so what is being read does not slide away.
        let top_id = self
            .lanes
            .rows
            .get(self.lane_top)
            .map(|row| self.history.moments[row.moment].id.clone());
        let before = self.history.moments.len();
        self.selected = selected_id.and_then(|id| grown.history.moment_of(&id));
        let new = grown.history.moments.len().saturating_sub(before);
        self.fire.forget();
        self.history = grown.history;
        self.growth = grown.growth;
        self.gestures = grown.gestures;
        self.days = grown.days;
        self.line = grown.line;
        self.lanes = grown.lanes;
        if let Some(top) = top_id.and_then(|id| self.history.moment_of(&id))
            && let Some(at) = self.lanes.rows.iter().position(|row| row.moment == top)
        {
            self.lane_top = at;
        }
        let now = self.history.moments.len();
        if before > now {
            self.said = format!(
                "read again whole: {} moments are no longer in the tree",
                before - now
            );
        } else if before > 0 {
            self.said = match new {
                0 => "read again: nothing new".to_owned(),
                1 => "read again: 1 new moment".to_owned(),
                n => format!("read again: {n} new moments"),
            };
        }
    }

    /// Ask for the tree to be read again now.
    pub fn reload(&mut self) {
        self.wants_reading = true;
        "reading the tree again…".clone_into(&mut self.said);
    }

    /// The view, fitted to the tree the first time it is asked for.
    pub fn view(&mut self, cols: u16, rows: u16) -> View {
        *self
            .view
            .get_or_insert_with(|| View::fit(self.growth.bounds, cols, rows))
    }

    fn zoom_about(&mut self, factor: f64, cell: Option<(u16, u16)>) {
        let (cols, rows) = self.area;
        let mut view = self.view(cols, rows);
        let least = render::min_scale(self.growth.bounds, cols, rows);
        let scale = (view.scale * factor).clamp(least, MAX_SCALE);
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

    fn pan(&mut self, cols_share: f64, rows_share: f64) {
        let (cols, rows) = self.area;
        let mut view = self.view(cols, rows);
        view.centre.0 += cols_share * f64::from(cols) * 2.0 / view.scale;
        view.centre.1 += rows_share * f64::from(rows) * 4.0 / view.scale;
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
        self.moved_to(moment);
        self.centre_on_selected();
    }

    /// When a moment was made, for walking in time: its time, then its
    /// commit (never its place in the reading, which is git's choice).
    fn when(&self, m: usize) -> (i64, &str) {
        let moment = &self.history.moments[m];
        (moment.time, &moment.id)
    }

    /// The marks and the wall's place, oldest first: what Tab walks.
    fn stops(&self) -> Vec<usize> {
        let mut stops: Vec<usize> = self.history.marks.iter().map(|m| m.moment).collect();
        stops.extend(self.history.here);
        stops.sort_by(|&a, &b| self.when(a).cmp(&self.when(b)));
        stops.dedup();
        stops
    }

    fn next_stop(&mut self, forward: bool) {
        let stops = self.stops();
        if stops.is_empty() {
            "no marks yet: nothing to jump to".clone_into(&mut self.said);
            return;
        }
        let next = self.selected.map_or(stops[0], |at| {
            let now = self.when(at);
            if forward {
                stops
                    .iter()
                    .copied()
                    .find(|&m| self.when(m) > now)
                    .unwrap_or(stops[0])
            } else {
                stops
                    .iter()
                    .rev()
                    .copied()
                    .find(|&m| self.when(m) < now)
                    .unwrap_or(stops[stops.len() - 1])
            }
        });
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
            f64::from(col).mul_add(2.0, 1.0),
            f64::from(row).mul_add(4.0, 2.0),
        );
        // Only what is within reach of the click is looked at: six dots
        // either way, in world units.
        let reach = 6.0 / view.scale;
        let click = view.to_world(col, row, cols, rows);
        let near = (
            click.0 - reach,
            click.1 - reach,
            click.0 + reach,
            click.1 + reach,
        );
        let mut best: Option<(f64, usize)> = None;
        let mut consider = |m: usize| {
            if let Some(p) = self.growth.at.get(m).copied().flatten()
                && p.0 >= near.0
                && p.0 <= near.2
                && p.1 >= near.1
                && p.1 <= near.3
            {
                let (x, y) = view.to_dots(p, cols, rows);
                let d = libm::hypot(x - target.0, (y - target.1) * 0.5);
                if d <= 6.0 && best.is_none_or(|(b, _)| d < b) {
                    best = Some((d, m));
                }
            }
        };
        for step in &self.growth.steps {
            let pad = step.widest + reach;
            if step.start.0.max(step.end.0) + pad < click.0
                || step.start.0.min(step.end.0) - pad > click.0
                || step.start.1.max(step.end.1) + pad < click.1
                || step.start.1.min(step.end.1) - pad > click.1
            {
                continue;
            }
            let moments = &self.history.limbs[step.limb].moments[step.first..=step.last];
            if level >= Level::Every {
                moments.iter().copied().for_each(&mut consider);
            } else if let Some(&last) = moments.last() {
                consider(last);
            }
        }
        self.history
            .marks
            .iter()
            .for_each(|mark| consider(mark.moment));
        self.history.here.into_iter().for_each(&mut consider);
        if level == Level::Close {
            self.gestures.iter().for_each(|g| consider(g.last));
        }
        best.map(|(_, m)| m)
    }

    /// A key.
    pub fn key(&mut self, key: KeyEvent) {
        if self.help {
            match key.code {
                KeyCode::Char('?' | 'q') | KeyCode::Esc => self.help = false,
                KeyCode::Up | KeyCode::Char('k') => self.help_top = self.help_top.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => self.help_top += 1,
                KeyCode::PageUp => self.help_top = self.help_top.saturating_sub(10),
                KeyCode::PageDown | KeyCode::Char(' ') => self.help_top += 10,
                KeyCode::Home => self.help_top = 0,
                _ => {}
            }
            return;
        }
        if self.shown == Shown::Lanes && self.lane_key(key.code) {
            return;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let step = if shift { FAR } else { 0.125 };
        match key.code {
            // The arrows walk the changes; with Shift they pan.
            KeyCode::Up if !shift => self.step(1),
            KeyCode::Down if !shift => self.step(-1),
            KeyCode::Right if !shift => self.step_into_branch(),
            KeyCode::Left if !shift => self.step_out_of_branch(),
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit = true,
            KeyCode::Char('?') => {
                self.help = true;
                self.help_top = 0;
            }
            KeyCode::Char('y') => self.copy_selected(),
            KeyCode::Enter | KeyCode::Char('g') => self.go_now(),
            KeyCode::Char('b') => self.fire.toggle().clone_into(&mut self.said),
            KeyCode::Char('f') => {
                self.follow = match self.follow {
                    Follow::Off => Follow::On(None),
                    Follow::On(_) => Follow::Off,
                };
                (if self.follow == Follow::On(None) {
                    "following: the wall goes where the selection rests"
                } else {
                    "not following: Enter puts the wall at the selected moment"
                })
                .clone_into(&mut self.said);
            }
            KeyCode::Left | KeyCode::Char('h') => self.pan(-step, 0.0),
            KeyCode::Right | KeyCode::Char('l') => self.pan(step, 0.0),
            KeyCode::Up | KeyCode::Char('k') => self.pan(0.0, step),
            KeyCode::Down | KeyCode::Char('j') => self.pan(0.0, -step),
            // Shift with a letter arrives as the capital, Shift or not.
            KeyCode::Char('H') => self.pan(-FAR, 0.0),
            KeyCode::Char('L') => self.pan(FAR, 0.0),
            KeyCode::Char('K') => self.pan(0.0, FAR),
            KeyCode::Char('J') => self.pan(0.0, -FAR),
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
            KeyCode::Char('c') => self.centre_on_selected(),
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('v') => {
                self.shown = match self.shown {
                    Shown::Tree => Shown::Lanes,
                    Shown::Lanes => Shown::Tree,
                };
            }
            _ => {}
        }
    }

    /// Put the wall at the selected moment now (`g`).
    fn go_now(&mut self) {
        let Some(m) = self.selected else {
            "select a moment to put the wall at first".clone_into(&mut self.said);
            return;
        };
        if self.history.here == Some(m) {
            "the wall is at this moment already".clone_into(&mut self.said);
            return;
        }
        let moment = &self.history.moments[m];
        self.said = format!("putting the wall at {} …", short(&moment.id));
        if let Follow::On(moved) = &mut self.follow {
            *moved = None;
        }
        self.go = Go::Send(Arc::clone(&moment.id));
    }

    /// The moment to put the wall at now, if any: one asked for with `g`,
    /// or, while following, the selection the user moved to once it has
    /// rested [`REST`] and the wall is not there already. One go at a
    /// time: while one is on its way, the latest rest waits for it.
    pub fn go_due(&mut self, now: Instant) -> Option<Arc<str>> {
        match &self.go {
            Go::Send(moment) => {
                let moment = Arc::clone(moment);
                self.go = Go::Sent;
                return Some(moment);
            }
            Go::Sent => return None,
            Go::Idle => {}
        }
        let Follow::On(moved) = &mut self.follow else {
            return None;
        };
        let (moment, at) = moved.clone()?;
        if now.duration_since(at) < REST {
            return None;
        }
        *moved = None;
        let here = self.history.here.map(|m| &self.history.moments[m].id);
        if here == Some(&moment) {
            return None;
        }
        self.said = format!("putting the wall at {} …", short(&moment));
        self.go = Go::Sent;
        Some(moment)
    }

    /// Whether a follow is waiting for the selection to rest, so the loop
    /// looks again soon.
    #[must_use]
    pub const fn resting(&self) -> bool {
        matches!(self.follow, Follow::On(Some(_)))
    }

    /// The user moved the selection: follow it once it rests.
    fn moved_to(&mut self, m: usize) {
        if let Follow::On(moved) = &mut self.follow {
            *moved = Some((Arc::clone(&self.history.moments[m].id), Instant::now()));
        }
    }

    /// How far moment `m` is from the wall, along the tree, and what
    /// Enter does there: worked out each time the details are drawn, so it
    /// is right however the wall has moved since.
    #[must_use]
    pub fn whereabouts(&self, m: usize) -> String {
        let changes = |n: usize| {
            if n == 1 {
                "1 change".to_owned()
            } else {
                format!("{n} changes")
            }
        };
        let go = if matches!(self.follow, Follow::On(_)) {
            "the wall follows when you stop"
        } else {
            "Enter puts the wall here"
        };
        match self.history.here {
            Some(here) if here == m => "the wall is here".to_owned(),
            Some(here) => match self.history.path_between(here, m) {
                Some((0, on)) => format!("{} on from the wall · {go}", changes(on)),
                Some((back, 0)) => format!("{} back from the wall · {go}", changes(back)),
                Some((back, on)) => format!(
                    "{} from the wall: {back} back to where they part, {on} on · {go}",
                    changes(back + on)
                ),
                None => format!("on no path from the wall · {go}"),
            },
            None => go.to_owned(),
        }
    }

    /// One moment on along the selected limb (`by` 1) or back toward the
    /// root (`by` -1), from the wall's place when nothing is selected.
    fn step(&mut self, by: isize) {
        if self.selected.is_none() {
            self.selected = self.history.here;
        }
        self.walk(by);
    }

    /// Into the nearest branch: the first bud that grows from the selected
    /// limb at or ahead of the selected moment (or, on a bud's own first
    /// moment, its next sister from the same moment).
    fn step_into_branch(&mut self) {
        let Some(m) = self.selected.or(self.history.here) else {
            return;
        };
        let Some((limb, place)) = self.history.place_of(m) else {
            return;
        };
        let history = &self.history;
        let first_of = |l: usize| history.limbs[l].moments.first().copied();
        // On a bud's first moment: its next sister, if it has one.
        let own = &history.limbs[limb];
        if place == 0
            && let Some(from) = own.from
            && let Some(sister) = (limb + 1..history.limbs.len())
                .find(|&l| history.limbs[l].from == Some(from) && first_of(l).is_some())
            && let Some(first) = first_of(sister)
        {
            self.select(first);
            return;
        }
        let ahead = (0..history.limbs.len())
            .filter_map(|l| {
                let from = history.limbs[l].from?;
                let (on, at) = history.place_of(from)?;
                (on == limb && at >= place && first_of(l).is_some()).then_some((at, l))
            })
            .min();
        match ahead.and_then(|(_, l)| first_of(l)) {
            Some(first) => {
                self.select(first);
            }
            None => "no branch grows from here on".clone_into(&mut self.said),
        }
    }

    /// Back out of a branch, to the moment it grew from.
    fn step_out_of_branch(&mut self) {
        let Some(m) = self.selected.or(self.history.here) else {
            return;
        };
        let from = self
            .history
            .place_of(m)
            .and_then(|(limb, _)| self.history.limbs[limb].from);
        match from {
            Some(from) => {
                self.select(from);
            }
            None => "this is the trunk: nothing to step back out to".clone_into(&mut self.said),
        }
    }

    /// Put the selected moment's whole commit on the terminal's clipboard.
    fn copy_selected(&mut self) {
        match self.selected {
            Some(m) => {
                let id = self.history.moments[m].id.to_string();
                self.said = format!("copied {id}");
                self.yank = Some(id);
            }
            None => "nothing is selected to copy".clone_into(&mut self.said),
        }
    }

    /// The selected moment's row in the lanes view.
    #[must_use]
    pub fn selected_row(&self) -> Option<usize> {
        let m = self.selected?;
        self.lanes.rows.iter().position(|row| row.moment == m)
    }

    /// A key the lanes view takes for itself: up and down move a row,
    /// a page moves a screen. Answers whether it was taken.
    fn lane_key(&mut self, code: KeyCode) -> bool {
        let page = usize::from(self.area.1.max(1));
        let at = self.selected_row().unwrap_or(0);
        let to = match code {
            KeyCode::Up | KeyCode::Char('k') => at.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => at + 1,
            KeyCode::PageUp => at.saturating_sub(page),
            KeyCode::PageDown => at + page,
            KeyCode::Home => 0,
            KeyCode::End => usize::MAX,
            _ => return false,
        };
        self.select_row(to);
        true
    }

    /// Select row `to` of the lanes view, or the last when past it.
    fn select_row(&mut self, to: usize) {
        if let Some(last) = self.lanes.rows.len().checked_sub(1) {
            let moment = self.lanes.rows[to.min(last)].moment;
            self.selected = Some(moment);
            self.moved_to(moment);
        }
    }

    /// Keep the selected row in view in a lanes view `height` rows tall;
    /// answers the first row to show.
    pub fn lanes_from(&mut self, height: u16) -> usize {
        let height = usize::from(height.max(1));
        if let Some(at) = self.selected_row() {
            if at < self.lane_top {
                self.lane_top = at;
            } else if at >= self.lane_top + height {
                self.lane_top = at + 1 - height;
            }
        }
        self.lane_top = self.lane_top.min(self.lanes.rows.len().saturating_sub(1));
        self.lane_top
    }

    /// A mouse event over the picture (`col`, `row` inside it).
    pub fn mouse(&mut self, event: MouseEvent) {
        let (cols, rows) = self.area;
        let inside = event.column < cols && event.row < rows;
        if self.shown == Shown::Lanes {
            let at = self.selected_row().unwrap_or(0);
            match event.kind {
                MouseEventKind::Down(MouseButton::Left) if inside => {
                    let row = self.lane_top + usize::from(event.row);
                    if row < self.lanes.rows.len() {
                        self.select_row(row);
                    }
                }
                MouseEventKind::ScrollUp if inside => self.select_row(at.saturating_sub(3)),
                MouseEventKind::ScrollDown if inside => self.select_row(at + 3),
                _ => {}
            }
            return;
        }
        let (col, row) = (event.column, event.row);
        match event.kind {
            MouseEventKind::ScrollUp if inside => self.zoom_about(ZOOM, Some((col, row))),
            MouseEventKind::ScrollDown if inside => self.zoom_about(1.0 / ZOOM, Some((col, row))),
            MouseEventKind::Down(MouseButton::Left) if inside => {
                if let Some(m) = self.nearest(col, row) {
                    self.selected = Some(m);
                    self.moved_to(m);
                }
                let centre = self.view(cols, rows).centre;
                self.drag = Some(((col, row), centre));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(((c0, r0), centre)) = self.drag {
                    let mut view = self.view(cols, rows);
                    let dx = (f64::from(col) - f64::from(c0)) * 2.0 / view.scale;
                    let dy = (f64::from(row) - f64::from(r0)) * 4.0 / view.scale;
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
        if let Some(batch) = moment.batch.as_deref() {
            lines[0].1.clone_from(&batch.note);
            match batch.part {
                Some((part, count)) if part < count => {
                    lines.push((
                        "batch".to_owned(),
                        format!("change {part} of {count}, sent at once"),
                    ));
                    for step in &batch.steps {
                        lines.push((format!("{part}"), step.clone()));
                    }
                }
                _ => {
                    lines.push((
                        "batch".to_owned(),
                        format!("{}, sent at once", crate::changes(batch.steps.len())),
                    ));
                    for (i, step) in batch.steps.iter().enumerate() {
                        let said = batch
                            .comments
                            .iter()
                            .find(|(number, _)| *number == i + 1)
                            .map_or_else(
                                || step.clone(),
                                |(_, comment)| format!("{step} \u{2014} \"{comment}\""),
                            );
                        lines.push((format!("{}", i + 1), said));
                    }
                }
            }
        }
        if let Some(comment) = &moment.comment {
            lines.push(("why".to_owned(), comment.clone()));
        }
        lines.push(("who".to_owned(), moment.seat.clone()));
        lines.push(("when".to_owned(), local_time(moment.time)));
        if self.history.here != Some(m) {
            lines.push(("wall".to_owned(), self.whereabouts(m)));
        }
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
        for problem in &moment.problems {
            lines.push(("problem".to_owned(), problem.clone()));
        }
        lines.push(("commit".to_owned(), moment.id.to_string()));
        for problem in &self.history.problems {
            lines.push(("tree".to_owned(), problem.clone()));
        }
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
            ("h j k l, Shift-arrows", "pan (H J K L: further)"),
            ("drag", "pan with the mouse"),
            ("+ - or wheel", "zoom in and out"),
            ("0", "the whole tree"),
        ],
    ),
    (
        "find a moment",
        &[
            ("click", "select the nearest moment"),
            ("Tab, Shift-Tab", "next or previous mark or the wall"),
            ("up down, [ ]", "one change on or back"),
            ("right", "into the nearest branch ahead"),
            ("left", "back to where this branch grew"),
            ("{ }", "sixteen moments back or on"),
            (".", "where the wall is"),
            ("c", "centre on the selected moment"),
        ],
    ),
    (
        "the tree",
        &[
            ("v", "the tree, or lanes: one row a moment"),
            ("y", "copy the selected moment's commit"),
            ("Enter, g", "put the wall at the selected moment"),
            ("f", "follow: the wall goes where you stop (off)"),
            ("b", "the wall's moment ablaze, or put out (on)"),
            ("Shift-drag", "select text (Option-drag in iTerm2)"),
            ("r", "read the tree again"),
            ("?", "this card"),
            ("q, Esc", "quit"),
        ],
    ),
    (
        "in the lanes",
        &[
            ("up down, j k", "one row up or down"),
            ("PgUp PgDn", "a screen up or down"),
            ("Home End", "the newest or the oldest"),
            ("click, wheel", "select a row, move three"),
        ],
    ),
    (
        "what you see",
        &[
            ("◉", "where the wall is"),
            ("flames, sparks", "the moment the wall plays, ablaze"),
            ("◆ ▪", "a mark, a save's mark"),
            ("•", "a gesture or a moment (zoomed in)"),
            ("╴ Wed 14 Oct 2026", "a branch's day, or a new day"),
            ("✧", "a tip that grew in the last hour"),
            ("bright limb", "the wall's own line"),
            ("thickness", "modules alive"),
            ("colour", "the modules and their settings"),
            ("lighter", "louder, when its sound was kept"),
            ("grey", "nothing but plumbing alive"),
            ("roots", "where a tree began"),
        ],
    ),
];

/// A commit's first twelve characters, as the wall shows short ids.
#[must_use]
pub fn short(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

#[cfg(test)]
mod tests;
