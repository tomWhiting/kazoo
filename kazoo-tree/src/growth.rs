//! Where every moment sits: the tree grown as a tree.
//!
//! A limb grows in steps of [`GROWTH_STEP`] moments. Each step heads a
//! little to one side or the other, choosing among a few headings the one
//! with the most room, nudged by the limb's own seed so every limb wanders
//! its own way, and drawn a little toward the light.
//!
//! Steps are grown in the order their first moments were made, across every
//! limb. So a step's shape depends only on what had grown before it, and
//! growing the tree further never moves what is already there: the same
//! history always gives the same tree, and a place you remember stays put.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

use crate::history::{History, Sound};

/// Moments per step of growth. Fixed, never derived from the tree's size,
/// so a step's shape never changes as the tree grows.
pub const GROWTH_STEP: usize = 16;
/// How far one step grows, in world units (one braille dot at scale 1).
pub const STEP_LENGTH: f32 = 4.0;
/// The side of a cell in the room grid, world units.
const CELL: f32 = 6.0;
/// The headings a step looks at, as a share of [`TURN`] either way.
const TURNS: [f32; 5] = [-0.5, -0.25, 0.0, 0.25, 0.5];
/// The widest turn one step can make, radians.
const TURN: f32 = 0.6;
/// Each limb's own wander per heading, radians either way.
const WANDER: f32 = 0.12;
/// How strongly a step prefers to rise.
const LIGHT: f32 = 0.6;
/// How strongly a step avoids growth already there.
const CROWD: f32 = 3.0;
/// Where the ground is: nothing grows below it but roots.
const GROUND: f32 = 1.0;
/// How far apart separate roots stand, for limbs that share no history.
const ROOT_SPACING: f32 = 80.0;

/// One step of growth: up to [`GROWTH_STEP`] moments of one limb.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// The limb it is on.
    pub limb: usize,
    /// Its moments' places on the limb: `first..=last`.
    pub first: usize,
    /// The last moment's place on the limb.
    pub last: usize,
    /// Where it starts, world units (x right, y up).
    pub start: (f32, f32),
    /// Where it ends.
    pub end: (f32, f32),
    /// Its heading, radians from the x axis.
    pub heading: f32,
    /// How wide it is drawn: from the most modules alive in it.
    pub width: f32,
    /// How full it is: 1 for a whole step; a limb's tip may be part grown,
    /// and is drawn only as far as its last moment.
    pub fill: f32,
    /// The most modules alive in it.
    pub modules: u32,
    /// Its moments' sounds, averaged; `None` when none recorded one.
    pub sound: Option<Sound>,
}

/// The grown tree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Growth {
    /// Every step, in the order they grew.
    pub steps: Vec<Step>,
    /// For each limb, its steps in order.
    pub limb_steps: Vec<Vec<usize>>,
    /// Where each moment sits; `None` for one on no limb.
    pub at: Vec<Option<(f32, f32)>>,
    /// For each moment, the step it is in.
    pub step_of: Vec<Option<usize>>,
    /// Where each separate root stands.
    pub roots: Vec<(f32, f32)>,
    /// How far the roots reach sideways: they grow with the tree.
    pub root_reach: f32,
    /// The tree's extent: `(min_x, min_y, max_x, max_y)`.
    pub bounds: (f32, f32, f32, f32),
}

/// A small, fixed random source (`SplitMix64`), so a limb's wander is the
/// same on every machine and every run.
#[derive(Debug, Clone)]
struct Seeded(u64);

impl Seeded {
    fn of(name: &str) -> Self {
        // FNV-1a over the limb's name: stable, and different per limb.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in name.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        Self(hash)
    }

    const fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..hi`.
    fn between(&mut self, lo: f32, hi: f32) -> f32 {
        // 24 bits: every value exactly representable in an f32.
        let unit = (self.next() >> 40) as f32 / (1_u32 << 24) as f32;
        (hi - lo).mul_add(unit, lo)
    }
}

/// Which grid cells hold growth: each cell lists the steps that grew
/// through it, as `(limb, step on the limb)`.
#[derive(Debug, Default)]
struct Room(HashMap<(i32, i32), Vec<(usize, usize)>>);

/// How many of its own latest steps a limb does not count as crowding: the
/// trail just behind a tip is always near it, and counting it would push
/// every limb sideways off its own path.
const OWN_TRAIL: usize = 4;

impl Room {
    fn cell(x: f32, y: f32) -> (i32, i32) {
        ((x / CELL).floor() as i32, (y / CELL).floor() as i32)
    }

    /// Growth around `(x, y)`, not counting `limb`'s own trail behind `step`.
    fn crowd(&self, x: f32, y: f32, limb: usize, step: usize) -> usize {
        let (cx, cy) = Self::cell(x, y);
        let mut n = 0;
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(here) = self.0.get(&(cx + dx, cy + dy)) {
                    n += here
                        .iter()
                        .filter(|&&(l, s)| l != limb || step.saturating_sub(s) > OWN_TRAIL)
                        .count();
                }
            }
        }
        n
    }

    fn take(&mut self, x: f32, y: f32, limb: usize, step: usize) {
        self.0
            .entry(Self::cell(x, y))
            .or_default()
            .push((limb, step));
    }
}

/// How wide a step is drawn for the modules alive in it: a sparse wall
/// thin, a full one thick.
#[must_use]
pub fn width_for(modules: u32) -> f32 {
    (f32::from(u16::try_from(modules).unwrap_or(u16::MAX)).sqrt())
        .mul_add(0.55, 1.0)
        .min(8.0)
}

fn mean_sound(history: &History, moments: &[usize]) -> Option<Sound> {
    let sounds: Vec<Sound> = moments
        .iter()
        .filter_map(|&m| history.moments[m].sound)
        .collect();
    if sounds.is_empty() {
        return None;
    }
    let n = sounds.len() as f32;
    let mean = |pick: fn(&Sound) -> f32| sounds.iter().map(pick).sum::<f32>() / n;
    Some(Sound {
        rms_db: mean(|s| s.rms_db),
        peak_db: mean(|s| s.peak_db),
        centroid_hz: mean(|s| s.centroid_hz),
        low: mean(|s| s.low),
        mid: mean(|s| s.mid),
        high: mean(|s| s.high),
        onsets: mean(|s| s.onsets),
    })
}

struct Pending {
    time: i64,
    limb: usize,
    index: usize,
}

impl Growth {
    /// Grow the tree for `history`.
    #[must_use]
    pub fn grow(history: &History) -> Self {
        let moments = history.moments.len();
        let mut growth = Self {
            steps: Vec::new(),
            limb_steps: vec![Vec::new(); history.limbs.len()],
            at: vec![None; moments],
            step_of: vec![None; moments],
            roots: Vec::new(),
            root_reach: ROOT_REACH,
            bounds: (0.0, 0.0, 0.0, 0.0),
        };
        let mut order = Vec::new();
        for (l, limb) in history.limbs.iter().enumerate() {
            for (index, chunk) in limb.moments.chunks(GROWTH_STEP).enumerate() {
                order.push(Pending {
                    time: history.moments[chunk[0]].time,
                    limb: l,
                    index,
                });
            }
        }
        // Stable: equal times keep limb order (a parent limb before its
        // buds) and step order.
        order.sort_by(|a, b| {
            a.time
                .cmp(&b.time)
                .then(a.limb.cmp(&b.limb))
                .then(a.index.cmp(&b.index))
        });
        let mut room = Room::default();
        let mut seeds: Vec<Seeded> = history
            .limbs
            .iter()
            .map(|limb| Seeded::of(&limb.name))
            .collect();
        for pending in order {
            growth.grow_step(history, &mut room, &mut seeds[pending.limb], &pending);
        }
        let height = growth.steps.iter().map(|s| s.end.1).fold(0.0_f32, f32::max);
        growth.root_reach = (height * ROOT_SHARE).clamp(ROOT_REACH, ROOT_REACH * 20.0);
        growth.bounds = growth.measure();
        growth
    }

    fn grow_step(
        &mut self,
        history: &History,
        room: &mut Room,
        seed: &mut Seeded,
        pending: &Pending,
    ) {
        let limb = &history.limbs[pending.limb];
        let first = pending.index * GROWTH_STEP;
        let last = (first + GROWTH_STEP).min(limb.moments.len()) - 1;
        let own = &limb.moments[first..=last];
        let (start, mut heading) = if let Some(&previous) = self.limb_steps[pending.limb].last() {
            let step = &self.steps[previous];
            (step.end, step.heading)
        } else {
            self.sprout(history, pending.limb, seed)
        };
        let mut best: Option<(f32, f32, (f32, f32))> = None;
        for turn in TURNS {
            let angle = turn.mul_add(TURN, heading) + seed.between(-WANDER, WANDER);
            let end = (
                angle.cos().mul_add(STEP_LENGTH, start.0),
                angle.sin().mul_add(STEP_LENGTH, start.1),
            );
            let crowd = room.crowd(end.0, end.1, pending.limb, pending.index) as f32;
            let mut score = LIGHT.mul_add(angle.sin(), (-CROWD).mul_add(crowd, -turn.abs()));
            if end.1 < GROUND {
                score -= 50.0;
            }
            if best.is_none_or(|(b, _, _)| score > b) {
                best = Some((score, angle, end));
            }
        }
        let Some((_, angle, end)) = best else {
            return;
        };
        heading = angle;
        room.take(end.0, end.1, pending.limb, pending.index);
        room.take(
            f32::midpoint(start.0, end.0),
            f32::midpoint(start.1, end.1),
            pending.limb,
            pending.index,
        );
        let modules = own
            .iter()
            .map(|&m| history.moments[m].modules)
            .max()
            .unwrap_or(0);
        let id = self.steps.len();
        // A moment's place on its step is by its place in the step, out of
        // a full step, never out of how many the step holds yet: a step
        // still filling keeps every moment where it first sat, and so does
        // any bud grown from one of them.
        for (i, &m) in own.iter().enumerate() {
            let t = (i + 1) as f32 / GROWTH_STEP as f32;
            self.at[m] = Some((
                (end.0 - start.0).mul_add(t, start.0),
                (end.1 - start.1).mul_add(t, start.1),
            ));
            self.step_of[m] = Some(id);
        }
        self.steps.push(Step {
            limb: pending.limb,
            first,
            last,
            start,
            end,
            heading,
            width: width_for(modules),
            fill: own.len() as f32 / GROWTH_STEP as f32,
            modules,
            sound: mean_sound(history, own),
        });
        self.limb_steps[pending.limb].push(id);
    }

    /// Where a limb's first step starts and which way it heads: from the
    /// moment it grew from, turned off its parent's heading to one side; a
    /// limb with no parent stands on its own root.
    fn sprout(&mut self, history: &History, limb: usize, seed: &mut Seeded) -> ((f32, f32), f32) {
        let side: f32 = if seed.next() & 1 == 0 { -1.0 } else { 1.0 };
        let spread = seed.between(0.6, 1.1);
        let from = history.limbs[limb].from;
        if let Some(parent) = from.and_then(|m| self.step_of[m].map(|s| (m, s))) {
            let (moment, step) = parent;
            let heading = self.steps[step].heading;
            if let Some(at) = self.at[moment] {
                return (at, side.mul_add(spread, heading));
            }
        }
        let x = self.roots.len() as f32 * ROOT_SPACING;
        self.roots.push((x, 0.0));
        ((x, 0.0), FRAC_PI_2)
    }

    fn measure(&self) -> (f32, f32, f32, f32) {
        let mut bounds = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut grow_to = |(x, y): (f32, f32), pad: f32| {
            bounds.0 = bounds.0.min(x - pad);
            bounds.1 = bounds.1.min(y - pad);
            bounds.2 = bounds.2.max(x + pad);
            bounds.3 = bounds.3.max(y + pad);
        };
        for step in &self.steps {
            grow_to(step.start, step.width);
            grow_to(step.end, step.width);
        }
        for &(x, y) in &self.roots {
            grow_to((x - self.root_reach, y - self.root_depth()), 0.0);
            grow_to((x + self.root_reach, y), 0.0);
        }
        if bounds.0 > bounds.2 {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            bounds
        }
    }
}

/// How far roots reach sideways at the least, world units.
pub const ROOT_REACH: f32 = 18.0;
/// How far roots reach for the tree's height: a tenth.
const ROOT_SHARE: f32 = 0.1;
/// How deep roots go, for how far they reach.
pub const ROOT_DEPTH_SHARE: f32 = 0.45;

impl Growth {
    /// How deep the roots go.
    #[must_use]
    pub fn root_depth(&self) -> f32 {
        self.root_reach * ROOT_DEPTH_SHARE
    }
}

#[cfg(test)]
mod tests;
