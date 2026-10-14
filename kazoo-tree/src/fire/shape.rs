//! How big a fire is, and how it is fed.

/// The widest the field ever is, in dots.
pub const MAX_W: usize = 128;
/// The tallest the field ever is, in dots.
pub const MAX_H: usize = 200;
/// The fewest dots a field is wide.
const LEAST_W: usize = 8;
/// The fewest dots a field is tall.
const LEAST_H: usize = 12;
/// The finest the flames' turbulence is ever drawn, in octaves.
pub const MOST_DETAIL: u8 = 4;

/// How big a fire is, and how it is fed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    /// Width, in dots.
    pub w: usize,
    /// Height, in dots.
    pub h: usize,
    /// The row the burning moment sits on, from the top: the flames rise
    /// above it, and the branch burns below it too where it runs down.
    pub base: usize,
    /// How far along the branch either side of the moment it burns, dots.
    pub reach: f32,
    /// How far either side of the branch's line the fuel lies, dots.
    pub thick: f32,
    /// How high the flames climb above the branch before they are gone,
    /// dots: the wisps and sparks go higher.
    pub tall: f32,
    /// How fine the flames' turbulence is, in octaves: more as you come in.
    pub detail: u8,
    /// Sparks born per second, at full blaze.
    pub sparks: f32,
    /// Wisps of flame torn from the tongues' tips per second, at full
    /// blaze.
    pub wisps: f32,
}

/// `v` if it is a number, else `or`.
const fn number(v: f32, or: f32) -> f32 {
    if v.is_finite() { v } else { or }
}

impl Shape {
    /// The shape held inside what the field can burn: at least a small
    /// ember, at most the field's buffers; the moment's row inside the
    /// field with room under it; the branch's burning stretch inside the
    /// width; every rate a number and not below zero.
    #[must_use]
    pub fn held(self) -> Self {
        let w = self.w.clamp(LEAST_W, MAX_W);
        let h = self.h.clamp(LEAST_H, MAX_H);
        let base = self.base.clamp(h / 3, h - 3);
        let half = (w / 2) as f32;
        // The flames lean and the sparks wander: the branch burns well
        // inside the field, so nothing reaches its edges.
        let reach = number(self.reach, 1.0).clamp(1.0, (half * 0.55).max(1.0));
        let thick = number(self.thick, 1.0).clamp(0.75, reach.max(0.75));
        let tall = number(self.tall, 4.0).clamp(4.0, (base as f32 * 0.8).max(4.0));
        let rate = |v: f32| number(v, 0.0).clamp(0.0, 400.0);
        Self {
            w,
            h,
            base,
            reach,
            thick,
            tall,
            detail: self.detail.clamp(1, MOST_DETAIL),
            sparks: rate(self.sparks),
            wisps: rate(self.wisps),
        }
    }
}
