//! Where the fire burns on screen, and how big, at each level of zoom.
//!
//! The fire stands on the wall's place: the source's floor straddles the
//! cell the ◉ is drawn in, and the flames rise up the screen from it. Its
//! size follows the zoom: far out a small bright ember that never covers
//! the tree, near and close flames as wide as the limb under them, right in
//! a full blaze around the dot.

use super::field::Shape;
use crate::render::{self, Level, Scene, View};

/// Where the fire is on the picture, in dots from its top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The middle of the cell the wall's glyph is drawn in.
    pub anchor: (i32, i32),
    /// The field's top left dot.
    pub origin: (i32, i32),
    /// The field's size and how it is fed.
    pub shape: Shape,
}

impl Placement {
    /// Where the fire burns for `scene` seen through `view` in `cols` ×
    /// `rows`: `None` when the wall has no place on the tree, or its
    /// place is not grown.
    #[must_use]
    pub fn of(scene: &Scene<'_>, view: &View, cols: u16, rows: u16) -> Option<Self> {
        let here = scene.history.here?;
        let at = scene.growth.at.get(here).copied().flatten()?;
        let (x, y) = view.to_dots(at, cols, rows);
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        // The cell the glyph is drawn in, and its middle dot.
        let (col, row) = ((x / 2.0).floor(), (y / 4.0).floor());
        if col.abs() > 1.0e6 || row.abs() > 1.0e6 {
            return None;
        }
        let anchor = (col as i32 * 2 + 1, row as i32 * 4 + 2);
        let trunk = scene
            .history
            .place
            .get(here)
            .copied()
            .flatten()
            .and_then(|(limb, _)| scene.history.limbs.get(limb))
            .is_some_and(|limb| limb.from.is_none());
        let limb = scene
            .growth
            .look
            .get(here)
            .copied()
            .flatten()
            .map_or(1.0, |look| render::limb_dots(look.width, trunk, view.scale))
            as f32;
        let shape = shape_of(Level::of(view.scale), limb, cols, rows);
        let (w, h) = (to_i32(shape.w), to_i32(shape.h));
        // The field's bottom row is just under the anchor, so the source's
        // floor straddles the glyph's cell.
        let origin = (anchor.0 - w / 2, anchor.1 + 2 - h);
        Some(Self {
            anchor,
            origin,
            shape,
        })
    }

    /// Whether any of the field is on a picture of `cols` × `rows`.
    #[must_use]
    pub fn visible(&self, cols: u16, rows: u16) -> bool {
        let (x, y) = self.origin;
        let (w, h) = (to_i32(self.shape.w), to_i32(self.shape.h));
        x + w > 0 && y + h > 0 && x < i32::from(cols) * 2 && y < i32::from(rows) * 4
    }
}

fn to_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Dots, rounded to a whole even number at least `least`.
fn dots(v: f32, least: usize) -> usize {
    let v = if v.is_finite() { v.max(0.0) } else { 0.0 };
    ((v / 2.0).round() as usize * 2).max(least)
}

/// The fire's shape at `level`, burning on a limb `limb` dots wide, on a
/// picture of `cols` × `rows`: never taller than most of the picture, nor
/// wider than it.
#[must_use]
pub fn shape_of(level: Level, limb: f32, cols: u16, rows: u16) -> Shape {
    let limb = if limb.is_finite() { limb.max(1.0) } else { 1.0 };
    let (source, wide, tall, floor, rate): (f32, f32, f32, usize, f32) = match level {
        // An ember: a few dots of fire and a spark now and then.
        Level::Far => (1.6, 10.0, 22.0, 2, 2.5),
        Level::Near => {
            let source = limb.mul_add(0.5, 1.5).clamp(2.0, 5.0);
            let wide = source * 5.0;
            (source, wide, wide * 1.8, 2, wide * 0.5)
        }
        Level::Close => {
            let source = limb.mul_add(0.6, 2.5).clamp(3.5, 8.0);
            let wide = source * 5.5;
            (source, wide, wide * 1.7, 3, wide * 0.6)
        }
        // A blaze around the moment's dot.
        Level::Every => (10.0, 64.0, 104.0, 3, 45.0),
    };
    let tallest = (usize::from(rows) * 4 * 9 / 10).max(8);
    let widest = (usize::from(cols) * 2).max(4);
    Shape {
        w: dots(wide, 4).min(widest),
        h: dots(tall, 8).min(tallest),
        source,
        floor,
        sparks: rate,
    }
    .held()
}
