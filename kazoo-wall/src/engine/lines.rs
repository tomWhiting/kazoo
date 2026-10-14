//! Delay lines on the cables, keeping every signal in step.
//!
//! An effect that oversamples holds its sound back a few dozen frames. When
//! its output meets a path that went round it (a dry signal and a wet one
//! summed in a mixer), the two must arrive together or they comb-filter.
//! The control side works out how late each cable's signal arrives and
//! gives every cable into a module the delay that lines it up with the
//! latest (see [`super::tables::Route::delay`]); this is where the delay
//! is applied.
//!
//! A jack whose cable has needed delay keeps a line, written every time its
//! signal passes, so its history survives changes back to zero delay. A
//! cable already playing direct through that jack joins its first line by
//! reading one sub-block at zero delay to fill its history, then gliding
//! to the new delay. A new cable, or one whose destination has just become
//! heard again, starts silent for its delay. For a cable already playing,
//! the signal never breaks when its delay changes:
//!
//! - Audio and control voltages are read from a point that glides from the
//!   old delay to the new one, like a tape head, over at least
//!   [`SHORTEST_FADE_SECONDS`] and [`FADE_PER_FRAME`] frames for every
//!   frame the delay moves. The signal plays at most 2% fast or slow while
//!   it glides (about a third of a semitone) and keeps its level at every
//!   pitch; between samples it is read with a cubic curve. (Fading from
//!   one reading point to the other instead would cancel any pitch whose
//!   period fits the move twice over.)
//! - A gate switches between pulses: once the old reading point is low, the
//!   gate is held low until the new one is too, so every pulse that comes
//!   out is whole (one may be skipped). A gate held high for longer than a
//!   line remembers has no edge to break, and switches while high.
//!
//! Lines are allocated on the control side and live in the destination
//! slot's page, so a room move preserves their history.

use super::slice_of;
use super::tables::{MAX_CABLE_DELAY, Route};
use crate::SUB_BLOCK;
use crate::dsp::{Block, GATE_HIGH, finite};

/// Frames in each line: a power of two past the longest delay and a
/// sub-block, with room for the cubic curve's farthest point.
const LINE_LEN: usize = 2_048;

/// The shortest glide when a delay changes.
pub const SHORTEST_FADE_SECONDS: f32 = 0.005;

/// The longest glide when a delay changes, unless the rate is so low that
/// gliding the longest delay at [`FADE_PER_FRAME`] takes longer.
pub const LONGEST_FADE_SECONDS: f32 = 2.0;

/// Glide frames for every frame a delay moves: moving D frames over 50·D
/// plays the signal at most 1/50 (2%) fast or slow.
pub const FADE_PER_FRAME: u32 = 50;

// Every delay, a sub-block and the curve's farthest point must fit a line.
const _: () = assert!((MAX_CABLE_DELAY as usize) + SUB_BLOCK + 3 < LINE_LEN);
const _: () = assert!(LINE_LEN.is_power_of_two());

/// How a gate cable is moving to a new delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// Reading the current delay.
    Steady,
    /// Waiting for the current reading point to go low; counts the frames
    /// it has been high.
    Waiting(u32),
    /// Held low until the new reading point is low; counts the frames it
    /// has been high.
    Muted(u32),
}

/// One line's reading state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Tap {
    /// The cable that owns the line (0: nobody yet).
    tag: u32,
    /// Where the next frame is written.
    write: usize,
    /// The delay being read, in frames (a gate's is always whole).
    at: f64,
    /// The delay the reading point is gliding to, or reads.
    target: u16,
    /// Whether the reading point is on its way to `target`.
    gliding: bool,
    /// How far the reading point moves each frame while gliding.
    speed: f64,
    /// A gate's move.
    gate: Gate,
    /// What the cable is multiplied by now.
    gain: f32,
    /// The amount it is moving to (or rests on): the last amount it saw.
    aim: f32,
    /// How far `gain` is from `aim` per frame left, while moving.
    gain_step: f32,
    /// Frames left to reach `aim`; 0 at rest.
    gain_left: u32,
}

impl Tap {
    pub(super) const EMPTY: Self = Self {
        tag: 0,
        write: 0,
        at: 0.0,
        target: 0,
        gliding: false,
        speed: 0.0,
        gate: Gate::Steady,
        gain: 0.0,
        aim: 0.0,
        gain_step: 0.0,
        gain_left: 0,
    };
}

impl Tap {
    pub(super) fn carry_direct(&mut self, route: &Route, source: &Block, out: &mut Block) {
        if self.tag != route.tag {
            let gain = if route.glide > 0 { 0.0 } else { route.amount };
            *self = Self {
                tag: route.tag,
                gain,
                aim: gain,
                ..Self::EMPTY
            };
        }
        self.prepare_gain(route);
        *out = *source;
        self.scale(out);
    }

    fn prepare_gain(&mut self, route: &Route) {
        if route.amount.to_bits() != self.aim.to_bits() {
            if route.glide == 0 {
                self.gain = route.amount;
                self.aim = route.amount;
                self.gain_left = 0;
            } else {
                // Frame counts are close enough in f32 for the line;
                // computing from the aim makes the last frame exact.
                self.gain_step = (route.amount - self.gain) / route.glide as f32;
                self.gain_left = route.glide;
                self.aim = route.amount;
            }
        }
    }

    fn scale(&mut self, out: &mut Block) {
        for sample in out.iter_mut() {
            if self.gain_left > 0 {
                self.gain_left -= 1;
                // Frame counts are close enough in f32 for the line;
                // the final frame has none left and lands on the aim.
                self.gain = (-self.gain_step).mul_add(self.gain_left as f32, self.aim);
                if !self.gain.is_finite() {
                    self.gain = self.aim;
                    self.gain_left = 0;
                }
            }
            *sample = finite(*sample * self.gain);
        }
    }
}

/// One cable's delay line: its history, and where it is read from.
#[derive(Debug)]
pub struct Line {
    /// [`LINE_LEN`] frames.
    samples: Box<[f32]>,
    tap: Tap,
    shortest: u32,
    longest: u32,
    joining: bool,
    /// The line command may arrive before the joining cable's new table.
    awaiting_route: bool,
}

impl Line {
    /// A silent line at `sample_rate`, or `None` when there is no memory
    /// for its history (control side only).
    #[must_use]
    pub fn new(sample_rate: f32) -> Option<Self> {
        let shortest = ((sample_rate * SHORTEST_FADE_SECONDS).round() as u32).max(1);
        let longest = ((sample_rate * LONGEST_FADE_SECONDS).round() as u32)
            .max(u32::from(MAX_CABLE_DELAY) * FADE_PER_FRAME)
            .max(shortest);
        Some(Self {
            samples: slice_of(LINE_LEN, || 0.0)?,
            tap: Tap::EMPTY,
            shortest,
            longest,
            joining: false,
            awaiting_route: false,
        })
    }

    /// A line for `tag`'s cable already playing direct through this jack.
    /// Its first sub-block reads at zero delay, filling history before it
    /// moves to the route's delay. Allocates on the control side, returning
    /// `None` when there is no memory for the history.
    #[must_use]
    pub fn joining(sample_rate: f32, tag: u32) -> Option<Self> {
        let mut line = Self::new(sample_rate)?;
        line.tap.tag = tag;
        line.joining = true;
        line.awaiting_route = true;
        Some(line)
    }

    /// How many frames a glide of `moved` frames takes, between `shortest`
    /// and `longest`.
    fn glide_frames(moved: f64, shortest: u32, longest: u32) -> f64 {
        (moved * f64::from(FADE_PER_FRAME))
            .max(f64::from(shortest))
            .min(f64::from(longest))
    }

    /// Carry one sub-block of `source` into `out` at the route's amount.
    /// A new owner starts empty; a joining cable keeps its first block
    /// at zero delay before moving toward the route's delay. Real-time safe.
    pub fn carry(&mut self, route: &Route, source: &Block, out: &mut Block) {
        self.carry_from(route, source, out, None);
    }

    pub(super) fn carry_from(
        &mut self,
        route: &Route,
        source: &Block,
        out: &mut Block,
        direct: Option<&mut Tap>,
    ) {
        let mut tap = self.tap;
        let (shortest, longest) = (self.shortest, self.longest);
        let Some(samples) = self.samples.get_mut(..LINE_LEN) else {
            *out = [0.0; SUB_BLOCK];
            return;
        };
        if tap.tag != route.tag && !(self.awaiting_route && route.tag == 0) {
            // A new cable: nothing came through it before now, and it
            // starts at its own delay with nothing to glide from.
            samples.fill(0.0);
            self.joining = false;
            self.awaiting_route = false;
            let gain = if route.glide > 0 { 0.0 } else { route.amount };
            tap = Tap {
                tag: route.tag,
                at: f64::from(route.delay),
                target: route.delay,
                gain,
                aim: gain,
                ..Tap::EMPTY
            };
        }
        if tap.tag == route.tag {
            self.awaiting_route = false;
        }
        // First keep the direct reading and fill history, so a glide can
        // read behind it without reaching an unwritten sample.
        let route = if self.joining {
            self.joining = false;
            Route { delay: 0, ..*route }
        } else {
            *route
        };
        if let Some(previous) = direct.as_ref().filter(|previous| previous.tag == route.tag) {
            tap.gain = previous.gain;
            tap.aim = previous.aim;
            tap.gain_step = previous.gain_step;
            tap.gain_left = previous.gain_left;
        }
        tap.prepare_gain(&route);
        for (frame, sample) in source.iter().enumerate() {
            samples[(tap.write + frame) & (LINE_LEN - 1)] = *sample;
        }
        if route.gate {
            gate_block(samples, &mut tap, &route, out);
        } else {
            if route.delay != tap.target {
                // Glide from wherever the reading point is now.
                let moved = (f64::from(route.delay) - tap.at).abs();
                tap.target = route.delay;
                tap.gliding = true;
                tap.speed = moved / Self::glide_frames(moved, shortest, longest);
            }
            glide_block(samples, &mut tap, out);
        }
        tap.scale(out);
        tap.write = (tap.write + SUB_BLOCK) & (LINE_LEN - 1);
        self.tap = tap;
        if let Some(direct) = direct {
            *direct = tap;
        }
    }
}

/// The sample `delay` whole frames before `position`.
fn at(samples: &[f32], position: usize, delay: usize) -> f32 {
    samples[(position + LINE_LEN - delay) & (LINE_LEN - 1)]
}

/// The signal `delay` frames (and a fraction) before `position`, read on a
/// cubic curve through the samples either side.
fn between(samples: &[f32], position: usize, delay: f64) -> f32 {
    // At most MAX_CABLE_DELAY: exact, and the fraction is below 1.
    let whole = delay.floor();
    let t = (delay - whole) as f32;
    let whole = whole as usize;
    let near = at(samples, position, whole);
    if t <= 0.0 {
        return near;
    }
    let far = at(samples, position, whole + 1);
    let farther = at(samples, position, whole + 2);
    if whole == 0 {
        // No newer sample yet: a parabola through the three latest.
        let slope = 0.5 * 4.0_f32.mul_add(far, (-3.0_f32).mul_add(near, -farther));
        let bend = 0.5 * (-2.0_f32).mul_add(far, near + farther);
        return bend.mul_add(t, slope).mul_add(t, near);
    }
    let newer = at(samples, position, whole - 1);
    // Catmull-Rom from `near` (t = 0) to `far` (t = 1).
    let c1 = 0.5 * (far - newer);
    let c2 = 2.5_f32.mul_add(-near, newer) + 2.0_f32.mul_add(far, -0.5 * farther);
    let c3 = 0.5_f32.mul_add(farther - newer, 1.5 * (near - far));
    c3.mul_add(t, c2).mul_add(t, c1).mul_add(t, near)
}

/// Read a sub-block of audio or control voltage, gliding the reading point
/// toward its target.
fn glide_block(samples: &[f32], tap: &mut Tap, out: &mut Block) {
    let target = f64::from(tap.target);
    for (frame, value) in out.iter_mut().enumerate() {
        if tap.gliding {
            let gap = target - tap.at;
            if gap.abs() <= tap.speed {
                tap.at = target;
                tap.gliding = false;
            } else {
                tap.at += tap.speed.copysign(gap);
            }
        }
        *value = between(samples, tap.write + frame, tap.at);
    }
}

/// Where a gate's next frame comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The current reading point.
    Old,
    /// The new one, which is now current.
    New,
    /// Neither: held low.
    Low,
}

/// One frame of a gate's move from delay `current` to `target`, with the
/// reading points high or not: the next state, and where the frame comes
/// from.
fn gate_step(state: Gate, moving: bool, old_high: bool, new_high: bool) -> (Gate, Source) {
    let state = match state {
        Gate::Steady if moving => Gate::Waiting(0),
        // Moved back before it switched: nothing to wait for, and nothing
        // counted so far counts.
        Gate::Waiting(_) if !moving => Gate::Steady,
        other => other,
    };
    let muted = |held: u32| {
        // Switch where the new point is low; or where it has been high
        // past anything a line remembers (no edge to break).
        if !new_high || held as usize >= LINE_LEN {
            (Gate::Steady, Source::New)
        } else {
            (Gate::Muted(held + 1), Source::Low)
        }
    };
    match state {
        Gate::Steady => (Gate::Steady, Source::Old),
        // Between pulses: hold low until the new point is too.
        Gate::Waiting(_) if !old_high => muted(0),
        // Both high past anything a line remembers: no edge to break.
        Gate::Waiting(held) if held as usize >= LINE_LEN && new_high => (Gate::Steady, Source::New),
        Gate::Waiting(held) => (Gate::Waiting(held + 1), Source::Old),
        Gate::Muted(held) => muted(held),
    }
}

/// Read a sub-block of gate, moving to a new delay only between pulses.
fn gate_block(samples: &[f32], tap: &mut Tap, route: &Route, out: &mut Block) {
    // A gate's reading point is always whole, and at most MAX_CABLE_DELAY.
    let mut current = tap.at as usize;
    let target = usize::from(route.delay);
    for (frame, value) in out.iter_mut().enumerate() {
        let position = tap.write + frame;
        let old = at(samples, position, current);
        let new = at(samples, position, target);
        let moving = current != target || matches!(tap.gate, Gate::Muted(_));
        let (state, from) = gate_step(tap.gate, moving, old > GATE_HIGH, new > GATE_HIGH);
        tap.gate = state;
        *value = match from {
            Source::Old => old,
            Source::New => {
                current = target;
                new
            }
            Source::Low => 0.0,
        };
    }
    tap.at = current as f64;
    tap.target = route.delay;
    tap.gliding = false;
}

#[cfg(test)]
mod tests;
