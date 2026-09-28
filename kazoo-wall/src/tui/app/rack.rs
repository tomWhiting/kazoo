//! The rack view's state between frames: its layout (made again only when
//! the wall's shape changes), the cables' paths, the pan, whatever the
//! mouse is holding, and a module being moved.
//!
//! A move shows at once: the rows it makes are drawn while it waits to be
//! sent, and once sent until the wall's own rows show it (or
//! [`MOVE_LONGEST`] has passed, or the wall refuses it). Moves made with keys are gathered, like turns, and go to
//! the wall as one when the keys rest.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use kazoo_wall::protocol::ModuleView;

use super::super::knob::Travel;
use super::super::rack::geometry::{self, Density, Hanging, Hit, LayoutKey, Point, Rack};

/// How close to the rack's edge (in cells) a carried cable pans the rack.
pub const EDGE: u16 = 2;

/// How often a cable held at the edge pans the rack a step...
pub const EDGE_STEP: Duration = Duration::from_millis(50);

/// ...of this many columns...
pub const EDGE_COLUMNS: u32 = 3;

/// ...or rows.
pub const EDGE_ROWS: u32 = 1;

/// How long a sent move is shown ahead of the wall showing it.
pub const MOVE_LONGEST: Duration = Duration::from_secs(2);

/// When a move goes to the wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// When its keys rest, or have been held long enough: gathered since
    /// `first`, the latest key at `last`.
    Keys {
        /// The first key.
        first: Instant,
        /// The latest.
        last: Instant,
    },
    /// As soon as the flood guard allows (a faceplate dropped, an undone
    /// move).
    Now,
}

/// A module being moved, not yet sent: the rows it makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    /// The module.
    pub module: String,
    /// The rows with it moved.
    pub rows: Vec<Vec<String>>,
    /// The rows before it moved, to put it back with `z`; `None` for a
    /// move that puts one back.
    pub from: Option<Vec<Vec<String>>>,
    /// When it goes.
    pub due: Due,
}

/// What the mouse is holding.
#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    /// A knob, turned by moving up and down.
    Knob {
        /// The module.
        module: String,
        /// The knob.
        knob: String,
        /// Its travel.
        travel: Travel,
        /// The row the turn is measured from.
        anchor_row: u16,
        /// Where it was at that row: its position along the sweep, or its
        /// value for a stepped knob.
        anchor: f64,
        /// Whether the turn is fine (shift held).
        fine: bool,
        /// Whether the mouse has moved off the row it grabbed the knob on.
        moved: bool,
    },
    /// A cable being carried from a jack.
    Cable {
        /// The module it comes from.
        module: String,
        /// The jack.
        name: String,
        /// Whether that jack is an output.
        output: bool,
        /// Where the mouse is, on screen.
        pointer: (u16, u16),
    },
    /// A faceplate, picked up by its title bar to move it.
    Plate {
        /// The module.
        module: String,
        /// The rack laid out without it, which the mouse is measured
        /// against, so the rack reflowing round it never moves the place
        /// under the mouse.
        base: Rack,
        /// The rows without it.
        rows: Vec<Vec<String>>,
        /// The rows before it was picked up.
        from: Vec<Vec<String>>,
        /// The rows with it where the mouse is, once the mouse has moved.
        preview: Option<Vec<Vec<String>>>,
        /// Where it was picked up, on screen.
        anchor: (u16, u16),
        /// Where the mouse is, on screen.
        pointer: (u16, u16),
    },
    /// The rack, being panned.
    Pan {
        /// Where the drag began, on screen.
        anchor: (u16, u16),
        /// The pan then.
        pan: Point,
    },
}

/// How often the rack's layout and the cables' paths have been made, for
/// telling that the frames between reuse them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Builds {
    /// Layouts made.
    pub layouts: u64,
    /// Cable paths made.
    pub paths: u64,
}

/// A cable's path is known by its number and its two ends: move either end
/// and it hangs anew.
pub type PathKey = (u32, Point, Point);

/// The rack view's state: its layout as last drawn (for the mouse), its
/// pan and whatever the mouse is holding.
#[derive(Debug, Clone, Default)]
pub struct RackState {
    /// The layout, as last drawn.
    pub layout: Rack,
    /// Where on screen the rack was drawn.
    pub area: Rect,
    /// The sheet's cell at the view's top-left.
    pub pan: Point,
    /// How densely the rack is drawn (`c`; for this session).
    pub density: Density,
    /// The level the overview goes back to when a faceplate is clicked:
    /// the one it was left from.
    pub zoom_back: Density,
    /// Whether to bring the selection into view (after a key).
    pub follow: bool,
    /// Whether to put the selection in the middle of the view (`.`, or a
    /// jump).
    pub centre: bool,
    /// What the mouse holds.
    pub drag: Option<Drag>,
    /// The cable under each cell, as last drawn.
    pub cables: BTreeMap<Point, u32>,
    /// A module being moved, not yet sent.
    pub moving: Option<Move>,
    /// The rows the wall will have once it has taken the moves sent, and
    /// when the latest went: shown until the wall's own rows show them.
    pub sent: Option<(Vec<Vec<String>>, Instant)>,
    /// The latest move made here, and the rows before it: `z` puts it back
    /// while nothing else has been done since.
    pub last_move: Option<(String, Vec<Vec<String>>)>,
    /// Each cable's path, as last drawn.
    pub paths: BTreeMap<PathKey, Hanging>,
    /// How often layouts and paths have been made.
    pub builds: Builds,
    /// What the layout was made from.
    key: Option<LayoutKey>,
    pub(super) last_click: Option<(Hit, Instant)>,
    /// Since when a carried cable has been panning the rack at its edge.
    edge_since: Option<Instant>,
}

impl RackState {
    /// The sheet cell under screen cell `(column, row)`, if it is in the
    /// rack's area.
    #[must_use]
    pub fn to_sheet(&self, (column, row): (u16, u16)) -> Option<Point> {
        let area = self.area;
        let inside = column >= area.x
            && row >= area.y
            && column < area.x.saturating_add(area.width)
            && row < area.y.saturating_add(area.height);
        inside.then(|| Point {
            x: self.pan.x + u32::from(column - area.x),
            y: self.pan.y + u32::from(row - area.y),
        })
    }

    pub(super) fn hit(&self, cell: (u16, u16)) -> Option<Hit> {
        self.to_sheet(cell).map(|point| self.layout.hit(point))
    }

    /// The rows the rack shows: a faceplate carried where the mouse is, a
    /// move not yet sent, one sent but not yet shown by the wall, or else
    /// the wall's own `rows`.
    #[must_use]
    pub fn rows<'a>(&'a self, rows: Option<&'a [Vec<String>]>) -> Option<&'a [Vec<String>]> {
        if let Some(Drag::Plate {
            preview: Some(preview),
            ..
        }) = &self.drag
        {
            return Some(preview);
        }
        self.moving
            .as_ref()
            .map(|moving| moving.rows.as_slice())
            .or_else(|| self.sent.as_ref().map(|(sent, _)| sent.as_slice()))
            .or(rows)
    }

    /// Lay `modules` out at the chosen density for a view `view` (width,
    /// height), in the rows shown (see [`Self::rows`]; the wall's own are
    /// `rows`), unless the layout already is theirs.
    pub fn lay_out(
        &mut self,
        modules: &[ModuleView],
        rows: Option<&[Vec<String>]>,
        view: (u16, u16),
    ) {
        let density = self.density;
        let shown = self.rows(rows);
        if self
            .key
            .as_ref()
            .is_some_and(|key| key.fits(modules, density, view, shown))
        {
            return;
        }
        let layout = geometry::rack(modules, density, view, shown);
        let key = LayoutKey::of(modules, density, view, shown);
        self.layout = layout;
        self.key = Some(key);
        self.builds.layouts += 1;
    }

    /// Draw the rack at `density` from the next frame, keeping the
    /// selection in the middle of the view.
    pub fn set_density(&mut self, density: Density) {
        if density == Density::Overview && self.density != Density::Overview {
            self.zoom_back = self.density;
        }
        self.density = density;
        self.drag = None;
        self.last_click = None;
        self.centre = true;
    }

    /// Move the view `dx` columns and `dy` rows, held to the sheet; the
    /// selection is no longer followed.
    pub fn pan_by(&mut self, dx: i64, dy: i64) {
        let axis = |pan: u32, delta: i64| {
            let moved = i64::from(pan).saturating_add(delta).max(0);
            u32::try_from(moved).unwrap_or(u32::MAX)
        };
        let pan = Point {
            x: axis(self.pan.x, dx),
            y: axis(self.pan.y, dy),
        };
        self.pan = geometry::clamp_pan(pan, &self.layout, self.area.width, self.area.height);
        self.follow = false;
    }

    /// Pan the rack while a cable or a faceplate is carried at its edge: a
    /// step every [`EDGE_STEP`] towards that edge, for as long as it is held
    /// there.
    pub fn edge_pan(&mut self, now: Instant) {
        let Some(Drag::Cable { pointer, .. } | Drag::Plate { pointer, .. }) = &self.drag else {
            self.edge_since = None;
            return;
        };
        let pointer = *pointer;
        let (dx, dy) = edge_direction(self.area, pointer);
        if dx == 0 && dy == 0 {
            self.edge_since = None;
            return;
        }
        let Some(since) = self.edge_since else {
            self.edge_since = Some(now);
            return;
        };
        let held = now.saturating_duration_since(since);
        let steps = u32::try_from(held.as_millis() / EDGE_STEP.as_millis()).unwrap_or(u32::MAX);
        if steps == 0 {
            return;
        }
        self.edge_since = Some(since + EDGE_STEP.saturating_mul(steps));
        let steps = i64::from(steps);
        self.pan_by(
            dx * steps * i64::from(EDGE_COLUMNS),
            dy * steps * i64::from(EDGE_ROWS),
        );
    }

    /// Forget the layout: the wall is empty, or not known.
    pub fn clear(&mut self) {
        self.layout = Rack::default();
        self.key = None;
        self.cables.clear();
        self.paths.clear();
    }
}

/// Which way a pointer at screen cell `(column, row)` presses on the edges
/// of `area`: -1, 0 or 1 across and down. Within [`EDGE`] of an edge, or
/// past it, presses that way.
fn edge_direction(area: Rect, (column, row): (u16, u16)) -> (i64, i64) {
    let press = |at: u16, start: u16, size: u16| {
        let end = start.saturating_add(size);
        if size <= EDGE * 2 {
            0
        } else {
            i64::from(at >= end - EDGE) - i64::from(at < start + EDGE)
        }
    };
    (
        press(column, area.x, area.width),
        press(row, area.y, area.height),
    )
}
