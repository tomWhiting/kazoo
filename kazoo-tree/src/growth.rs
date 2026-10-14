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

use crate::colour::{self, Tint};
use crate::history::{History, Sound};
use crate::ledger::{Saved, Shape, StepKey};

/// The version of how the tree grows.
///
/// Every constant here shapes every tree drawn; changing one moves every place a person remembers, so it is
/// done only on purpose: this moves with it, and so does the golden test.
pub const GROWTH_VERSION: u32 = 4;
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
/// How strongly a bud's step prefers to rise.
const LIGHT: f64 = 0.6;
/// How strongly a tree's own trunk prefers to rise: enough that leaning
/// sideways always turns it back up, so a trunk never curls over and grows
/// down.
const TRUNK_LIGHT: f64 = 2.5;
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
    /// Its place in [`Growth::steps`].
    pub id: usize,
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
    /// Its colour: every module alive at it, each its own shade from its
    /// settings, together, lighter for the mean sound of the step's moments
    /// so far. Only ever the past: a stretch drawn once is drawn the same
    /// way however the step fills after it.
    pub colour: (u8, u8, u8),
    /// Everything from its tree's first moment up to and including it,
    /// along the limbs it grew from, added up: so far out the colour of any
    /// run of moments is one subtraction.
    pub running: Running,
}

/// Sums along a path of moments, from a tree's first moment.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Running {
    /// The moments' tints.
    pub tint: Tint,
    /// How many moments.
    pub moments: u64,
    /// Their loudness, dBFS, for those that kept their sound.
    pub rms_db: f64,
    /// How many kept their sound.
    pub heard: u64,
}

impl Running {
    /// These sums with `moment` added.
    fn and(&self, tint: &Tint, sound: Option<&Sound>) -> Self {
        let rms = sound.map(|sound| {
            if sound.rms_db.is_finite() {
                f64::from(sound.rms_db)
            } else {
                SILENT_DB
            }
        });
        Self {
            tint: self.tint.plus(tint),
            moments: self.moments + 1,
            rms_db: self.rms_db + rms.unwrap_or(0.0),
            heard: self.heard + u64::from(rms.is_some()),
        }
    }

    /// The moments in these sums that are not in `before`'s, as a colour:
    /// their tints' mean, lightened by their mean loudness.
    #[must_use]
    pub fn colour_since(&self, before: &Self) -> (u8, u8, u8) {
        let moments = self.moments.saturating_sub(before.moments);
        let heard = self.heard.saturating_sub(before.heard);
        let rms = (heard > 0).then(|| (self.rms_db - before.rms_db) / heard as f64);
        colour::blend(&self.tint.minus(&before.tint), moments as f64, rms)
    }
}

/// How loud a sound that does not read is taken to be, dBFS.
const SILENT_DB: f64 = -60.0;

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
    /// Each step's name, for the ledger.
    keys: Vec<StepKey>,
    /// Each step's first moment, to see that a history read again still
    /// lines up with what was grown.
    firsts: Vec<usize>,
    /// Which grid cells hold growth.
    room: Room,
    /// Each limb's line of growth by its key, numbered in the order first
    /// grown: what the room knows a limb by.
    lines: HashMap<String, usize>,
    /// The right edge of everything grown, counting every step as wide as
    /// any can be drawn: where a separate tree stands.
    right: f64,
}

/// A small, fixed random source (`SplitMix64`), so a limb's wander is the
/// same on every machine and every run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Seeded(u64);

impl Seeded {
    /// The seed of step `index` of the limb whose first commit is `key`:
    /// its own, whatever grew before it, so a step grown now and one
    /// adopted from the ledger leave the next step the same seed.
    fn of_step(key: &str, index: usize) -> Self {
        let mut seed = Self::of(key);
        seed.0 ^= (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        seed.next();
        seed
    }

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
/// through it, as `(line, step on the line)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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
    f64::from(modules).sqrt().mul_add(0.55, 1.0).min(WIDEST)
}

/// The widest a moment is ever drawn, world units.
pub const WIDEST: f64 = 8.0;

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

/// A limb's name for growing: the commit of its first moment of its own,
/// which never changes however the tree is read or what its branch is
/// called. `None` for a limb with no moments yet.
fn limb_key(history: &History, limb: usize) -> Option<&str> {
    history.limbs[limb]
        .moments
        .first()
        .map(|&m| &*history.moments[m].id)
}

impl Growth {
    /// Grow the tree for `history` afresh, in the order its own times
    /// give (no ledger).
    #[must_use]
    pub fn grow(history: &History) -> Self {
        let mut growth = Self::default();
        growth.grow_on(history, &[]);
        growth
    }

    /// Grow on to `history`: first every step `known` names (the ledger's
    /// order, the order the tree first grew in) that is not grown yet, then
    /// every step still not grown, by its causal key (its first moment's
    /// time, held no earlier than the step before it on its limb, and for a
    /// limb's first step, the step holding the moment it grew from), then
    /// parents before their buds, then step order. What was grown before is
    /// kept as it is; only its tips fill in. Answers the steps grown now
    /// that `known` did not name, in the order they grew, for the ledger.
    ///
    /// A history read afresh, whose steps no longer line up with what was
    /// grown, is grown again from nothing, still in `known`'s order.
    pub fn grow_on(&mut self, history: &History, known: &[Saved]) -> Vec<Saved> {
        if !self.lines_up(history) {
            *self = Self::default();
        }
        self.reindex(history);
        // Every limb's last step grown may have filled since: its new
        // moments are placed before anything grows after it.
        for l in 0..history.limbs.len() {
            if let Some(&last) = self.limb_steps[l].last() {
                self.place(history, last);
            }
        }
        let mut fresh = Vec::new();
        let mut grown: Vec<Vec<bool>> = history
            .limbs
            .iter()
            .map(|limb| vec![false; limb.moments.len().div_ceil(GROWTH_STEP)])
            .collect();
        for (l, steps) in self.limb_steps.iter().enumerate() {
            for (i, _) in steps.iter().enumerate() {
                grown[l][i] = true;
            }
        }
        let by_key: HashMap<&str, usize> = (0..history.limbs.len())
            .filter_map(|l| limb_key(history, l).map(|key| (key, l)))
            .collect();
        let ready = |grown: &[Vec<bool>], limb: usize, index: usize| {
            if index > 0 {
                return grown[limb][index - 1];
            }
            history.limbs[limb]
                .from
                .and_then(|m| history.place_of(m))
                .is_none_or(|(parent, at)| grown[parent][at / GROWTH_STEP])
        };
        for step in known {
            let key = &step.key;
            let Some(&limb) = by_key.get(key.limb.as_str()) else {
                continue;
            };
            if key.index < grown[limb].len()
                && !grown[limb][key.index]
                && ready(&grown, limb, key.index)
            {
                self.grow_step(history, limb, key.index, step.shape);
                grown[limb][key.index] = true;
            }
        }
        for pending in Self::order(history) {
            if grown[pending.limb][pending.index] {
                continue;
            }
            let before = self.steps.len();
            self.grow_step(history, pending.limb, pending.index, None);
            grown[pending.limb][pending.index] = true;
            if let Some(step) = self.steps.get(before) {
                fresh.push(Saved {
                    key: self.keys[before].clone(),
                    shape: Some(Shape {
                        start: step.start,
                        end: step.end,
                        heading: step.heading,
                    }),
                });
            }
        }
        self.bounds = self.measure();
        fresh
    }

    /// Whether what was grown is still `history`'s: each step's limb is
    /// still there, by its first commit, with the same moment first in the
    /// step.
    fn lines_up(&self, history: &History) -> bool {
        if history.moments.len() < self.at.len() {
            return false;
        }
        let by_key: HashMap<&str, usize> = (0..history.limbs.len())
            .filter_map(|l| limb_key(history, l).map(|key| (key, l)))
            .collect();
        self.steps.iter().zip(&self.keys).all(|(step, key)| {
            by_key.get(key.limb.as_str()).is_some_and(|&l| {
                history.limbs[l].moments.get(step.first) == Some(&self.firsts[step.id])
            })
        })
    }

    /// Fit the grown steps to `history`'s limbs, which may be numbered
    /// differently now (a new bud sorts among the old ones), and make room
    /// for its new moments.
    fn reindex(&mut self, history: &History) {
        let moments = history.moments.len();
        self.at.resize(moments, None);
        self.step_of.resize(moments, None);
        self.look.resize(moments, None);
        let by_key: HashMap<&str, usize> = (0..history.limbs.len())
            .filter_map(|l| limb_key(history, l).map(|key| (key, l)))
            .collect();
        self.limb_steps = vec![Vec::new(); history.limbs.len()];
        for (s, key) in self.keys.iter().enumerate() {
            if let Some(&l) = by_key.get(key.limb.as_str()) {
                self.steps[s].limb = l;
                self.limb_steps[l].push(s);
            }
        }
        for steps in &mut self.limb_steps {
            steps.sort_by_key(|&s| self.steps[s].first);
        }
    }

    /// Every step not grown yet, in its causal order (see [`Self::grow_on`]).
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

    /// Grow step `index` of `limb`: choose where it heads, take its room,
    /// and place its moments.
    fn grow_step(&mut self, history: &History, limb: usize, index: usize, saved: Option<Shape>) {
        let Some(key) = limb_key(history, limb).map(str::to_owned) else {
            return;
        };
        let line = if let Some(&line) = self.lines.get(&key) {
            line
        } else {
            let line = self.lines.len();
            self.lines.insert(key.clone(), line);
            line
        };
        if let Some(shape) = saved {
            // Drawn before: kept exactly as it was drawn.
            if index == 0
                && self.limb_steps[limb].is_empty()
                && !self.grows_from_a_limb(history, limb)
            {
                self.roots.push(shape.start);
                self.right = self.right.max(shape.start.0 + ROOT_REACH);
            }
            self.add_step(history, limb, index, line, key, shape);
            return;
        }
        let mut seed = Seeded::of_step(&key, index);
        let (start, heading) = if let Some(&previous) = self.limb_steps[limb].last() {
            let step = &self.steps[previous];
            (step.end, step.heading)
        } else {
            self.sprout(history, limb, line, &mut seed)
        };
        // A tree's own trunk reaches for the light; its buds wander out
        // looking for room.
        let light = if self.grows_from_a_limb(history, limb) {
            LIGHT
        } else {
            TRUNK_LIGHT
        };
        let mut best: Option<(f64, f64, (f64, f64))> = None;
        for turn in TURNS {
            let angle = turn.mul_add(TURN, heading) + seed.between(-WANDER, WANDER);
            let end = (
                libm::cos(angle).mul_add(STEP_LENGTH, start.0),
                libm::sin(angle).mul_add(STEP_LENGTH, start.1),
            );
            let crowd = self.room.crowd(end.0, end.1, line, index) as f64;
            let mut score = light.mul_add(libm::sin(angle), (-CROWD).mul_add(crowd, -turn.abs()));
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
        let shape = Shape {
            start,
            end,
            heading: angle,
        };
        self.add_step(history, limb, index, line, key, shape);
    }

    /// Whether `limb` grew from a moment on another limb (a bud), rather
    /// than standing as a tree of its own.
    fn grows_from_a_limb(&self, history: &History, limb: usize) -> bool {
        history.limbs[limb]
            .from
            .is_some_and(|m| self.step_of.get(m).copied().flatten().is_some())
    }

    /// Add step `index` of `limb` with `shape`: take its room, keep its
    /// name for the ledger, and place its moments.
    fn add_step(
        &mut self,
        history: &History,
        limb: usize,
        index: usize,
        line: usize,
        key: String,
        shape: Shape,
    ) {
        let Shape {
            start,
            end,
            heading,
        } = shape;
        self.room.take(end.0, end.1, line, index);
        self.room.take(
            f64::midpoint(start.0, end.0),
            f64::midpoint(start.1, end.1),
            line,
            index,
        );
        let id = self.steps.len();
        let first = index * GROWTH_STEP;
        self.steps.push(Step {
            id,
            limb,
            first,
            last: first,
            start,
            end,
            heading,
            widest: 1.0,
        });
        self.keys.push(StepKey { limb: key, index });
        self.firsts.push(history.limbs[limb].moments[first]);
        self.limb_steps[limb].push(id);
        self.right = self.right.max(start.0.max(end.0) + WIDEST);
        self.place(history, id);
    }

    /// Place step `id`'s moments, as many as its limb holds now. A moment's
    /// place on its step is by its place in the step, out of a full step,
    /// never out of how many the step holds yet: a step still filling keeps
    /// every moment where it first sat, and so does any bud grown from one
    /// of them.
    fn place(&mut self, history: &History, id: usize) {
        let step = &self.steps[id];
        let moments = &history.limbs[step.limb].moments;
        let last = (step.first + GROWTH_STEP).min(moments.len()) - 1;
        let (start, end, first, step_limb) = (step.start, step.end, step.first, step.limb);
        let own = &moments[first..=last];
        let mut sounds = Vec::new();
        let mut widest: f64 = 1.0;
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
            // The moment before it on its path: on its limb, or for a
            // bud's first, the moment it grew from.
            let before = (first + i)
                .checked_sub(1)
                .map(|p| moments[p])
                .or(history.limbs[step_limb].from)
                .and_then(|b| self.look[b])
                .map_or_else(Running::default, |look| look.running);
            let sound = mean_of(&sounds);
            self.look[m] = Some(Look {
                width,
                colour: colour::colour(&moment.tint, sound.as_ref()),
                running: before.and(&moment.tint, moment.sound.as_ref()),
            });
        }
        let step = &mut self.steps[id];
        step.last = last;
        step.widest = widest;
    }

    /// Where a limb's first step starts and which way it heads: from the
    /// moment it grew from, turned off its parent's heading to one side. A
    /// limb with no parent is a tree of its own: the first stands at the
    /// origin, and each after it to the right of everything grown before
    /// it, counting every step as wide as any can be drawn, so separate
    /// trees never grow into each other and, once placed, never move.
    fn sprout(
        &mut self,
        history: &History,
        limb: usize,
        line: usize,
        seed: &mut Seeded,
    ) -> ((f64, f64), f64) {
        let mut side: f64 = if seed.next() & 1 == 0 { -1.0 } else { 1.0 };
        let spread = seed.between(0.6, 1.1);
        let from = history.limbs[limb].from;
        if let Some(moment) = from
            && let Some(step) = self.step_of.get(moment).copied().flatten()
            && let Some(at) = self.at[moment]
        {
            // Bud toward the side with more room, a few steps out; the
            // limb's own seed decides only when both sides are as open.
            let parent = self.steps[step].heading;
            let crowd = |side: f64| {
                let heading = side.mul_add(spread, parent);
                (1..=3)
                    .map(|reach| {
                        let far = STEP_LENGTH * f64::from(reach);
                        self.room.crowd(
                            libm::cos(heading).mul_add(far, at.0),
                            libm::sin(heading).mul_add(far, at.1),
                            line,
                            0,
                        )
                    })
                    .sum::<usize>()
            };
            let (here, there) = (crowd(side), crowd(-side));
            if there < here {
                side = -side;
            }
            return (at, side.mul_add(spread, parent));
        }
        let x = if self.roots.is_empty() {
            0.0
        } else {
            self.right + TREE_GAP + ROOT_REACH
        };
        self.roots.push((x, 0.0));
        self.right = self.right.max(x + ROOT_REACH);
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
