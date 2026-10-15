//! The fire as the app keeps it.
//!
//! Lit or out (`b`); the wall's moment's fire and the one it left, dying
//! down; where each was last drawn; its frame clock;
//! and the tree's picture kept between frames where only the fire moved,
//! so a fire frame never draws the tree again.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;

use super::field::Field;
use super::paint::paint;
use super::place::{Placement, shape_of};
use crate::render::{self, Level, Picture, Scene, View};

/// Frames a second while a fire is in view: a fast display's refresh, so
/// the turning dither blends into smooth gradients.
pub const FPS: u32 = 120;

/// The time between fire frames.
#[must_use]
pub fn period() -> Duration {
    Duration::from_secs(1) / FPS
}

/// When the frame after one due at `due` and drawn at `now` is due: a
/// period on, keeping the beat; after a gap (that frame already past), a
/// period from now, never two frames back to back.
#[must_use]
pub fn next_frame(due: Instant, now: Instant) -> Instant {
    let next = due + period();
    if next <= now { now + period() } else { next }
}

/// When fire frames are drawn: each a period after the last, counted from
/// when that one finished.
#[derive(Debug, Clone, Copy)]
pub struct Beat {
    next: Instant,
}

impl Beat {
    /// A beat whose first frame is due at `now`.
    #[must_use]
    pub const fn new(now: Instant) -> Self {
        Self { next: now }
    }

    /// Whether a fire frame is due at `now`.
    #[must_use]
    pub fn due(&self, now: Instant) -> bool {
        now >= self.next
    }

    /// A fire frame was drawn, finishing at `finished`.
    pub fn drawn(&mut self, finished: Instant) {
        self.next = next_frame(self.next, finished);
    }

    /// How long from `now` until the next fire frame is due.
    #[must_use]
    pub fn wait(&self, now: Instant) -> Duration {
        self.next.saturating_duration_since(now)
    }
}

/// What a kept picture was drawn from: a picture is kept only for the same.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Key {
    view: View,
    cols: u16,
    rows: u16,
    selected: Option<usize>,
    now: i64,
}

/// How long a moment takes to catch, from a spark to a full blaze,
/// seconds.
const CATCH: f32 = 1.4;
/// How long the flames of a moment left behind take to sink, seconds.
const SINK: f32 = 0.6;
/// How long the embers of a moment left behind glow on, seconds.
const GLOW_ON: f32 = 2.5;
/// How many moments along the first-parent path the wall's new moment may
/// have grown from the burning one and still carry its fire on: every change
/// to the wall is a moment, so a wall being played grows by one at a time;
/// more than this between two frames reads as a jump.
const CARRY: usize = 64;

/// Whether a frame may draw the tree's picture kept from the frame before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reuse {
    /// Drawn afresh.
    No,
    /// The next frame is one where only the fire moved.
    Asked,
    /// The last frame drew the kept picture.
    Drawn,
}

/// One moment's fire: which moment, its field, how far it has caught or
/// died down, and where it was last drawn.
#[derive(Debug)]
struct Fire {
    /// The moment's commit: what it is known by when the tree is read
    /// again.
    id: Option<Arc<str>>,
    field: Field,
    /// How far it has caught, `0.0..=1.0`: down again once left behind.
    lit: f32,
    /// How brightly its embers glow, `0.0..=1.0`.
    ember: f32,
    place: Option<Placement>,
}

impl Fire {
    fn new() -> Self {
        let mut field = Field::new(shape_of(Level::Far, 1.0, 80, 24));
        field.clear();
        Self {
            id: None,
            field,
            lit: 0.0,
            ember: 0.0,
            place: None,
        }
    }

    /// Light it on the moment `id`, from cold.
    fn light(&mut self, id: Option<Arc<str>>) {
        self.id = id;
        self.field.clear();
        self.lit = 0.0;
        self.ember = 0.0;
        self.place = None;
    }

    /// Catch, or die down, by `dt` seconds: `burning` while it is the
    /// wall's moment.
    fn tend(&mut self, dt: f32, burning: bool) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        if burning {
            self.lit = (self.lit + dt / CATCH).min(1.0);
            self.ember = self.lit;
        } else {
            self.lit = (self.lit - dt / SINK).max(0.0);
            self.ember = (self.ember - dt / GLOW_ON).max(0.0);
        }
        let gain = if burning {
            // A spark first, a small lick, then all at once.
            self.lit * self.lit * 2.0f32.mul_add(-self.lit, 3.0)
        } else {
            // Sinking from the moment it is left, fastest at first.
            self.lit * self.lit
        };
        self.field.feed_by(gain, self.ember);
    }
}

/// The fire, as the app keeps it: the wall's moment burning, and every
/// moment it has just left dying down where it was.
#[derive(Debug)]
pub struct Blaze {
    /// Lit (the default) or out: `b` turns it.
    pub on: bool,
    /// Whether the next frame is one where only the fire moved (the kept
    /// picture of the tree may be drawn again if it was drawn from the
    /// same; taken by the frame that reads it), or the last drew it.
    reuse: Reuse,
    /// The wall's moment, burning.
    now: Fire,
    /// The moments the wall has left, dying down, the oldest first: each
    /// until it is cold, a few seconds.
    dying: Vec<Fire>,
    /// Fires burned out, kept to be lit again, so a move of the wall
    /// allocates nothing once there have been as many fires at once.
    spare: Vec<Fire>,
    /// Whether any fire was on screen at the last frame.
    seen: bool,
    /// When the fire last burned on.
    last: Option<Instant>,
    kept: Option<(Key, Picture)>,
    /// The cells of the panel and the footer from the last frame drawn
    /// afresh, and the screen they were drawn on: drawn again as they were
    /// on a frame where only the fire moved.
    chrome: Option<(Rect, Vec<Cell>)>,
}

impl Default for Blaze {
    fn default() -> Self {
        Self {
            on: true,
            reuse: Reuse::No,
            now: Fire::new(),
            dying: Vec::new(),
            spare: Vec::new(),
            seen: false,
            last: None,
            kept: None,
            chrome: None,
        }
    }
}

impl Blaze {
    /// Light the fire, or put it out; answers what the footer says. Lit
    /// again, the wall's moment catches from a spark.
    pub fn toggle(&mut self) -> &'static str {
        self.on = !self.on;
        self.last = None;
        self.now.light(None);
        while let Some(mut fire) = self.dying.pop() {
            fire.light(None);
            self.spare.push(fire);
        }
        if self.on {
            "the wall's moment is ablaze (b puts it out)"
        } else {
            "the fire is out (b lights it again)"
        }
    }

    /// The next frame is one where only the fire moved: the tree's picture
    /// kept from the last frame may be drawn again, if it was drawn from the
    /// same view, size, selection and second.
    pub const fn fire_only(&mut self) {
        self.reuse = Reuse::Asked;
    }

    /// The tree read has changed: the kept picture is never drawn again.
    pub fn forget(&mut self) {
        self.kept = None;
        self.reuse = Reuse::No;
    }

    /// Whether the fire is lit and was in view at the last frame: while it
    /// is, the screen is drawn every [`period`]; otherwise only when
    /// something happens.
    #[must_use]
    pub const fn live(&self) -> bool {
        self.on && self.seen
    }

    /// Where the wall's moment's fire was placed at the last frame, if it
    /// was drawn.
    #[must_use]
    pub const fn placed(&self) -> Option<Placement> {
        if self.on && self.seen {
            self.now.place
        } else {
            None
        }
    }

    /// The wall's moment's field, for tests and measures.
    #[must_use]
    pub const fn field(&self) -> &Field {
        &self.now.field
    }

    /// The field of the moment the wall left last, dying down, while it
    /// burns: for tests and measures.
    #[must_use]
    pub fn left_behind(&self) -> Option<&Field> {
        self.dying.last().map(|fire| &fire.field)
    }

    /// How far the wall's moment's fire has caught, `0.0..=1.0`: for tests
    /// and measures.
    #[must_use]
    pub const fn caught(&self) -> f32 {
        self.now.lit
    }

    /// How many spent fires are kept to be lit again: for tests and
    /// measures.
    #[must_use]
    pub fn spares(&self) -> usize {
        self.spare.len()
    }

    /// Whether the last frame drew the tree's picture kept from the frame
    /// before.
    #[must_use]
    pub const fn reused(&self) -> bool {
        matches!(self.reuse, Reuse::Drawn)
    }

    /// How many moments the wall has left are still dying down.
    #[must_use]
    pub fn dying(&self) -> usize {
        self.dying.len()
    }

    /// Burn on to `now`: by the time since the last tick. The first tick
    /// after lighting it, or after it was out of view, burns nothing.
    pub fn tick(&mut self, now: Instant) {
        let dt = self.last.map_or(0.0, |last| {
            now.saturating_duration_since(last).as_secs_f32()
        });
        self.last = Some(now);
        self.burn(dt);
    }

    /// Burn on by `dt` seconds, with no clock: the wall's moment catching,
    /// the ones it left dying down until each is cold.
    pub fn burn(&mut self, dt: f32) {
        if self.now.id.is_some() {
            self.now.tend(dt, true);
            self.now.field.step(dt);
        }
        let mut i = 0;
        while i < self.dying.len() {
            let fire = &mut self.dying[i];
            fire.tend(dt, false);
            fire.field.step(dt);
            if fire.lit <= 0.0 && fire.ember <= 0.0 && fire.field.is_cold() {
                self.retire(i);
            } else {
                i += 1;
            }
        }
        // Once nothing dies down, one spent fire is kept to be lit again and
        // the rest let go: a burst of moves leaves no memory behind.
        if self.dying.is_empty() && self.spare.len() > 1 {
            self.spare.truncate(1);
            self.spare.shrink_to_fit();
        }
    }

    /// The dying fire at `i` is out: kept to be lit again.
    fn retire(&mut self, i: usize) {
        let mut fire = self.dying.remove(i);
        fire.light(None);
        self.spare.push(fire);
    }

    /// The wall has moved to `here`: the moment it left dies down where it
    /// was, and `here` catches; from where it was, if it is still dying
    /// down from the wall being there before, else from a spark.
    fn move_to(&mut self, here: Option<Arc<str>>) {
        let back = here.as_ref().and_then(|id| {
            self.dying
                .iter()
                .position(|fire| fire.id.as_ref() == Some(id))
        });
        let next = if let Some(i) = back {
            self.dying.remove(i)
        } else {
            let mut fire = self.spare.pop().unwrap_or_else(Fire::new);
            fire.light(here);
            fire
        };
        let left = std::mem::replace(&mut self.now, next);
        if left.id.is_some() {
            self.dying.push(left);
        } else {
            self.spare.push(left);
        }
    }

    /// Whether the wall's new moment `here` grew on from the burning one:
    /// it is on `here`'s first-parent path within [`CARRY`] moments, not a
    /// moment still dying down (going back to one takes its own fire up),
    /// and its fire would stand within the burning fire's reach on screen,
    /// so a go further along the branch is still a jump.
    fn grew_on(&self, scene: &Scene<'_>, view: &View, area: Rect, here: Option<usize>) -> bool {
        let Some(here) = here else {
            return false;
        };
        let Some(was) = self.now.place else {
            return false;
        };
        let Some(next) = Placement::of(scene, view, area.width, area.height, here) else {
            return false;
        };
        let (dx, dy) = (
            f64::from(next.anchor.0) - f64::from(was.anchor.0),
            f64::from(next.anchor.1) - f64::from(was.anchor.1),
        );
        if dx.hypot(dy) > f64::from(was.shape.reach) {
            return false;
        }
        let history = scene.history;
        let Some(from) = self.now.id.as_deref().and_then(|id| history.moment_of(id)) else {
            return false;
        };
        let here_id = history.moments.get(here).map(|moment| &moment.id);
        if self.dying.iter().any(|fire| fire.id.as_ref() == here_id) {
            return false;
        }
        let mut m = here;
        for _ in 0..CARRY {
            let Some(parent) = history.moments.get(m).and_then(|moment| moment.parent) else {
                return false;
            };
            if parent == from {
                return true;
            }
            m = parent;
        }
        false
    }

    /// The tree's picture for `scene` through `view` in `cols` × `rows`:
    /// the one kept from the last frame when only the fire has moved since
    /// and it was drawn from the same, else drawn afresh and kept.
    pub fn picture(&mut self, scene: &Scene<'_>, view: &View, cols: u16, rows: u16) -> &Picture {
        let key = Key {
            view: *view,
            cols,
            rows,
            selected: scene.selected,
            now: scene.now,
        };
        let asked = self.reuse == Reuse::Asked;
        self.reuse = if asked && self.kept.as_ref().is_some_and(|(kept, _)| *kept == key) {
            Reuse::Drawn
        } else {
            self.kept = None;
            Reuse::No
        };
        &self
            .kept
            .get_or_insert_with(|| (key, render::draw(scene, view, cols, rows)))
            .1
    }

    /// Nothing of the tree is shown (the lanes are): the fire is not in
    /// view, and burns no frames until it is again.
    pub const fn out_of_view(&mut self) {
        self.seen = false;
        self.last = None;
        self.reuse = Reuse::No;
    }

    /// Keep the cells of `parts` (the panel and the footer) of `buf`, drawn
    /// afresh on a screen of `area`, to be drawn again on fire-only frames.
    pub fn keep_chrome(&mut self, area: Rect, parts: [Rect; 2], buf: &Buffer) {
        let (kept, cells) = self.chrome.get_or_insert_with(|| (area, Vec::new()));
        *kept = area;
        cells.clear();
        for part in parts {
            for y in part.top()..part.bottom() {
                for x in part.left()..part.right() {
                    cells.push(buf[(x, y)].clone());
                }
            }
        }
    }

    /// Draw the panel and the footer kept from the last frame into `parts`
    /// of `buf`, when the last frame drew the kept picture and the screen is
    /// the same: answers whether they were, else they must be worked out.
    pub fn paint_chrome(&self, area: Rect, parts: [Rect; 2], buf: &mut Buffer) -> bool {
        let Some((kept, cells)) = &self.chrome else {
            return false;
        };
        if self.reuse != Reuse::Drawn || *kept != area {
            return false;
        }
        let mut cells = cells.iter();
        for part in parts {
            for y in part.top()..part.bottom() {
                for x in part.left()..part.right() {
                    let Some(cell) = cells.next() else {
                        return false;
                    };
                    buf[(x, y)] = cell.clone();
                }
            }
        }
        true
    }

    /// Draw the fire for `scene` through `view` over the tree's picture
    /// already drawn in `area`, when lit: the moments the wall left first,
    /// dying down where they were, then the wall's moment over them.
    pub fn draw(&mut self, scene: &Scene<'_>, view: &View, area: Rect, buf: &mut Buffer) {
        if !self.on {
            self.out_of_view();
            return;
        }
        let here = scene.history.here;
        let here_id = here
            .and_then(|m| scene.history.moments.get(m))
            .map(|m| m.id.clone());
        if self.now.id != here_id {
            if self.grew_on(scene, view, area, here) {
                // The wall grew on from its moment: the same fire burns on,
                // and moves with it.
                self.now.id = here_id;
            } else {
                self.move_to(here_id);
            }
        }
        let mut seen = false;
        let mut i = 0;
        while i < self.dying.len() {
            let moment = self.dying[i]
                .id
                .as_deref()
                .and_then(|id| scene.history.moment_of(id));
            if moment.is_none() {
                // Its moment is no longer on the tree read.
                self.retire(i);
                continue;
            }
            seen |= show(&mut self.dying[i], moment, scene, view, area, buf);
            i += 1;
        }
        seen |= show(&mut self.now, here, scene, view, area, buf);
        if seen {
            self.seen = true;
        } else {
            self.out_of_view();
        }
    }
}

/// Place `fire` on `moment` for `scene` through `view`, and draw it over
/// `area` if any of it is in view: answers whether it was.
fn show(
    fire: &mut Fire,
    moment: Option<usize>,
    scene: &Scene<'_>,
    view: &View,
    area: Rect,
    buf: &mut Buffer,
) -> bool {
    fire.place = moment.and_then(|m| Placement::of(scene, view, area.width, area.height, m));
    let Some(place) = fire.place else {
        return false;
    };
    fire.field.keep_time(
        moment
            .and_then(|m| scene.history.moments.get(m))
            .and_then(|m| m.tempo),
    );
    if place.shape != fire.field.shape() {
        fire.field.reshape(place.shape);
    }
    fire.field.burn_on(&place.fuel);
    if place.visible(area.width, area.height) {
        paint(&fire.field, &place, area, buf);
        true
    } else {
        false
    }
}
