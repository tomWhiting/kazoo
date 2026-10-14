//! The fire as the app keeps it.
//!
//! Lit or out (`b`); the wall's moment's fire and the one it left, dying
//! down; where each was last drawn; its frame clock;
//! and the tree's picture kept between frames where only the fire moved,
//! so a fire frame never draws the tree again.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
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

/// The fire, as the app keeps it: the wall's moment burning, and the
/// moment it was at before dying down where it was.
#[derive(Debug)]
pub struct Blaze {
    /// Lit (the default) or out: `b` turns it.
    pub on: bool,
    /// The next frame only the fire moved: the kept picture of the tree
    /// may be drawn again if it was drawn from the same. Taken by the
    /// frame that reads it.
    reuse: bool,
    /// The wall's moment, burning.
    now: Fire,
    /// The moment the wall left, dying down.
    then: Fire,
    /// Whether any of either fire was on screen at the last frame.
    seen: bool,
    /// When the fire last burned on.
    last: Option<Instant>,
    kept: Option<(Key, Picture)>,
}

impl Default for Blaze {
    fn default() -> Self {
        Self {
            on: true,
            reuse: false,
            now: Fire::new(),
            then: Fire::new(),
            seen: false,
            last: None,
            kept: None,
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
        self.then.light(None);
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
        self.reuse = true;
    }

    /// The tree read has changed: the kept picture is never drawn again.
    pub fn forget(&mut self) {
        self.kept = None;
        self.reuse = false;
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

    /// The field of the moment the wall left, dying down, while it burns:
    /// for tests and measures.
    #[must_use]
    pub const fn left_behind(&self) -> Option<&Field> {
        if self.then.id.is_some() {
            Some(&self.then.field)
        } else {
            None
        }
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
    /// the one it left dying down until it is cold.
    pub fn burn(&mut self, dt: f32) {
        if self.now.id.is_some() {
            self.now.tend(dt, true);
            self.now.field.step(dt);
        }
        if self.then.id.is_some() {
            self.then.tend(dt, false);
            self.then.field.step(dt);
            if self.then.lit <= 0.0 && self.then.ember <= 0.0 && self.then.field.is_cold() {
                self.then.light(None);
            }
        }
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
        let reuse = std::mem::take(&mut self.reuse);
        if !reuse || self.kept.as_ref().is_none_or(|(kept, _)| *kept != key) {
            self.kept = None;
        }
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
    }

    /// Draw the fire for `scene` through `view` over the tree's picture
    /// already drawn in `area`, when lit: the moment the wall left first,
    /// dying down where it was, then the wall's moment over it.
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
            if self.now.id.is_some() {
                // The wall moved on: what burned dies down where it was.
                std::mem::swap(&mut self.now, &mut self.then);
            }
            self.now.light(here_id);
        }
        let then = self
            .then
            .id
            .as_deref()
            .and_then(|id| scene.history.moment_of(id));
        if self.then.id.is_some() && then.is_none() {
            // Its moment is no longer on the tree read.
            self.then.light(None);
        }
        let mut seen = false;
        for (fire, moment) in [(&mut self.then, then), (&mut self.now, here)] {
            fire.place =
                moment.and_then(|m| Placement::of(scene, view, area.width, area.height, m));
            let Some(place) = fire.place else { continue };
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
                seen = true;
                paint(&fire.field, &place, area, buf);
            }
        }
        if seen {
            self.seen = true;
        } else {
            self.out_of_view();
        }
    }
}
