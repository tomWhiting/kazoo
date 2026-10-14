//! What burns: the branch's line through the moment, in the field's dots.
//!
//! The fire is fed along the branch, not from a floor: the line through
//! the burning moment and its neighbours on either side, so the flames
//! wrap the branch wherever it runs, up, down or across.

use super::noise::smooth;
use super::shape::Shape;

/// The most points on a branch's line: the moment and its neighbours.
pub const PATH: usize = 17;

/// The branch's line through the burning moment, in the field's dots.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fuel {
    pts: [(f32, f32); PATH],
    len: usize,
    /// Which point is the moment.
    anchor: usize,
}

impl Fuel {
    /// A moment alone: no branch either side of it drawn.
    #[must_use]
    pub const fn at(point: (f32, f32)) -> Self {
        let mut pts = [(0.0, 0.0); PATH];
        pts[0] = point;
        Self {
            pts,
            len: 1,
            anchor: 0,
        }
    }

    /// The line through `anchor`, with the points `before` it and `after`
    /// it each nearest first; points that are not numbers are left off,
    /// and with them every one past them, and at most [`PATH`] are kept,
    /// shared evenly either side.
    #[must_use]
    pub fn line(before: &[(f32, f32)], anchor: (f32, f32), after: &[(f32, f32)]) -> Self {
        if !(anchor.0.is_finite() && anchor.1.is_finite()) {
            return Self::at((0.0, 0.0));
        }
        let sound = |p: &&(f32, f32)| p.0.is_finite() && p.1.is_finite();
        let side = (PATH - 1) / 2;
        let back = before.iter().take_while(sound).take(side).count();
        let mut fuel = Self::at(anchor);
        fuel.len = 0;
        for &p in before[..back].iter().rev() {
            fuel.pts[fuel.len] = p;
            fuel.len += 1;
        }
        fuel.anchor = fuel.len;
        fuel.pts[fuel.len] = anchor;
        fuel.len += 1;
        for &p in after.iter().take_while(sound).take(side) {
            fuel.pts[fuel.len] = p;
            fuel.len += 1;
        }
        fuel
    }

    /// The line's points, in order along the branch.
    #[must_use]
    pub fn points(&self) -> &[(f32, f32)] {
        &self.pts[..self.len]
    }

    /// The moment's point.
    #[must_use]
    pub const fn moment(&self) -> (f32, f32) {
        self.pts[self.anchor]
    }

    /// Each point's way along the line from the moment, dots: below zero
    /// before it, above zero after.
    fn along(&self) -> [f32; PATH] {
        let mut s = [0.0; PATH];
        for i in self.anchor + 1..self.len {
            s[i] = s[i - 1] + dist(self.pts[i - 1], self.pts[i]);
        }
        for i in (0..self.anchor).rev() {
            s[i] = s[i + 1] - dist(self.pts[i], self.pts[i + 1]);
        }
        s
    }

    /// The point `s` dots along the line from the moment, held to the
    /// line's ends.
    #[must_use]
    pub fn point_at(&self, s: f32) -> (f32, f32) {
        let along = self.along();
        let s = if s.is_finite() { s } else { 0.0 };
        for i in 1..self.len {
            let (lo, hi) = (along[i - 1], along[i]);
            if s <= hi || i == self.len - 1 {
                let frac = if hi > lo {
                    ((s - lo) / (hi - lo)).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (from, to) = (self.pts[i - 1], self.pts[i]);
                return (
                    (to.0 - from.0).mul_add(frac, from.0),
                    (to.1 - from.1).mul_add(frac, from.1),
                );
            }
        }
        self.moment()
    }

    /// How far the dot at `p` is from the line, and how far along the line
    /// from the moment its nearest point is.
    #[must_use]
    pub fn nearest(&self, dot: (f32, f32)) -> (f32, f32) {
        if self.len == 1 {
            return (dist(dot, self.pts[0]), 0.0);
        }
        let along = self.along();
        let mut best = (f32::INFINITY, 0.0);
        for i in 1..self.len {
            let (start, end) = (self.pts[i - 1], self.pts[i]);
            let (dx, dy) = (end.0 - start.0, end.1 - start.1);
            let long = dx.mul_add(dx, dy * dy);
            let frac = if long > 0.0 {
                ((dot.0 - start.0).mul_add(dx, (dot.1 - start.1) * dy) / long).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let foot = (dx.mul_add(frac, start.0), dy.mul_add(frac, start.1));
            let gap = dist(dot, foot);
            if gap < best.0 {
                best = (gap, (along[i] - along[i - 1]).mul_add(frac, along[i - 1]));
            }
        }
        best
    }

    /// Lay the fuel out over a field of `shape`: each dot's share of the
    /// fuel, `0.0..=1.0`, into `weight`, and its way along the branch into
    /// `way`; both row by row, `shape.w` wide.
    pub fn lay(&self, shape: &Shape, weight: &mut [f32], way: &mut [f32]) {
        let n = shape.w * shape.h;
        if weight.len() < n || way.len() < n {
            return;
        }
        for y in 0..shape.h {
            for x in 0..shape.w {
                let i = y * shape.w + x;
                let (d, s) = self.nearest((x as f32, y as f32));
                // Thickest at the moment, thinning along the branch, and
                // hottest on its line.
                let reach = 1.0 - smooth(shape.reach * 0.6, shape.reach, s.abs());
                let across = 1.0 - smooth(shape.thick * 0.6, shape.thick * 1.6, d);
                weight[i] = (reach * across).clamp(0.0, 1.0);
                way[i] = s;
            }
        }
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}
