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
//! Every cable has a line of its own, written every time its signal passes,
//! so its history is always there. When a cable's delay changes (an effect
//! added or taken away upstream), the signal never breaks:
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
//! Everything is allocated when the engine is built.

use super::tables::{MAX_CABLE_DELAY, Route};
use crate::dsp::{Block, GATE_HIGH, finite};
use crate::{MAX_CABLES, SUB_BLOCK};

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
struct Tap {
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
}

impl Tap {
    const EMPTY: Self = Self {
        tag: 0,
        write: 0,
        at: 0.0,
        target: 0,
        gliding: false,
        speed: 0.0,
        gate: Gate::Steady,
    };
}

/// Every cable's delay line.
#[derive(Debug)]
pub struct Lines {
    samples: Box<[f32]>,
    taps: Box<[Tap]>,
    /// The shortest and longest glide, in frames.
    shortest: u32,
    longest: u32,
}

impl Lines {
    /// Lines for every cable at `sample_rate` (this allocates).
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        // Rates are at most a few hundred kHz, so both are well inside
        // u32 (and a nonsense rate saturates to 0, then 1).
        let shortest = ((sample_rate * SHORTEST_FADE_SECONDS).round() as u32).max(1);
        let longest = ((sample_rate * LONGEST_FADE_SECONDS).round() as u32)
            .max(u32::from(MAX_CABLE_DELAY) * FADE_PER_FRAME)
            .max(shortest);
        Self {
            samples: vec![0.0; MAX_CABLES * LINE_LEN].into_boxed_slice(),
            taps: vec![Tap::EMPTY; MAX_CABLES].into_boxed_slice(),
            shortest,
            longest,
        }
    }

    /// How many frames a glide of `moved` frames takes, between `shortest`
    /// and `longest`.
    fn glide_frames(moved: f64, shortest: u32, longest: u32) -> f64 {
        (moved * f64::from(FADE_PER_FRAME))
            .max(f64::from(shortest))
            .min(f64::from(longest))
    }

    /// Pass one sub-block of `source` through `route`'s line into `out`,
    /// scaled by the cable's amount. Real-time safe.
    pub fn carry(&mut self, route: &Route, source: &Block, out: &mut Block) {
        let line = usize::from(route.line);
        let Some(tap) = self.taps.get(line).copied() else {
            *out = [0.0; SUB_BLOCK];
            return;
        };
        let mut tap = tap;
        let (shortest, longest) = (self.shortest, self.longest);
        let Some(samples) = self.samples.get_mut(line * LINE_LEN..(line + 1) * LINE_LEN) else {
            *out = [0.0; SUB_BLOCK];
            return;
        };
        if tap.tag != route.tag {
            // A new cable: nothing came through it before now, and it
            // starts at its own delay with nothing to glide from.
            samples.fill(0.0);
            tap = Tap {
                tag: route.tag,
                at: f64::from(route.delay),
                target: route.delay,
                ..Tap::EMPTY
            };
        }
        for (frame, sample) in source.iter().enumerate() {
            samples[(tap.write + frame) & (LINE_LEN - 1)] = *sample;
        }
        if route.gate {
            gate_block(samples, &mut tap, route, out);
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
        for sample in out.iter_mut() {
            *sample = finite(*sample * route.amount);
        }
        tap.write = (tap.write + SUB_BLOCK) & (LINE_LEN - 1);
        if let Some(slot) = self.taps.get_mut(line) {
            *slot = tap;
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
