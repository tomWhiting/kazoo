//! The fire as the app keeps it.
//!
//! Lit or out (`b`), its field, where it was last drawn, its frame clock,
//! and the tree's picture kept between frames where only the fire moved,
//! so a fire frame never draws the tree again.

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

/// The fire, as the app keeps it.
#[derive(Debug)]
pub struct Blaze {
    /// Lit (the default) or out: `b` turns it.
    pub on: bool,
    /// The next frame only the fire moved: the kept picture of the tree
    /// may be drawn again if it was drawn from the same. Taken by the
    /// frame that reads it.
    reuse: bool,
    field: Field,
    place: Option<Placement>,
    /// Whether any of the fire was on screen at the last frame.
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
            field: Field::new(shape_of(Level::Far, 1.0, 80, 24)),
            place: None,
            seen: false,
            last: None,
            kept: None,
        }
    }
}

impl Blaze {
    /// Light the fire, or put it out; answers what the footer says.
    pub const fn toggle(&mut self) -> &'static str {
        self.on = !self.on;
        self.last = None;
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

    /// Where the fire was placed at the last frame, if it was drawn.
    #[must_use]
    pub const fn placed(&self) -> Option<Placement> {
        if self.on && self.seen {
            self.place
        } else {
            None
        }
    }

    /// The field, for tests and measures.
    #[must_use]
    pub const fn field(&self) -> &Field {
        &self.field
    }

    /// Burn on to `now`: by the time since the last tick. The first tick
    /// after lighting it, or after it was out of view, burns nothing.
    pub fn tick(&mut self, now: Instant) {
        let dt = self.last.map_or(0.0, |last| {
            now.saturating_duration_since(last).as_secs_f32()
        });
        self.last = Some(now);
        self.field.step(dt);
    }

    /// Burn on by `dt` seconds, with no clock: for tests and measures.
    pub fn burn(&mut self, dt: f32) {
        self.field.step(dt);
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
    /// already drawn in `area`, when lit.
    pub fn draw(&mut self, scene: &Scene<'_>, view: &View, area: Rect, buf: &mut Buffer) {
        if !self.on {
            self.out_of_view();
            return;
        }
        self.place = Placement::of(scene, view, area.width, area.height);
        let Some(place) = self.place else {
            self.out_of_view();
            return;
        };
        if place.shape != self.field.shape() {
            self.field.reshape(place.shape);
        }
        if place.visible(area.width, area.height) {
            self.seen = true;
            paint(&self.field, &place, area, buf);
        } else {
            self.out_of_view();
        }
    }
}
