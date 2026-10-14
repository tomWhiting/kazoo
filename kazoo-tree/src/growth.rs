//! Where every moment sits: the tree grown as a tree.
//!
//! A limb grows in steps of [`GROWTH_STEP`] moments. Each step heads a
//! little to one side or the other, choosing among a few headings the one
//! with the most room, nudged by the limb's own seed so every limb wanders
//! its own way, and drawn a little toward the light.
//!
//! Steps are grown in the order they began: each step's first moment's
//! time, held to be no earlier than the step before it on its limb, nor
//! than the step its limb grew from (a commit's time can step backwards:
//! the wall dates a gap when it writes it, after changes made earlier). So
//! a step's shape depends only on what grew before it, and growing the tree
//! further never moves what is already there: the same history always
//! gives the same tree, and a place you remember stays put.
//!
//! The arithmetic is the same on every machine: the angles go through
//! `libm`, never the platform's own `sin` and `cos`, whose last bits differ
//! between systems; everything else is IEEE arithmetic. And a golden test
//! pins the shape of a fixed history, so any change to how the tree grows
//! is made on purpose, with [`GROWTH_VERSION`] moved with it.

use std::collections::HashMap;
use std::f64::consts::FRAC_PI_2;

use crate::history::{History, Sound};

/// The version of how the tree grows.
///
/// Every constant here shapes every tree drawn; changing one moves every place a person remembers, so it is
/// done only on purpose: this moves with it, and so does the golden test.
pub const GROWTH_VERSION: u32 = 2;
/// Moments per step of growth. Fixed, never derived from the tree's size,
/// so a step's shape never changes as the tree grows.
pub const GROWTH_STEP: usize = 16;
/// How far one step grows, in world units (one braille dot at scale 1).
pub const STEP_LENGTH: f64 = 4.0;
/// The side of a cell in the room grid, world units.
pub const CELL: f64 = 6.0;
/// The headings a step looks at, as a share of [`TURN`] either way.
const TURNS: [f64; 5] = [-0.5, -0.25, 0.0, 0.25, 0.5];
/// The widest turn one step can make, radians.
const TURN: f64 = 0.6;
/// Each limb's own wander per heading, radians either way.
const WANDER: f64 = 0.12;
/// How strongly a step prefers to rise.
const LIGHT: f64 = 0.6;
/// How strongly a step avoids growth already there.
const CROWD: f64 = 3.0;
/// Where the ground is: nothing grows below it but roots.
const GROUND: f64 = 1.0;
/// How far a tree's roots spread either way, world units. Fixed: roots
/// stand for where a tree began, and never grow, so they are never redrawn.
pub const ROOT_REACH: f64 = 24.0;
/// How deep a tree's roots go, world units.
pub const ROOT_DEPTH: f64 = 11.0;
/// The room left between a tree that shares no history with the others
/// and everything grown before it, world units.
const TREE_GAP: f64 = 40.0;

/// One step of growth: up to [`GROWTH_STEP`] moments of one limb.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// The limb it is on.
    pub limb: usize,
    /// Its first moment's place on the limb.
    pub first: usize,
    /// Its last moment's place on the limb.
    pub last: usize,
    /// Where it starts, world units (x right, y up).
    pub start: (f64, f64),
    /// Where a whole step ends; a step still filling is drawn only as far
    /// as its last moment.
    pub end: (f64, f64),
    /// Its heading, radians from the x axis.
    pub heading: f64,
    /// The widest any of its moments is drawn.
    pub widest: f64,
}

/// How one moment's stretch of limb is drawn: from the moment before it
/// (or its step's start) to it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// Width, from the modules alive at the moment.
    pub width: f64,
    /// The mean sound of its step's moments up to and including it; `None`
    /// while none of them kept one. Only ever the past: a stretch drawn
    /// once is drawn the same way however the step fills after it.
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
    pub at: Vec<Option<(f64, f64)>>,
    /// For each moment, the step it is in.
    pub step_of: Vec<Option<usize>>,
    /// How each moment's stretch is drawn.
    pub look: Vec<Option<Look>>,
    /// Where each separate tree stands.
    pub roots: Vec<(f64, f64)>,
    /// The tree's extent: `(min_x, min_y, max_x, max_y)`.
    pub bounds: (f64, f64, f64, f64),
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
    fn between(&mut self, lo: f64, hi: f64) -> f64 {
        // 53 bits: every value exactly representable in an f64.
        let unit = (self.next() >> 11) as f64 / (1_u64 << 53) as f64;
        (hi - lo).mul_add(unit, lo)
    }
}

/// Which grid cells hold growth: each cell lists the steps that grew
/// through it, as `(limb, step on the limb)`.
#[derive(Debug, Default)]
struct Room(HashMap<(i64, i64), Vec<(usize, usize)>>);

/// How many of its own latest steps a limb does not count as crowding: the
/// trail just behind a tip is always near it, and counting it would push
/// every limb sideways off its own path.
const OWN_TRAIL: usize = 4;

impl Room {
    fn cell(x: f64, y: f64) -> (i64, i64) {
        ((x / CELL).floor() as i64, (y / CELL).floor() as i64)
    }

    /// Growth around `(x, y)`, not counting `limb`'s own trail behind `step`.
    fn crowd(&self, x: f64, y: f64, limb: usize, step: usize) -> usize {
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

    fn take(&mut self, x: f64, y: f64, limb: usize, step: usize) {
        self.0
            .entry(Self::cell(x, y))
            .or_default()
            .push((limb, step));
    }
}

/// How wide a moment is drawn for the modules alive at it: a sparse wall
/// thin, a full one thick.
#[must_use]
pub fn width_for(modules: u32) -> f64 {
    f64::from(modules).sqrt().mul_add(0.55, 1.0).min(8.0)
}

/// The mean of `sounds`.
fn mean_of(sounds: &[Sound]) -> Option<Sound> {
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

/// A step waiting to grow, with the key it grows in order of.
struct Pending {
    key: i64,
    rank: usize,
    limb: usize,
    index: usize,
}

/// The limbs in an order where each comes after the limb it grew from;
/// answers each limb's rank in it. A limb whose `from` is on no limb
/// stands on its own.
fn limb_ranks(history: &History) -> Vec<usize> {
    let n = history.limbs.len();
    let parent: Vec<Option<usize>> = history
        .limbs
        .iter()
        .map(|limb| limb.from.and_then(|m| history.place_of(m)).map(|(l, _)| l))
        .collect();
    let mut rank = vec![usize::MAX; n];
    let mut next = 0;
    for start in 0..n {
        // Climb to the oldest ancestor not yet ranked, then rank down.
        let mut chain = Vec::new();
        let mut at = Some(start);
        while let Some(l) = at {
            if rank[l] != usize::MAX || chain.contains(&l) {
                break;
            }
            chain.push(l);
            at = parent[l];
        }
        for &l in chain.iter().rev() {
            rank[l] = next;
            next += 1;
        }
    }
    rank
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
            look: vec![None; moments],
            roots: Vec::new(),
            bounds: (0.0, 0.0, 0.0, 0.0),
        };
        let order = Self::order(history);
        let mut room = Room::default();
        let mut seeds: Vec<Seeded> = history
            .limbs
            .iter()
            .map(|limb| Seeded::of(&limb.name))
            .collect();
        let mut right = f64::MIN;
        for pending in order {
            growth.grow_step(
                history,
                &mut room,
                &mut seeds[pending.limb],
                &pending,
                right,
            );
            if let Some(step) = growth.steps.last() {
                right = right.max(step.start.0.max(step.end.0) + step.widest);
            }
            if let Some(&(x, _)) = growth.roots.last() {
                right = right.max(x + ROOT_REACH);
            }
        }
        growth.bounds = growth.measure();
        growth
    }

    /// Every step, in the order it grows: by its causal key (its first
    /// moment's time, held no earlier than the step before it on its limb,
    /// and for a limb's first step, the step holding the moment it grew
    /// from), then parents before their buds, then step order.
    fn order(history: &History) -> Vec<Pending> {
        let rank = limb_ranks(history);
        let mut by_rank: Vec<usize> = (0..history.limbs.len()).collect();
        by_rank.sort_by_key(|&l| rank[l]);
        // Each limb's steps' keys, filled parents first.
        let mut keys: Vec<Vec<i64>> = vec![Vec::new(); history.limbs.len()];
        for &l in &by_rank {
            let limb = &history.limbs[l];
            let mut before = limb
                .from
                .and_then(|m| history.place_of(m))
                .and_then(|(parent, at)| keys[parent].get(at / GROWTH_STEP).copied())
                .unwrap_or(i64::MIN);
            let mut own = Vec::with_capacity(limb.moments.len().div_ceil(GROWTH_STEP));
            for chunk in limb.moments.chunks(GROWTH_STEP) {
                let key = history.moments[chunk[0]].time.max(before);
                own.push(key);
                before = key;
            }
            keys[l] = own;
        }
        let mut order: Vec<Pending> = keys
            .iter()
            .enumerate()
            .flat_map(|(limb, own)| {
                let rank = rank[limb];
                own.iter().enumerate().map(move |(index, &key)| Pending {
                    key,
                    rank,
                    limb,
                    index,
                })
            })
            .collect();
        order.sort_by(|a, b| {
            a.key
                .cmp(&b.key)
                .then(a.rank.cmp(&b.rank))
                .then(a.index.cmp(&b.index))
        });
        order
    }

    fn grow_step(
        &mut self,
        history: &History,
        room: &mut Room,
        seed: &mut Seeded,
        pending: &Pending,
        right: f64,
    ) {
        let limb = &history.limbs[pending.limb];
        let first = pending.index * GROWTH_STEP;
        let last = (first + GROWTH_STEP).min(limb.moments.len()) - 1;
        let own = &limb.moments[first..=last];
        let (start, mut heading) = if let Some(&previous) = self.limb_steps[pending.limb].last() {
            let step = &self.steps[previous];
            (step.end, step.heading)
        } else {
            self.sprout(history, pending.limb, seed, right)
        };
        let mut best: Option<(f64, f64, (f64, f64))> = None;
        for turn in TURNS {
            let angle = turn.mul_add(TURN, heading) + seed.between(-WANDER, WANDER);
            let end = (
                libm::cos(angle).mul_add(STEP_LENGTH, start.0),
                libm::sin(angle).mul_add(STEP_LENGTH, start.1),
            );
            let crowd = room.crowd(end.0, end.1, pending.limb, pending.index) as f64;
            let mut score = LIGHT.mul_add(libm::sin(angle), (-CROWD).mul_add(crowd, -turn.abs()));
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
            f64::midpoint(start.0, end.0),
            f64::midpoint(start.1, end.1),
            pending.limb,
            pending.index,
        );
        let id = self.steps.len();
        let mut sounds = Vec::new();
        let mut widest: f64 = 1.0;
        // A moment's place on its step is by its place in the step, out of
        // a full step, never out of how many the step holds yet: a step
        // still filling keeps every moment where it first sat, and so does
        // any bud grown from one of them.
        for (i, &m) in own.iter().enumerate() {
            let t = (i + 1) as f64 / GROWTH_STEP as f64;
            self.at[m] = Some((
                (end.0 - start.0).mul_add(t, start.0),
                (end.1 - start.1).mul_add(t, start.1),
            ));
            self.step_of[m] = Some(id);
            let moment = &history.moments[m];
            if let Some(sound) = moment.sound {
                sounds.push(sound);
            }
            let width = width_for(moment.modules);
            widest = widest.max(width);
            self.look[m] = Some(Look {
                width,
                sound: mean_of(&sounds),
            });
        }
        self.steps.push(Step {
            limb: pending.limb,
            first,
            last,
            start,
            end,
            heading,
            widest,
        });
        self.limb_steps[pending.limb].push(id);
    }

    /// Where a limb's first step starts and which way it heads: from the
    /// moment it grew from, turned off its parent's heading to one side. A
    /// limb with no parent is a tree of its own: the first stands at the
    /// origin, and each after it to the right of everything grown before
    /// it (`right`), so separate trees never grow into each other and,
    /// once placed, never move.
    fn sprout(
        &mut self,
        history: &History,
        limb: usize,
        seed: &mut Seeded,
        right: f64,
    ) -> ((f64, f64), f64) {
        let side: f64 = if seed.next() & 1 == 0 { -1.0 } else { 1.0 };
        let spread = seed.between(0.6, 1.1);
        let from = history.limbs[limb].from;
        if let Some(moment) = from
            && let Some(step) = self.step_of.get(moment).copied().flatten()
            && let Some(at) = self.at[moment]
        {
            return (at, side.mul_add(spread, self.steps[step].heading));
        }
        let x = if self.roots.is_empty() {
            0.0
        } else {
            right + TREE_GAP + ROOT_REACH
        };
        self.roots.push((x, 0.0));
        ((x, 0.0), FRAC_PI_2)
    }

    fn measure(&self) -> (f64, f64, f64, f64) {
        let mut bounds = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let mut grow_to = |(x, y): (f64, f64), pad: f64| {
            bounds.0 = bounds.0.min(x - pad);
            bounds.1 = bounds.1.min(y - pad);
            bounds.2 = bounds.2.max(x + pad);
            bounds.3 = bounds.3.max(y + pad);
        };
        for step in &self.steps {
            grow_to(step.start, step.widest);
            grow_to(step.end, step.widest);
        }
        for &(x, y) in &self.roots {
            grow_to((x - ROOT_REACH, y - ROOT_DEPTH), 0.0);
            grow_to((x + ROOT_REACH, y), 0.0);
        }
        if bounds.0 > bounds.2 {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            bounds
        }
    }
}

#[cfg(test)]
mod tests;
