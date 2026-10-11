//! The shifter's read taps: where they read, when they splice, and where a
//! splice lands so that no attack is heard twice or lost.

use std::f32::consts::FRAC_PI_2;

use super::super::parts::{SINC_SHORTEST, Sinc};
use crate::dsp::DelayLine;

/// The read delay's floor, seconds: half a millisecond, never closer than
/// the band-limited read allows.
const FLOOR: f32 = 0.000_5;
/// How far a new tap may slide to match the heard one, seconds each way,
/// and how much audio the match compares.
const SEARCH: f32 = 0.008;
const MATCH: f32 = 0.008;
/// How far a new tap starts from the heard one, seconds, for shifts of up
/// to an octave up (further beyond, in step with the shift).
const JUMP: f32 = 0.025;
/// How long two taps crossfade, seconds.
const OVERLAP: f32 = 0.012;
/// The coarse alignment pass looks at the audio at about this rate.
const COARSE_RATE: f32 = 12_000.0;
/// The largest ratio there can be: two octaves and a semitone up.
pub const MOST_RATIO: f32 = 4.238;

/// An attack: how far after it is flagged a splice may land (the body of
/// a click or a pluck is over by then), how far before the flag it may
/// have begun, and the least jump a splice ever makes, seconds.
const AFTER_ATTACK: f32 = 0.004;
const BEFORE_ATTACK: f32 = 0.002;
const LEAST_JUMP: f32 = 0.002;
/// How soon, seconds, the heard tap must reach an attack for a falling
/// splice to wait for it.
const WAIT: f32 = 0.01;
/// The attack detector: a fast and a slow envelope, seconds; how far the
/// fast one must rise over the slow one; the quietest level that counts;
/// and how long after one attack the next may be flagged.
const FAST: f32 = 0.001;
const SLOW: f32 = 0.03;
const RISE: f32 = 2.5;
const QUIETEST: f32 = 1e-4;
const REFRACTORY: f32 = 0.04;
/// How many recent attacks are remembered: more than can lie within the
/// longest reach of a tap at the shortest refractory spacing.
const ATTACKS: usize = 8;

/// The shifter's distances, in samples at the running rate.
#[derive(Debug, Clone, Copy, Default)]
pub struct Geometry {
    pub floor: f32,
    search: i32,
    pub span: usize,
    jump: f32,
    overlap: f32,
    stride: i32,
    after: f32,
    before: f32,
    least: f32,
    wait: f32,
    /// The furthest back a tap ever reads.
    pub longest: f32,
}

impl Geometry {
    pub fn new(rate: f32) -> Self {
        let floor = (FLOOR * rate).max(SINC_SHORTEST as f32 + 2.0).round();
        let search = (SEARCH * rate).round();
        let jump = JUMP * rate;
        let overlap = OVERLAP * rate;
        Self {
            floor,
            search: search as i32,
            span: ((MATCH * rate) as usize).max(1),
            jump,
            overlap,
            stride: (rate / COARSE_RATE).round().max(1.0) as i32,
            after: AFTER_ATTACK * rate,
            before: BEFORE_ATTACK * rate,
            least: LEAST_JUMP * rate,
            wait: WAIT * rate,
            longest: jump.mul_add(
                MOST_RATIO - 1.0,
                2.0f32.mul_add(overlap + search, floor) + 8.0,
            ),
        }
    }

    /// Where a tap starts: with no shift it stays here, a plain delay.
    pub fn start(&self) -> f32 {
        (0.5f32.mul_add(self.jump, self.floor) + self.search as f32).round()
    }
}

/// Recent attacks in the input, by the frame they were heard at.
#[derive(Debug, Clone, Copy)]
pub struct Onsets {
    fast: f32,
    slow: f32,
    fast_coeff: f32,
    slow_coeff: f32,
    refractory: u64,
    /// Frames heard so far.
    now: u64,
    /// The frames of the latest attacks, a ring; `u64::MAX` for none.
    heard: [u64; ATTACKS],
    next: usize,
}

impl Default for Onsets {
    fn default() -> Self {
        Self {
            fast: 0.0,
            slow: 0.0,
            fast_coeff: 1.0,
            slow_coeff: 1.0,
            refractory: 0,
            now: 0,
            heard: [u64::MAX; ATTACKS],
            next: 0,
        }
    }
}

impl Onsets {
    pub fn prepare(&mut self, rate: f32) {
        let coeff = |seconds: f32| -(-1.0 / (seconds * rate)).exp_m1();
        self.fast_coeff = coeff(FAST);
        self.slow_coeff = coeff(SLOW);
        self.refractory = (REFRACTORY * rate) as u64;
        self.clear();
    }

    pub const fn clear(&mut self) {
        self.fast = 0.0;
        self.slow = 0.0;
        self.now = 0;
        self.heard = [u64::MAX; ATTACKS];
        self.next = 0;
    }

    /// Hear one frame of the input.
    pub fn push(&mut self, sample: f32) {
        let size = sample.abs();
        self.fast = (size - self.fast).mul_add(self.fast_coeff, self.fast);
        self.slow = (size - self.slow).mul_add(self.slow_coeff, self.slow);
        crate::dsp::flush(&mut self.fast);
        crate::dsp::flush(&mut self.slow);
        let latest = self.heard[(self.next + ATTACKS - 1) % ATTACKS];
        let rested = latest == u64::MAX || self.now.saturating_sub(latest) >= self.refractory;
        if rested && self.fast > RISE.mul_add(self.slow, QUIETEST) {
            self.heard[self.next] = self.now;
            self.next = (self.next + 1) % ATTACKS;
        }
        self.now = self.now.wrapping_add(1);
    }

    /// The ages, in frames, of the attacks remembered (1 is the frame just
    /// heard, as a delay line reads).
    fn ages(&self) -> impl Iterator<Item = f32> + '_ {
        self.heard
            .iter()
            .filter(|frame| **frame != u64::MAX)
            .map(|frame| (self.now - frame) as f32)
    }

    /// The youngest attack whose age lies in `low..=high`, if any.
    fn youngest_within(&self, low: f32, high: f32) -> Option<f32> {
        self.ages()
            .filter(|age| (low..=high).contains(age))
            .fold(None, |best, age| {
                Some(best.map_or(age, |b: f32| b.min(age)))
            })
    }

    /// The oldest attack whose age lies in `low..=high`, if any.
    fn oldest_within(&self, low: f32, high: f32) -> Option<f32> {
        self.ages()
            .filter(|age| (low..=high).contains(age))
            .fold(None, |best, age| {
                Some(best.map_or(age, |b: f32| b.max(age)))
            })
    }

    /// The oldest attack younger than `age`, if any: the first a tap
    /// reading at `age` will reach.
    fn next_before(&self, age: f32) -> Option<f32> {
        self.ages()
            .filter(|attack| *attack < age)
            .fold(None, |best, attack| {
                Some(best.map_or(attack, |b: f32| b.max(attack)))
            })
    }
}

/// The two read taps: the one heard, and the one fading out after a
/// splice.
#[derive(Debug, Clone, Copy, Default)]
pub struct Taps {
    pub heard: f32,
    pub fading: f32,
    /// How far through the crossfade, 0 to 1; 1 when only one tap plays.
    fade: f32,
    step: f32,
    /// How alike the two taps' audio was at the splice, 0 to 1.
    alike: f32,
}

impl Taps {
    pub const fn settle(&mut self, at: f32) {
        self.heard = at;
        self.fading = at;
        self.fade = 1.0;
        self.step = 0.0;
        self.alike = 0.0;
    }

    /// Move both taps on by a sample at `drift` (`1 - ratio`), splicing to
    /// a new tap when the heard one reaches the end of its travel. True when
    /// a splice starts.
    pub fn advance(
        &mut self,
        drift: f32,
        shape: &Geometry,
        guide: &DelayLine,
        onsets: &Onsets,
    ) -> bool {
        let hold = |delay: f32| delay.clamp(shape.floor, shape.longest);
        self.heard = hold(self.heard + drift);
        if self.fade < 1.0 {
            self.fading = hold(self.fading + drift);
            // A tap held at the end of its travel stops shifting, and heard
            // like that it kinks the wave: when the shift speeds up through
            // a crossfade, the crossfade speeds up to end first.
            let room = |delay: f32| {
                if drift < 0.0 {
                    delay - shape.floor
                } else {
                    shape.longest - delay
                }
            };
            let pace = drift.abs();
            if pace > 0.0 {
                let travel = room(self.fading).min(room(self.heard)) / pace;
                let left = (1.0 - self.fade) / self.step.max(1e-9);
                if travel < left {
                    self.step = (1.0 - self.fade) / travel.max(1.0);
                }
            }
            self.hurry_past_attacks(drift, shape, onsets);
            self.fade = (self.fade + self.step).min(1.0);
            return false;
        }
        let pace = drift.abs();
        let jump = shape.jump * pace.max(1.0);
        let heard = self.heard;
        let placed = if drift < 0.0 && heard <= shape.overlap.mul_add(pace.min(1.0), shape.floor) {
            Some(Self::rising(heard, jump, shape, onsets))
        } else if drift > 0.0 && heard >= shape.floor + shape.search as f32 + shape.jump {
            Self::falling(heard, jump, shape, onsets, 1.0 - drift)
        } else {
            None
        };
        let Some((target, low, high)) = placed else {
            return false;
        };
        let (at, alike) = self.align(target, low, high, shape, guide);
        self.fading = heard;
        self.heard = at;
        self.fade = 0.0;
        self.step = pace.max(1.0) / shape.overlap.max(1.0);
        self.alike = alike;
        self.hurry_past_attacks(drift, shape, onsets);
        true
    }

    /// An attack must be played whole by one tap. One the heard tap has
    /// still to reach, it plays: met while it is still fading in, the
    /// attack would come at part strength, and met by the fading tap too
    /// (if that has still to reach it), twice. So the crossfade ends before
    /// either tap gets there. (An attack only the fading tap will reach is
    /// its own to play; the splice was placed to keep those out.) A tap at
    /// delay `d` meets an attack of age `a` after `(d - a) / ratio` frames:
    /// its delay moves by `drift` a frame, the attack's age by one.
    fn hurry_past_attacks(&mut self, drift: f32, shape: &Geometry, onsets: &Onsets) {
        let ratio = 1.0 - drift;
        if self.fade >= 1.0 || ratio <= 0.0 {
            return;
        }
        let Some(attack) = onsets.next_before(self.heard) else {
            return;
        };
        let heard = (self.heard - attack) / ratio;
        let fading = if attack < self.fading {
            (self.fading - attack) / ratio
        } else {
            f32::INFINITY
        };
        let meeting = heard.min(fading) - shape.before;
        let left = (1.0 - self.fade) / self.step.max(1e-9);
        if meeting < left {
            self.step = (1.0 - self.fade) / meeting.max(1.0);
        }
    }

    /// Where a rising tap's successor starts, and how far its alignment may
    /// slide it (in samples, each way). It jumps back over audio the heard
    /// tap has played; if an attack lies there, it lands after the attack,
    /// so the attack is not heard twice.
    fn rising(heard: f32, jump: f32, shape: &Geometry, onsets: &Onsets) -> (f32, i32, i32) {
        let search = shape.search;
        let reach = heard + jump + search as f32 + shape.before;
        onsets
            .youngest_within(heard, reach)
            .map_or((heard + jump, -search, search), |attack| {
                // Never younger than the heard tap: that would skip audio.
                let target = (attack - shape.after).max(heard + shape.least);
                let low = ((heard + shape.least - target).ceil() as i32).max(-search);
                (target, low, 0)
            })
    }

    /// Where a falling tap's successor starts, and how far its alignment
    /// may slide it, or none for now. It jumps forward over audio no tap
    /// has played yet (ages from the heard tap's down to the jump's). An
    /// attack the heard tap will reach within [`WAIT`] it is left to play:
    /// the splice waits. One further on would be skipped, so the new tap
    /// jumps only as far as just before it, and plays it whole.
    fn falling(
        heard: f32,
        jump: f32,
        shape: &Geometry,
        onsets: &Onsets,
        ratio: f32,
    ) -> Option<(f32, i32, i32)> {
        let search = shape.search;
        let Some(attack) = onsets.oldest_within(heard - jump - search as f32, heard) else {
            return Some((heard - jump, -search, search));
        };
        if (heard - attack) / ratio.max(1e-3) <= shape.wait {
            return None;
        }
        Some((attack + shape.before, 0, search))
    }

    /// Where within `low..=high` samples of `target` a new tap's audio best
    /// matches the heard tap's, and how alike the two are there.
    fn align(
        &self,
        target: f32,
        low: i32,
        high: i32,
        shape: &Geometry,
        guide: &DelayLine,
    ) -> (f32, f32) {
        let here = self.heard.round() as usize;
        let centre = target.round();
        let lowest = (shape.floor - centre) as i32;
        let highest = (shape.longest - shape.span as f32 - centre) as i32;
        let (low, high) = (lowest.max(low), highest.min(high));
        if low > high {
            return (target.clamp(shape.floor, shape.longest), 0.0);
        }
        let score = |lag: i32, step: usize| {
            let at = (centre as i64 + i64::from(lag)).max(1) as usize;
            let (mut both, mut mine, mut theirs) = (0.0f32, 0.0f32, 0.0f32);
            let mut i = 0;
            while i < shape.span {
                let (x, y) = (guide.tap(at + i), guide.tap(here + i));
                both = x.mul_add(y, both);
                mine = x.mul_add(x, mine);
                theirs = y.mul_add(y, theirs);
                i += step;
            }
            (both, mine, theirs)
        };
        let fit = |(both, mine, _): (f32, f32, f32)| both / (mine + 1e-20).sqrt();
        let stride = shape.stride.max(1);
        let coarse_step = stride.unsigned_abs() as usize;
        // Where nothing matches better (silence matches everywhere alike),
        // the jump as planned stands.
        let planned = 0.clamp(low, high);
        let mut best = (fit(score(planned, coarse_step)), planned);
        let mut lag = low;
        while lag <= high {
            let value = fit(score(lag, coarse_step));
            if value > best.0 {
                best = (value, lag);
            }
            lag += stride;
        }
        let fine_step = (coarse_step / 4).max(1);
        let coarse = best.1;
        best = (fit(score(coarse, fine_step)), coarse);
        for lag in (coarse - stride).max(low)..=(coarse + stride).min(high) {
            let value = fit(score(lag, fine_step));
            if value > best.0 {
                best = (value, lag);
            }
        }
        let (both, mine, theirs) = score(best.1, fine_step);
        let energy = (mine * theirs).sqrt();
        let alike = if energy > 1e-12 && both.is_finite() {
            (both / energy).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (centre + best.1 as f32, alike)
    }

    /// The heard and the fading tap's gains: one and nothing between
    /// splices; through a splice, a quarter-sine crossfade scaled so its
    /// power holds whether the taps' audio is alike or not.
    pub fn gains(&self) -> (f32, f32) {
        if self.fade >= 1.0 {
            return (1.0, 0.0);
        }
        let (rise, fall) = (FRAC_PI_2 * self.fade).sin_cos();
        let scale = 1.0 / (2.0 * self.alike * rise).mul_add(fall, 1.0).sqrt();
        (rise * scale, fall * scale)
    }

    /// `line` through both taps, each `later` samples further back.
    pub fn read(
        &self,
        sinc: &Sinc,
        line: &DelayLine,
        (heard, fading): (f32, f32),
        later: f32,
    ) -> f32 {
        let now = heard * sinc.read(line, f64::from(self.heard + later));
        if fading > 0.0 {
            fading.mul_add(sinc.read(line, f64::from(self.fading + later)), now)
        } else {
            now
        }
    }
}
