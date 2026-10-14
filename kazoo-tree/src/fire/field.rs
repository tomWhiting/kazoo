//! The fire itself: a heat field at braille-dot resolution, fed along the
//! burning branch, and what flies off it.
//!
//! The field is a fixed grid, at most [`MAX_W`] × [`MAX_H`] dots, allocated
//! once; a new shape (the view zoomed) resamples what is burning about the
//! moment into the same buffers, so a frame never allocates. Row 0 is the
//! top: heat rises toward it. Every speed is per second and every step
//! takes the seconds since the last, so the fire burns at the same pace at
//! any frame rate.
//!
//! The heat is fed along the branch in tongues whose roots wander, carried
//! up on drifting turbulence, and cooled by a texture that rises with it,
//! so the tongues pinch and tear into licks and wisps; outside the field
//! is cold, and the heat fades before any edge, so the fire has no box.

use super::flicks::{Flick, Kind, MAX_FLICKS};
use super::fuel::Fuel;
use super::noise::{Rng, fractal, noise, noise_wrapped, smooth};
pub use super::shape::{MAX_H, MAX_W, Shape};

/// The longest a step is taken as, in seconds: a pause (the screen busy,
/// the fire out of view) resumes where it was, never with a lurch.
const LONGEST_STEP: f32 = 0.05;
/// The fire's clock wraps here (seconds), so its turbulence keeps its
/// precision however long the tree is open.
const CLOCK_WRAP: f32 = 4_096.0;
/// The cooling texture's height, dots: it repeats, and scrolls up with
/// the flames forever.
const COOL_H: usize = 128;
/// The coolest heat counted as still burning.
const COLD: f32 = 0.02;
/// How hot the branch's embers glow, at full.
const EMBER: f32 = 0.42;
/// The tempo the fire burns at its own pace, BPM: faster above it, slower
/// below.
pub const TEMPO: f64 = 120.0;
/// A wisp's size at each detail, dots.
const WISP_SIZE: [f32; 4] = [1.0, 1.4, 1.9, 2.6];

/// A fire burning in a fixed grid of dots.
pub struct Field {
    shape: Shape,
    /// Heat, `0.0..=1.0`, row by row, `shape.w` wide.
    heat: Vec<f32>,
    /// The next step's heat, then swapped in.
    next: Vec<f32>,
    /// What is drawn: the heat with the sparks over it.
    seen: Vec<f32>,
    /// The branch's embers glowing under the flames: drawn, never risen.
    embers: Vec<f32>,
    /// Each dot's share of the fuel, `0.0..=1.0`.
    weight: Vec<f32>,
    /// Each dot's way along the branch from the moment, dots.
    way: Vec<f32>,
    /// What the fuel was laid from: none until it is.
    fuel: Option<Fuel>,
    /// How much of its heat each dot keeps, fading to nothing at the
    /// field's edges.
    edge: Vec<f32>,
    /// The cooling texture, `0.0..=1.0`, [`MAX_W`] wide, [`COOL_H`] tall.
    cool: Vec<f32>,
    flicks: Vec<Flick>,
    rng: Rng,
    /// The fire's own clock, seconds.
    t: f32,
    /// How far the cooling texture has risen, dots.
    scroll: f32,
    /// Sparks owed, carried between steps so a low rate still sends some.
    sparks_due: f32,
    /// Wisps owed.
    wisps_due: f32,
    /// Steps taken: the dither turns with it.
    frame: u32,
    /// How strongly the flames are fed, `0.0..=1.0`.
    gain: f32,
    /// How brightly the branch's embers glow, `0.0..=1.0`.
    ember: f32,
    /// How fast it burns, as a share of its pace at [`TEMPO`].
    pace: f32,
}

impl std::fmt::Debug for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Field")
            .field("shape", &self.shape)
            .field("flicks", &self.flicks.len())
            .field("t", &self.t)
            .field("frame", &self.frame)
            .field("gain", &self.gain)
            .field("ember", &self.ember)
            .finish_non_exhaustive()
    }
}

impl Field {
    /// A field of `shape`, cold but fed in full, its buffers allocated
    /// once and for all.
    #[must_use]
    pub fn new(shape: Shape) -> Self {
        let mut field = Self {
            shape: shape.held(),
            heat: vec![0.0; MAX_W * MAX_H],
            next: vec![0.0; MAX_W * MAX_H],
            seen: vec![0.0; MAX_W * MAX_H],
            embers: vec![0.0; MAX_W * MAX_H],
            weight: vec![0.0; MAX_W * MAX_H],
            way: vec![0.0; MAX_W * MAX_H],
            fuel: None,
            edge: vec![0.0; MAX_W * MAX_H],
            cool: vec![0.0; MAX_W * COOL_H],
            flicks: Vec::with_capacity(MAX_FLICKS),
            rng: Rng::new(0x9e37_79b9_7f4a_7c15),
            t: 0.0,
            scroll: 0.0,
            sparks_due: 0.0,
            wisps_due: 0.0,
            frame: 0,
            gain: 1.0,
            ember: 1.0,
            pace: 1.0,
        };
        field.prepare();
        field
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

    /// How many sparks and wisps are alive.
    #[must_use]
    pub fn spark_count(&self) -> usize {
        self.flicks.len()
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

    /// Every spark's and wisp's place, `(x, y)`: for tests and measures.
    pub fn spark_places(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.flicks.iter().map(|s| (s.x, s.y))
    }

    /// Whether nothing burns: no heat worth drawing, nothing flying.
    #[must_use]
    pub fn is_cold(&self) -> bool {
        self.flicks.is_empty() && self.seen().iter().all(|&v| v < COLD)
    }

    /// How strongly it is fed: `gain` the flames, `ember` the glow of the
    /// branch under them; each held to `0.0..=1.0`, and a value that is
    /// not a number feeds nothing.
    pub fn feed_by(&mut self, gain: f32, ember: f32) {
        let hold = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        self.gain = hold(gain);
        self.ember = hold(ember);
    }

    /// Burn in time with `bpm`: at [`TEMPO`] at its own pace, at half of
    /// it at half the pace. A tempo not known, not a number, or not above
    /// zero burns at its own pace.
    pub fn keep_time(&mut self, bpm: Option<f64>) {
        self.pace = match bpm {
            Some(bpm) if bpm.is_finite() && bpm > 0.0 => (bpm / TEMPO) as f32,
            _ => 1.0,
        };
        if !self.pace.is_finite() || self.pace <= 0.0 {
            self.pace = 1.0;
        }
    }

    /// Put it out at once: cold, nothing flying, fed nothing.
    pub fn clear(&mut self) {
        let n = self.shape.w * self.shape.h;
        self.heat[..n].fill(0.0);
        self.seen[..n].fill(0.0);
        self.embers[..n].fill(0.0);
        self.flicks.clear();
        self.sparks_due = 0.0;
        self.wisps_due = 0.0;
        self.gain = 0.0;
        self.ember = 0.0;
    }

    /// Burn along `fuel`: laid out over the field when it differs from
    /// what was laid last.
    pub fn burn_on(&mut self, fuel: &Fuel) {
        if self.fuel.as_ref() != Some(fuel) {
            fuel.lay(&self.shape, &mut self.weight, &mut self.way);
            self.fuel = Some(*fuel);
        }
    }

    /// The moment's dot in the field: its middle across, its base row.
    const fn moment(shape: &Shape) -> (f32, f32) {
        ((shape.w / 2) as f32, shape.base as f32)
    }

    /// Take on `shape`: what is burning is resampled about the moment, at
    /// the new size, in the same buffers, and what is flying moved with it.
    pub fn reshape(&mut self, shape: Shape) {
        let shape = shape.held();
        if shape == self.shape {
            return;
        }
        let old = self.shape;
        let (ox, oy) = Self::moment(&old);
        let (nx, ny) = Self::moment(&shape);
        // Old dots per new dot: the flames' height is the fire's scale.
        let k = old.tall / shape.tall;
        for y in 0..shape.h {
            for x in 0..shape.w {
                let (sx, sy) = (
                    (x as f32 - nx).mul_add(k, ox),
                    (y as f32 - ny).mul_add(k, oy),
                );
                self.next[y * shape.w + x] = self.sample(sx, sy);
            }
        }
        std::mem::swap(&mut self.heat, &mut self.next);
        for f in &mut self.flicks {
            f.x = (f.x - ox) / k + nx;
            f.y = (f.y - oy) / k + ny;
            f.size /= k;
        }
        let (wf, hf) = (shape.w as f32, shape.h as f32);
        self.flicks
            .retain(|f| f.x >= 0.0 && f.x < wf && f.y >= 0.0 && f.y < hf);
        self.shape = shape;
        self.fuel = None;
        self.prepare();
        self.compose();
    }

    /// The edges' fade and the cooling texture, for the shape.
    fn prepare(&mut self) {
        let Shape {
            w,
            h,
            reach,
            detail,
            ..
        } = self.shape;
        let (wf, hf) = (w as f32, h as f32);
        let side = (wf * 0.12).max(3.0);
        let top = (hf * 0.15).max(4.0);
        for y in 0..h {
            let yf = y as f32;
            let rows = smooth(0.0, top, yf) * smooth(0.0, 2.0, hf - 1.0 - yf);
            for x in 0..w {
                let xf = x as f32;
                self.edge[y * w + x] =
                    rows * smooth(0.0, side, xf) * smooth(0.0, side, wf - 1.0 - xf);
            }
        }
        // Blobs about the size of a tongue's width, taller than wide, the
        // finer octaves only as the view comes in.
        let cells = ((COOL_H as f32 / (reach * 0.9).max(2.0)).round() as i32).max(2);
        let across = 1.0 / (reach * 0.45).max(1.5);
        let down = cells as f32 / COOL_H as f32;
        for y in 0..COOL_H {
            for x in 0..w {
                let (mut sum, mut weight, mut total, mut fine) = (0.0, 1.0, 0.0, 1);
                for octave in 0..detail {
                    let f = fine as f32;
                    sum += noise_wrapped(
                        x as f32 * across * f,
                        y as f32 * down * f,
                        f32::from(octave) * 31.0,
                        cells * fine,
                    ) * weight;
                    total += weight;
                    weight *= 0.55;
                    fine *= 2;
                }
                self.cool[y * MAX_W + x] = smooth(-0.15, 0.55, sum / total);
            }
        }
    }

    /// Heat at a point between dots, from the four around it; outside the
    /// field is cold.
    fn sample(&self, x: f32, y: f32) -> f32 {
        let (w, h) = (
            i32::try_from(self.shape.w).unwrap_or(i32::MAX),
            i32::try_from(self.shape.h).unwrap_or(i32::MAX),
        );
        if !(x.is_finite() && y.is_finite()) || x <= -1.0 || y <= -1.0 {
            return 0.0;
        }
        let (fx, fy) = (x.floor(), y.floor());
        let (x0, y0) = (fx as i32, fy as i32);
        let (tx, ty) = (x - fx, y - fy);
        let at = |x: i32, y: i32| {
            if x < 0 || y < 0 || x >= w || y >= h {
                0.0
            } else {
                self.heat[(y * w + x) as usize]
            }
        };
        let top = (at(x0 + 1, y0) - at(x0, y0)).mul_add(tx, at(x0, y0));
        let bottom = (at(x0 + 1, y0 + 1) - at(x0, y0 + 1)).mul_add(tx, at(x0, y0 + 1));
        (bottom - top).mul_add(ty, top)
    }

    /// Dots the flames climb a second.
    fn rate(&self) -> f32 {
        0.35f32.mul_add(self.shape.tall, 18.0)
    }

    /// Burn on by `dt` seconds, at its pace. A `dt` that is not a number,
    /// or below zero, burns nothing; a long one is taken as
    /// [`LONGEST_STEP`].
    pub fn step(&mut self, dt: f32) {
        let dt = dt * self.pace;
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
        if self.fuel.is_none() {
            let fuel = Fuel::at(Self::moment(&self.shape));
            self.burn_on(&fuel);
        }
        self.rise(dt);
        self.feed();
        self.fly(dt);
        self.compose();
    }

    /// Carry the heat up along the turbulence, cooling it as it climbs by
    /// the rising texture, and fading it toward the edges.
    fn rise(&mut self, dt: f32) {
        let Shape {
            w,
            h,
            base,
            reach,
            tall,
            ..
        } = self.shape;
        let t = self.t;
        let up = self.rate() * dt;
        self.scroll = (self.scroll + up) % COOL_H as f32;
        // Cooling is by height climbed, so how tall the flames reach is
        // the same at any speed.
        let climbed = up / tall;
        let lean = reach.mul_add(1.0, 4.0) * dt;
        let (across, down) = (1.0 / reach.mul_add(1.2, 2.0), 1.0 / (tall * 0.5));
        let scroll = [self.scroll, self.scroll.mul_add(0.61, 53.0) % COOL_H as f32];
        let middle = (w / 2) as f32;
        // The flames gather toward the moment as they climb: far from it
        // they cool faster, so the fire tapers into tongues over its heart.
        let narrow = 1.0 / (reach * 1.3);
        let wf = w as f32;
        for y in 0..h {
            let height = ((base as f32 - y as f32) / tall).max(0.0);
            let cool = climbed * 2.6f32.mul_add(height * height, 0.25);
            let rows = scroll.map(|s| (y + s as usize) % COOL_H);
            // The texture sways from side to side as it rises, so the
            // tongues it carves curl rather than stand in columns.
            let sway = noise(y as f32 * down * 2.0, t * 0.5, 11.0) * reach * 0.6;
            for x in 0..w {
                let n = noise(
                    x as f32 * across,
                    (y as f32).mul_add(down, t * 1.4),
                    t * 0.6,
                );
                let v = self.sample(n.mul_add(lean * (0.3 + height), x as f32), y as f32 + up);
                let tx = ((x as f32 + sway).rem_euclid(wf)) as usize;
                let tex = (self.cool[rows[0] * MAX_W + tx] + self.cool[rows[1] * MAX_W + x]) * 0.5;
                let off = (x as f32 - middle) * narrow;
                let taper = (1.4 * height).mul_add(off * off, 1.0);
                let lost = cool * 1.7f32.mul_add(tex, 0.2) * taper;
                let i = y * w + x;
                self.next[i] = ((v - lost) * self.edge[i]).clamp(0.0, 1.0);
            }
        }
        std::mem::swap(&mut self.heat, &mut self.next);
    }

    /// The branch burning: tongues whose roots wander along it, over the
    /// glow of its embers, spreading from the moment as it catches.
    fn feed(&mut self) {
        let Shape {
            w, h, reach, thick, ..
        } = self.shape;
        let t = self.t;
        let spread = reach * 0.88f32.mul_add(self.gain, 0.12);
        let tongue = 1.0 / (reach * 0.35).max(1.5);
        for i in 0..w * h {
            let weight = self.weight[i];
            if weight <= 0.0 {
                self.embers[i] = 0.0;
                continue;
            }
            let s = self.way[i];
            let flick = noise(s * 0.5, t * 7.0, 3.0).mul_add(0.5, 0.5);
            let flame = if s.abs() <= spread {
                let root = smooth(0.05, 0.35, fractal(s * tongue, t * 0.9, 5.0, 2));
                let core = 1.0 - smooth(0.0, thick * 1.5, s.abs());
                weight
                    * self.gain
                    * 0.75f32.mul_add(root, 0.25).max(core)
                    * 0.2f32.mul_add(flick, 0.8)
            } else {
                0.0
            };
            self.embers[i] = weight * self.ember * EMBER * 0.15f32.mul_add(flick, 0.85);
            self.heat[i] = self.heat[i].max(flame).min(1.0);
        }
    }

    /// Send sparks and wisps off, fly them on, and lay each wisp's heat
    /// into the field.
    fn fly(&mut self, dt: f32) {
        let Shape {
            w,
            h,
            reach,
            thick,
            detail,
            sparks,
            wisps,
            ..
        } = self.shape;
        let (wf, hf) = (w as f32, h as f32);
        let rate = self.rate();
        let spread = reach * 0.88f32.mul_add(self.gain, 0.12);
        let fuel = self
            .fuel
            .unwrap_or_else(|| Fuel::at(Self::moment(&self.shape)));
        self.sparks_due = (sparks * self.gain)
            .mul_add(dt, self.sparks_due)
            .min(MAX_FLICKS as f32);
        self.wisps_due = (wisps * self.gain)
            .mul_add(dt, self.wisps_due)
            .min(MAX_FLICKS as f32);
        while self.sparks_due >= 1.0 && self.flicks.len() < MAX_FLICKS {
            self.sparks_due -= 1.0;
            let (x, y) = fuel.point_at(self.rng.unit().mul_add(2.0, -1.0) * spread);
            let span = self.rng.unit().mul_add(1.3, 0.6);
            self.flicks.push(Flick::spark(
                ((self.rng.unit() - 0.5) * 2.0).mul_add(thick, x),
                thick.mul_add(-0.5, y),
                (self.rng.unit() - 0.5) * reach * 0.9,
                -self.rng.unit().mul_add(0.8, 0.5) * rate * 1.2,
                span,
            ));
        }
        while self.wisps_due >= 1.0 && self.flicks.len() < MAX_FLICKS {
            self.wisps_due -= 1.0;
            let (x, y) = fuel.point_at(self.rng.unit().mul_add(2.0, -1.0) * spread * 0.85);
            if let Some(tip) = self.tip(x, y) {
                let size =
                    WISP_SIZE[usize::from(detail - 1).min(3)] * self.rng.unit().mul_add(0.6, 0.7);
                self.flicks.push(Flick::wisp(
                    (self.rng.unit() - 0.5).mul_add(2.0, x),
                    tip,
                    (self.rng.unit() - 0.5) * reach * 1.5,
                    -self.rng.unit().mul_add(0.6, 0.9) * rate,
                    self.rng.unit().mul_add(0.45, 0.25),
                    self.rng.unit().mul_add(0.25, 0.75),
                    size,
                ));
            }
        }
        if self.sparks_due >= 1.0 {
            self.sparks_due = 0.0;
        }
        if self.wisps_due >= 1.0 {
            self.wisps_due = 0.0;
        }
        let (t, rng) = (self.t, &mut self.rng);
        self.flicks.retain_mut(|f| f.fly(dt, t, (wf, hf), rng));
        for f in &self.flicks {
            if f.kind == Kind::Wisp {
                lay_wisp(&mut self.heat, (w, h), f);
            }
        }
    }

    /// Where the tongue over `(x, y)` ends: the top of the hot heat in its
    /// column, if there is a tongue there at all.
    fn tip(&self, x: f32, y: f32) -> Option<f32> {
        let (w, h) = (self.shape.w, self.shape.h);
        if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 {
            return None;
        }
        let (col, mut row) = (x as usize, (y as usize).min(h - 1));
        if col >= w {
            return None;
        }
        let start = row;
        while row > 0 && self.heat[(row - 1) * w + col] > 0.45 {
            row -= 1;
        }
        (start - row >= 2).then_some(row as f32)
    }

    /// What is drawn: the heat, each spark over it with a faint tail.
    fn compose(&mut self) {
        let Shape { w, h, .. } = self.shape;
        let n = w * h;
        // What flies, and the wisps laid after the heat rose, fade at the
        // edges as the flames do: nothing is ever cut off by the field.
        let layers = self.heat[..n]
            .iter()
            .zip(&self.embers[..n])
            .zip(&self.edge[..n]);
        for (seen, ((&heat, &ember), &edge)) in self.seen[..n].iter_mut().zip(layers) {
            *seen = heat.max(ember) * edge;
        }
        for s in &self.flicks {
            if s.kind != Kind::Spark || !(s.x >= 0.0 && s.y >= 0.0) {
                continue;
            }
            let (x, y) = (s.x as usize, s.y as usize);
            if x >= w || y >= h {
                continue;
            }
            let glow = s.glow.clamp(0.0, 1.0) * self.edge[y * w + x];
            let dot = &mut self.seen[y * w + x];
            *dot = dot.max(glow);
            if y + 1 < h {
                let tail = &mut self.seen[(y + 1) * w + x];
                *tail = tail.max(glow * 0.45);
            }
        }
    }
}

/// Lay a wisp's heat into `heat` (`w` × `h`): a soft blob taller than wide.
fn lay_wisp(heat: &mut [f32], (w, h): (usize, usize), f: &Flick) {
    let (rx, ry) = (f.size.max(0.5), f.size.max(0.5) * 1.7);
    let (x0, x1) = ((f.x - rx).floor().max(0.0), (f.x + rx).ceil());
    let (y0, y1) = ((f.y - ry).floor().max(0.0), (f.y + ry).ceil());
    if !(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite()) {
        return;
    }
    for y in y0 as usize..=(y1 as usize).min(h - 1) {
        for x in x0 as usize..=(x1 as usize).min(w - 1) {
            let (dx, dy) = ((x as f32 - f.x) / rx, (y as f32 - f.y) / ry);
            let k = smooth(0.0, 1.0, 1.0 - dx.mul_add(dx, dy * dy));
            if k > 0.0 {
                let dot = &mut heat[y * w + x];
                *dot = dot.max(f.glow * k * 0.95).min(1.0);
            }
        }
    }
}
