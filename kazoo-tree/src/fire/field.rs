//! The fire itself: a heat field at braille-dot resolution and its sparks.
//!
//! The field is a fixed grid, at most [`MAX_W`] × [`MAX_H`] dots, allocated
//! once; a new shape (the view zoomed) resamples what is burning into the
//! same buffers, so a frame never allocates. Row 0 is the top: heat rises
//! toward it. Every speed is per second and every step takes the seconds
//! since the last, so the fire burns at the same pace at any frame rate.

use super::noise::{Rng, noise};

/// The widest the field ever is, in dots.
pub const MAX_W: usize = 72;
/// The tallest the field ever is, in dots.
pub const MAX_H: usize = 120;
/// The most sparks alive at once.
pub const MAX_SPARKS: usize = 160;
/// The longest a step is taken as, in seconds: a pause (the screen busy,
/// the fire out of view) resumes where it was, never with a lurch.
const LONGEST_STEP: f32 = 0.05;
/// The fire's clock wraps here (seconds), so its turbulence keeps its
/// precision however long the tree is open.
const CLOCK_WRAP: f32 = 4_096.0;

/// How big a fire is, and how it is fed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    /// Width, in dots.
    pub w: usize,
    /// Height, in dots.
    pub h: usize,
    /// Half the width of the burning source, in dots.
    pub source: f32,
    /// How many rows at the bottom the source feeds.
    pub floor: usize,
    /// Sparks born per second.
    pub sparks: f32,
}

impl Shape {
    /// The shape held inside what the field can burn: at least a small
    /// ember, at most the field's buffers; a source inside the width; a
    /// spark rate that is a number and not below zero.
    #[must_use]
    pub fn held(self) -> Self {
        let w = self.w.clamp(4, MAX_W);
        let h = self.h.clamp(8, MAX_H);
        let half = w as f32 / 2.0;
        let source = if self.source.is_finite() {
            self.source.clamp(0.5, half)
        } else {
            0.5
        };
        let sparks = if self.sparks.is_finite() {
            self.sparks.clamp(0.0, MAX_SPARKS as f32 * 4.0)
        } else {
            0.0
        };
        Self {
            w,
            h,
            source,
            floor: self.floor.clamp(1, h / 4),
            sparks,
        }
    }
}

/// A spark: where it is, how it moves, how long it has left, how bright.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Spark {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    life: f32,
    span: f32,
    glow: f32,
}

/// A fire burning in a fixed grid of dots.
pub struct Field {
    shape: Shape,
    /// Heat, `0.0..=1.0`, row by row, `shape.w` wide.
    heat: Vec<f32>,
    /// The next step's heat, then swapped in.
    next: Vec<f32>,
    /// What is drawn: the heat with the sparks over it.
    seen: Vec<f32>,
    sparks: Vec<Spark>,
    rng: Rng,
    /// The fire's own clock, seconds.
    t: f32,
    /// Sparks owed, carried between steps so a low rate still sends some.
    due: f32,
    /// Steps taken: the dither turns with it.
    frame: u32,
}

impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Field")
            .field("shape", &self.shape)
            .field("sparks", &self.sparks.len())
            .field("t", &self.t)
            .field("frame", &self.frame)
            .finish_non_exhaustive()
    }
}

impl Field {
    /// A field of `shape`, cold, its buffers allocated once and for all.
    #[must_use]
    pub fn new(shape: Shape) -> Self {
        Self {
            shape: shape.held(),
            heat: vec![0.0; MAX_W * MAX_H],
            next: vec![0.0; MAX_W * MAX_H],
            seen: vec![0.0; MAX_W * MAX_H],
            sparks: Vec::with_capacity(MAX_SPARKS),
            rng: Rng::new(0x9e37_79b9_7f4a_7c15),
            t: 0.0,
            due: 0.0,
            frame: 0,
        }
    }

    /// Its shape.
    #[must_use]
    pub const fn shape(&self) -> Shape {
        self.shape
    }

    /// Steps taken: what the dither turns by.
    #[must_use]
    pub const fn frame(&self) -> u32 {
        self.frame
    }

    /// How many sparks are alive.
    #[must_use]
    pub fn spark_count(&self) -> usize {
        self.sparks.len()
    }

    /// What is drawn at dot `(x, y)`: its heat with any spark over it,
    /// `0.0` outside the field.
    #[must_use]
    pub fn at(&self, x: i32, y: i32) -> f32 {
        let (w, h) = (self.shape.w, self.shape.h);
        match (usize::try_from(x), usize::try_from(y)) {
            (Ok(x), Ok(y)) if x < w && y < h => self.seen[y * w + x],
            _ => 0.0,
        }
    }

    /// Every dot drawn, row by row: for tests and measures.
    #[must_use]
    pub fn seen(&self) -> &[f32] {
        &self.seen[..self.shape.w * self.shape.h]
    }

    /// Every spark's place, `(x, y)`: for tests and measures.
    pub fn spark_places(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.sparks.iter().map(|s| (s.x, s.y))
    }

    /// Take on `shape`: what is burning is resampled to the new size in
    /// the same buffers, and the sparks moved with it.
    pub fn reshape(&mut self, shape: Shape) {
        let shape = shape.held();
        let (w0, h0) = (self.shape.w, self.shape.h);
        let (w1, h1) = (shape.w, shape.h);
        if (w0, h0) != (w1, h1) {
            let sx = (w0 - 1) as f32 / (w1 - 1) as f32;
            let sy = (h0 - 1) as f32 / (h1 - 1) as f32;
            for y in 0..h1 {
                for x in 0..w1 {
                    self.next[y * w1 + x] = self.sample(x as f32 * sx, y as f32 * sy);
                }
            }
            std::mem::swap(&mut self.heat, &mut self.next);
            // The sparks move as the heat was resampled: dot `x` of the
            // new field holds what was at `x * sx` of the old.
            let (kx, ky) = (1.0 / sx, 1.0 / sy);
            for s in &mut self.sparks {
                s.x *= kx;
                s.y *= ky;
                s.vx *= kx;
                s.vy *= ky;
            }
            // A spark at the old field's very edge can land just past the
            // new one's: it has left the fire.
            let (wf, hf) = (w1 as f32, h1 as f32);
            self.sparks
                .retain(|s| s.x >= 0.0 && s.x < wf && s.y >= 0.0 && s.y < hf);
        }
        self.shape = shape;
        self.compose();
    }

    /// Heat at a point between dots, from the four around it.
    fn sample(&self, x: f32, y: f32) -> f32 {
        let (w, h) = (self.shape.w, self.shape.h);
        let x = if x.is_finite() {
            x.clamp(0.0, (w - 1) as f32)
        } else {
            0.0
        };
        let y = if y.is_finite() {
            y.clamp(0.0, (h - 1) as f32)
        } else {
            0.0
        };
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: usize, y: usize| self.heat[y * w + x];
        let top = (at(x1, y0) - at(x0, y0)).mul_add(fx, at(x0, y0));
        let bottom = (at(x1, y1) - at(x0, y1)).mul_add(fx, at(x0, y1));
        (bottom - top).mul_add(fy, top)
    }

    /// Burn on by `dt` seconds. A `dt` that is not a number, or below
    /// zero, burns nothing; a long one is taken as [`LONGEST_STEP`].
    pub fn step(&mut self, dt: f32) {
        let dt = if dt.is_finite() {
            dt.clamp(0.0, LONGEST_STEP)
        } else {
            0.0
        };
        self.frame = self.frame.wrapping_add(1);
        self.t += dt;
        if self.t >= CLOCK_WRAP {
            self.t -= CLOCK_WRAP;
        }
        self.rise(dt);
        self.feed();
        self.spark(dt);
        self.compose();
    }

    /// Carry the heat up along the turbulence, cooling it as it climbs.
    fn rise(&mut self, dt: f32) {
        let Shape { w, h, .. } = self.shape;
        let (wf, hf) = (w as f32, h as f32);
        let t = self.t;
        // Dots climbed per second: a small flame is quick, a blaze stately.
        let rise = 0.3f32.mul_add(hf, 20.0) * dt;
        // Cooling is by height climbed, so how tall the flames reach is the
        // same at any speed: about two thirds of the field, the rest for
        // the sparks.
        let climbed = rise / hf;
        let centre = (wf - 1.0) / 2.0;
        for y in 0..h {
            let height = 1.0 - y as f32 / (hf - 1.0);
            let cool = 3.0f32.mul_add(height * height, 0.5) * climbed;
            let lean = 0.5 * wf * (0.4 + height) * dt;
            for x in 0..w {
                let n = noise(
                    x as f32 * 4.0 / wf,
                    (y as f32 * 3.0).mul_add(1.0 / hf, t * 1.6),
                    t * 0.7,
                );
                let off = (x as f32 - centre) / (wf / 2.0);
                // The edges cool faster: the flames gather into tongues
                // that meet over the source.
                let edge = 1.8f32.mul_add(off * off, 1.0);
                let v = self.sample(n.mul_add(lean, x as f32), y as f32 + rise);
                let lost = cool * 0.6f32.mul_add(n, 1.1) * edge;
                self.next[y * w + x] = (v - lost).clamp(0.0, 1.0);
            }
        }
        std::mem::swap(&mut self.heat, &mut self.next);
    }

    /// The burning point: hottest at its heart, flickering in patches.
    fn feed(&mut self) {
        let Shape {
            w,
            h,
            source,
            floor,
            ..
        } = self.shape;
        let centre = (w as f32 - 1.0) / 2.0;
        for x in 0..w {
            let off = (x as f32 - centre).abs() / source;
            if off >= 1.0 {
                continue;
            }
            let heart = (1.0 - off).sqrt();
            let flicker = noise(x as f32 * 1.5 / source, self.t * 3.0, 7.0).mul_add(0.5, 0.5);
            let base = 0.38f32.mul_add(flicker, 0.62) * 0.65f32.mul_add(heart, 0.35);
            for y in h - floor..h {
                self.heat[y * w + x] = base.min(1.0);
            }
        }
    }

    /// Sparks: born at the source, rising, wandering, fading.
    fn spark(&mut self, dt: f32) {
        let Shape {
            w,
            h,
            source,
            floor,
            sparks,
        } = self.shape;
        let (wf, hf) = (w as f32, h as f32);
        let centre = (wf - 1.0) / 2.0;
        self.due = sparks.mul_add(dt, self.due).min(MAX_SPARKS as f32);
        let born = self.due.floor();
        self.due -= born;
        for _ in 0..born as usize {
            if self.sparks.len() >= MAX_SPARKS {
                break;
            }
            let span = self.rng.unit().mul_add(1.1, 0.5);
            let spark = Spark {
                x: ((self.rng.unit() - 0.5) * 2.0).mul_add(source, centre),
                y: (h - floor) as f32 - 1.0,
                vx: (self.rng.unit() - 0.5) * 0.25 * wf,
                vy: -self.rng.unit().mul_add(0.45, 0.3) * hf,
                life: span,
                span,
                glow: 1.0,
            };
            self.sparks.push(spark);
        }
        let t = self.t;
        let rng = &mut self.rng;
        for s in &mut self.sparks {
            let wander = noise(s.x * 4.0 / wf, s.y * 3.0 / hf, t);
            s.vx = (wander * 1.2 * wf).mul_add(dt, s.vx) * 0.8f32.mul_add(-dt, 1.0);
            s.vy *= 0.25f32.mul_add(-dt, 1.0);
            s.x = s.vx.mul_add(dt, s.x);
            s.y = s.vy.mul_add(dt, s.y);
            s.life -= dt;
            // Crackling: each frame a little brighter or dimmer; fading
            // with age, and into the dark near the field's top.
            let crackle = rng.unit().mul_add(0.3, 0.7);
            let top = (s.y / (0.15 * hf)).clamp(0.0, 1.0);
            s.glow = 0.55f32.mul_add(s.life / s.span, 0.45) * crackle * top;
        }
        self.sparks
            .retain(|s| s.life > 0.0 && s.y >= 0.0 && s.x >= 0.0 && s.x < wf && s.x.is_finite());
    }

    /// What is drawn: the heat, each spark over it with a faint tail.
    fn compose(&mut self) {
        let Shape { w, h, .. } = self.shape;
        let n = w * h;
        self.seen[..n].copy_from_slice(&self.heat[..n]);
        for s in &self.sparks {
            if !(s.x.is_finite() && s.y.is_finite() && s.x >= 0.0 && s.y >= 0.0) {
                continue;
            }
            let (x, y) = (s.x as usize, s.y as usize);
            if x >= w || y >= h {
                continue;
            }
            let glow = s.glow.clamp(0.0, 1.0);
            let dot = &mut self.seen[y * w + x];
            *dot = dot.max(glow);
            if y + 1 < h {
                let tail = &mut self.seen[(y + 1) * w + x];
                *tail = tail.max(glow * 0.45);
            }
        }
    }
}
