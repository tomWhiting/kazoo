//! What flies off the fire: sparks, and wisps of flame.
//!
//! A spark is a single bright dot that rises from the burning branch,
//! wanders, crackles and fades. A wisp is a little flame torn from a
//! tongue's tip: it lays its heat into the field as it climbs, so it burns
//! and breaks up like the flames it came from, and is gone in a moment.

use super::noise::{Rng, noise};

/// The most sparks and wisps alive at once.
pub const MAX_FLICKS: usize = 320;

/// What a flick is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// One bright dot.
    Spark,
    /// A little flame, laid into the field as heat.
    Wisp,
}

/// A spark or a wisp: where it is, how it moves, how long it has left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flick {
    /// What it is.
    pub kind: Kind,
    /// Where it is, dots from the field's top left.
    pub x: f32,
    /// Where it is, dots from the field's top left.
    pub y: f32,
    vx: f32,
    vy: f32,
    life: f32,
    span: f32,
    /// How bright it is now, `0.0..=1.0`.
    pub glow: f32,
    /// A wisp's size, dots.
    pub size: f32,
}

impl Flick {
    /// A spark at `(x, y)`, flying at `(vx, vy)` dots a second, for `span`
    /// seconds.
    #[must_use]
    pub const fn spark(x: f32, y: f32, vx: f32, vy: f32, span: f32) -> Self {
        Self {
            kind: Kind::Spark,
            x,
            y,
            vx,
            vy,
            life: span,
            span,
            glow: 1.0,
            size: 0.0,
        }
    }

    /// A wisp of `size` dots at `(x, y)`, flying at `(vx, vy)`, for `span`
    /// seconds, as hot as `glow`.
    #[must_use]
    pub const fn wisp(x: f32, y: f32, vx: f32, vy: f32, span: f32, glow: f32, size: f32) -> Self {
        Self {
            kind: Kind::Wisp,
            x,
            y,
            vx,
            vy,
            life: span,
            span,
            glow,
            size,
        }
    }

    /// How much of its life is left, `0.0..=1.0`.
    #[must_use]
    pub fn left(&self) -> f32 {
        if self.span > 0.0 {
            (self.life / self.span).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Fly on by `dt` seconds through a field `w` × `h` dots at clock `t`:
    /// a spark wanders on the air and slows, a wisp rides up with the
    /// flames, flickering out. Answers whether it is still alive and in
    /// the field.
    pub fn fly(&mut self, dt: f32, t: f32, (w, h): (f32, f32), rng: &mut Rng) -> bool {
        let wander = noise(self.x * 4.0 / w, self.y * 3.0 / h, t);
        match self.kind {
            Kind::Spark => {
                self.vx = (wander * 0.9 * w).mul_add(dt, self.vx) * 0.8f32.mul_add(-dt, 1.0);
                self.vy *= 0.25f32.mul_add(-dt, 1.0);
            }
            Kind::Wisp => {
                self.vx = (wander * 0.6 * w).mul_add(dt, self.vx) * 2.0f32.mul_add(-dt, 1.0);
            }
        }
        self.x = self.vx.mul_add(dt, self.x);
        self.y = self.vy.mul_add(dt, self.y);
        self.life -= dt;
        let left = self.left();
        // Fading into the dark near the field's top.
        let top = (self.y / (0.12 * h)).clamp(0.0, 1.0);
        self.glow = match self.kind {
            // Crackling: each frame a little brighter or dimmer.
            Kind::Spark => 0.55f32.mul_add(left, 0.45) * rng.unit().mul_add(0.3, 0.7) * top,
            Kind::Wisp => self.glow.min(left.sqrt()) * top,
        };
        self.life > 0.0
            && self.x.is_finite()
            && self.y.is_finite()
            && self.x >= 0.0
            && self.x < w
            && self.y >= 0.0
            && self.y < h
    }
}
