//! Where a fire burns on screen, how big, and along what, at each level of
//! zoom.
//!
//! A fire stands on its moment's place: the field's moment dot is the cell
//! the moment's glyph is drawn in, and the branch through it, its
//! neighbours either side, is what burns. Its size follows the zoom: far
//! out a small bright ember that never covers the tree, near and close
//! flames along the limb as thick as it is drawn, right in a full blaze.

use super::fuel::{Fuel, PATH};
use super::shape::Shape;
use crate::render::{self, Level, Scene, View};

/// Where a fire is on the picture, in dots from its top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The middle of the cell the moment's glyph is drawn in.
    pub anchor: (i32, i32),
    /// The field's top left dot.
    pub origin: (i32, i32),
    /// The field's size and how it is fed.
    pub shape: Shape,
    /// The branch through the moment, in the field's dots.
    pub fuel: Fuel,
}

/// The farthest a placed point is taken as, dots: past it a point is not
/// on any picture.
const FARTHEST: f64 = 1.0e6;

impl Placement {
    /// Where the fire for `moment` burns, for `scene` seen through `view`
    /// in `cols` × `rows`: `None` when the moment is not on the tree, or its
    /// place is not grown.
    #[must_use]
    pub fn of(scene: &Scene<'_>, view: &View, cols: u16, rows: u16, moment: usize) -> Option<Self> {
        let at = scene.growth.at.get(moment).copied().flatten()?;
        let (across, down) = view.to_dots(at, cols, rows);
        if !(across.is_finite() && down.is_finite())
            || across.abs() > FARTHEST
            || down.abs() > FARTHEST
        {
            return None;
        }
        // The cell the glyph is drawn in, and its middle dot.
        let (col, row) = ((across / 2.0).floor(), (down / 4.0).floor());
        let anchor = (col as i32 * 2 + 1, row as i32 * 4 + 2);
        let limb = scene.history.place.get(moment).copied().flatten();
        let trunk = limb
            .and_then(|(limb, _)| scene.history.limbs.get(limb))
            .is_some_and(|limb| limb.from.is_none());
        let wide = scene
            .growth
            .look
            .get(moment)
            .copied()
            .flatten()
            .map_or(1.0, |look| render::limb_dots(look.width, trunk, view.scale))
            as f32;
        let shape = shape_of(Level::of(view.scale), wide, cols, rows);
        let (width, base) = (to_i32(shape.w), to_i32(shape.base));
        let origin = (anchor.0 - width / 2, anchor.1 - base);
        // The branch's points, in the field's dots: the moment's own is
        // its middle, the rest where they are drawn about it.
        let local = |m: usize| {
            let point = scene.growth.at.get(m).copied().flatten()?;
            let (px, py) = view.to_dots(point, cols, rows);
            let (lx, ly) = (px - f64::from(origin.0), py - f64::from(origin.1));
            (lx.abs() < FARTHEST && ly.abs() < FARTHEST).then_some((lx as f32, ly as f32))
        };
        let side = (PATH - 1) / 2;
        let mut before = [(f32::NAN, f32::NAN); PATH];
        let mut at_m = scene.history.moments.get(moment).and_then(|m| m.parent);
        for slot in before.iter_mut().take(side) {
            let Some(m) = at_m else { break };
            let Some(p) = local(m) else { break };
            *slot = p;
            at_m = scene.history.moments.get(m).and_then(|m| m.parent);
        }
        let mut after = [(f32::NAN, f32::NAN); PATH];
        if let Some((limb, i)) = limb
            && let Some(limb) = scene.history.limbs.get(limb)
        {
            for (slot, &m) in after
                .iter_mut()
                .zip(limb.moments.iter().skip(i + 1).take(side))
            {
                let Some(p) = local(m) else { break };
                *slot = p;
            }
        }
        let middle = ((shape.w / 2) as f32, shape.base as f32);
        // The moment's glyph sits in its cell's middle, a little off its
        // exact place: the branch is moved with it.
        let off = local(moment).map_or((0.0, 0.0), |p| (middle.0 - p.0, middle.1 - p.1));
        for p in before.iter_mut().chain(after.iter_mut()) {
            *p = (p.0 + off.0, p.1 + off.1);
        }
        Some(Self {
            anchor,
            origin,
            shape,
            fuel: Fuel::line(&before, middle, &after)?,
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
/// picture of `cols` × `rows`: never wider than the picture, nor taller
/// than most of it.
#[must_use]
pub fn shape_of(level: Level, limb: f32, cols: u16, rows: u16) -> Shape {
    let limb = if limb.is_finite() { limb.max(1.0) } else { 1.0 };
    // How far along the branch it burns, how thick, how high the flames
    // climb, how fine their detail, and sparks and wisps a second.
    let (reach, thick, tall, detail, sparks, wisps): (f32, f32, f32, u8, f32, f32) = match level {
        // An ember: a few dots of fire and a spark now and then.
        Level::Far => (2.5, 1.5, 14.0, 1, 3.0, 0.0),
        Level::Near => {
            let reach = limb.mul_add(1.0, 4.0).clamp(5.0, 10.0);
            let thick = limb.mul_add(0.5, 1.2).clamp(1.5, 4.0);
            (reach, thick, reach * 4.0, 2, reach * 1.2, reach * 0.5)
        }
        Level::Close => {
            let reach = limb.mul_add(1.2, 7.0).clamp(8.0, 16.0);
            let thick = limb.mul_add(0.6, 1.5).clamp(2.0, 6.0);
            (reach, thick, reach * 4.0, 3, reach * 1.6, reach)
        }
        // A blaze along the branch through the moment's dot.
        Level::Every => {
            let thick = limb.mul_add(0.6, 2.0).clamp(3.0, 7.0);
            (26.0, thick, 112.0, 4, 70.0, 34.0)
        }
    };
    // Room either side for the flames to lean and the sparks to wander,
    // and under the moment for the branch where it runs down.
    let side = (tall * 0.3).max(6.0);
    let below = reach + thick + 4.0;
    let tallest = (usize::from(rows) * 4 * 9 / 10).max(12);
    let widest = (usize::from(cols) * 2).max(8);
    let w = dots(2.0 * (reach + side), 8).min(widest);
    let h = dots(tall.mul_add(1.45, below), 12).min(tallest);
    // A field cut short by the picture keeps its room under the moment
    // first: the flames climb off the top of the screen.
    let base = h.saturating_sub(dots(below, 4).min(h / 3));
    Shape {
        w,
        h,
        base,
        reach,
        thick,
        tall,
        detail,
        sparks,
        wisps,
    }
    .held()
}
