//! Beat-slaved stereo tape, with bar-aligned punches and bounded ownership transfer.
pub mod feed;
#[cfg(test)]
mod tests;

use super::{Io, Module, Tick, finite};
use crate::SUB_BLOCK;
pub use feed::{
    LoopFeed, LoopGauges, LoopIn, LoopOut, LoopSnapshot, LoopState, LoopTape, TakeMark,
};
use ringbuf::{
    HeapCons, HeapProd,
    traits::{Consumer, Observer, Producer},
};
use std::sync::{Arc, atomic::Ordering};

/// Fits 23 bars at 57.5 BPM and 17 at 42.5 BPM, including unequal ambient loops.
pub const MAX_LOOP_SECONDS: f64 = 96.0;
/// Caps stereo storage at 36.9 MB even when the device runs above 48 kHz.
pub const MAX_LOOP_FRAMES: usize = 4_608_000;
/// A diagonal tape splice lasts about 8 ms, smoothing a seam without smearing a hit.
pub const SEAM_SECONDS: f32 = 0.008;
/// The wall's clock calls four beats one bar.
pub const BEATS_PER_BAR: f64 = 4.0;
/// Allows 64 full bars followed by a short bar of one to three beats.
pub const MAX_LOOP_BARS: usize = 65;
/// Sixty-four levels describe a reel or loom row without a large control message.
pub const LOOP_BINS: usize = 64;
/// Copies 64 KB per sub-block, completing a maximum loop in 563 sub-blocks.
pub const COPY_FRAMES_PER_BLOCK: usize = 8_192;
/// A five millisecond seam pulse is long enough for another module's gate input.
pub const PASS_PULSE_SECONDS: f32 = 0.005;
/// Full wear loses roughly half the amplitude over two passes.
pub const WEAR_DB_PER_PASS: f32 = 3.0;
/// Full wear lowers the top by a quarter octave each pass.
pub const WEAR_PASSES_PER_OCTAVE: f32 = 4.0;
/// Fresh tape retains the audible top before wear accumulates.
pub const WEAR_TOP_HZ: f32 = 18_000.0;
/// Worn tape retains a low midrange rather than filtering down to DC.
pub const WEAR_FLOOR_HZ: f32 = 800.0;
/// Thirty full-wear passes reach silence rather than leaving denormals.
pub const WEAR_SILENT_DB: f32 = -90.0;
/// Eight requests bound control traffic and returned buffer ownership.
pub const FEED_SLOTS: usize = 8;

/// Fixed tape geometry established at a recording bar line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Splice {
    /// Duration in song beats.
    pub beats: f64,
    /// Frames in the main loop.
    pub frames: usize,
    /// Frames in the recorded continuation.
    pub tail: usize,
    /// Song beat of the first frame.
    pub anchor: f64,
    /// Device rate at the splice.
    pub rate: f32,
}

#[derive(Debug, Clone, Copy)]
enum Rec {
    Idle,
    Armed,
    Rolling,
    PostRoll { frames_left: u32, into_tail: bool },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Head {
    pub pass: i64,
    pub phase: f64,
    pub bar: usize,
}

pub(crate) fn head(beat: f64, anchor: f64, beats: f64) -> Option<Head> {
    let rel = beat - anchor;
    if !rel.is_finite() || !beats.is_finite() || beats < 4.0 {
        return None;
    }
    // `rem_euclid` of a value a hair below a multiple of the loop rounds up
    // to the whole loop: that is the next pass's first beat. The pass is
    // taken from the same remainder, so the two always agree.
    let within = rel.rem_euclid(beats);
    let within = if within >= beats { 0.0 } else { within };
    let pass = ((rel - within) / beats).round();
    if pass < i64::MIN as f64 || pass >= i64::MAX as f64 {
        return None;
    }
    let Ok(bar) = usize::try_from((within / BEATS_PER_BAR).floor() as u64) else {
        return None;
    };
    if bar >= MAX_LOOP_BARS {
        return None;
    }
    Some(Head {
        pass: pass as i64,
        phase: within / beats,
        bar,
    })
}

#[derive(Debug)]
struct Snap {
    /// The control side's box: its tape is the destination, and it goes
    /// back whole as `Snapped`, so nothing is allocated or freed here.
    job: Box<LoopSnapshot>,
    copied: usize,
    len: usize,
}

/// A loop owns every buffer before it reaches the realtime engine.
pub struct LoopModule {
    tape: Box<[f32]>,
    capacity: usize,
    tail: usize,
    rate: f32,
    splice: Option<Splice>,
    rec: Rec,
    gain: f32,
    last_cell: Option<usize>,
    seam_carry: bool,
    bar_pass: [i64; MAX_LOOP_BARS],
    written: [bool; MAX_LOOP_BARS],
    filter: [f32; 2],
    wear_bar: Option<(usize, i64)>,
    wear_value: f32,
    wear_gain: f32,
    /// The wear gain applied now: it glides to `wear_gain` over the seam
    /// length whenever that changes, so a pass's 3 dB never steps.
    wear_level: f32,
    wear_step: f32,
    wear_left: u32,
    coefficient: f32,
    pulse_left: u32,
    peak: [f32; 2],
    bins: [u8; LOOP_BINS],
    snapshot: Option<Snap>,
    held: [Option<LoopOut>; 3],
    incoming: HeapCons<LoopIn>,
    outgoing: HeapProd<LoopOut>,
    gauges: Arc<LoopGauges>,
    previous: Option<Head>,
    song_bar: Option<f64>,
    mark: TakeMark,
    sine: [f32; 64],
    bin_threshold: [f32; 256],
    pending_take: bool,
    spliced_here: bool,
}

/// The tape a loop built at `rate` holds: `(capacity, tail)` in frames.
/// A rate outside 8 kHz to 768 kHz is taken as 48 kHz, as the module does.
#[must_use]
pub fn geometry(rate: f32) -> (usize, usize) {
    let rate = if rate.is_finite() && (8_000.0..=768_000.0).contains(&rate) {
        rate
    } else {
        48_000.0
    };
    let capacity = (MAX_LOOP_SECONDS * f64::from(rate))
        .round()
        .min(MAX_LOOP_FRAMES as f64) as usize;
    (capacity, (SEAM_SECONDS * rate).round() as usize)
}

/// The tape the control side gives a loop, whatever rate the wall runs at.
///
/// `(capacity, tail)` in frames: room for the longest take at any rate a
/// loop records at. A wall that opens its audio again at another rate
/// (KZ-D21) keeps its loops' takes, which were recorded at the old one; a
/// loop still records no more than [`MAX_LOOP_SECONDS`] at its own rate.
#[must_use]
pub const fn tape_geometry() -> (usize, usize) {
    (MAX_LOOP_FRAMES, MAX_TAIL)
}

/// The longest seam a loop records, at the highest rate one runs at.
const MAX_TAIL: usize = 6_144;

/// Whether a loop takes `splice` on a tape of `len` interleaved samples:
/// the geometry an install is checked against, for the control side to
/// check first.
#[must_use]
pub fn splice_fits(splice: Splice, len: usize) -> bool {
    splice.beats.is_finite()
        && (4.0..=259.0).contains(&splice.beats)
        && splice.anchor.is_finite()
        && splice.rate.is_finite()
        && (8_000.0..=768_000.0).contains(&splice.rate)
        && splice.frames > 0
        && splice.frames <= MAX_LOOP_FRAMES
        && splice.tail > 0
        && splice.tail <= splice.frames
        && splice.tail <= (SEAM_SECONDS * splice.rate).round() as usize
        && 2 * (splice.frames + splice.tail + 1) <= len
}

/// The 256 peak thresholds of the level bins' 60 dB scale (see
/// [`level_bin`]).
#[must_use]
pub fn bin_thresholds() -> [f32; 256] {
    std::array::from_fn(|i| 10.0_f32.powf((-60.0 + i as f32 * 60.0 / 255.0) / 20.0))
}

/// A peak's level bin: the highest threshold it reaches, 0 below -60 dB.
#[must_use]
pub fn level_bin(thresholds: &[f32; 256], peak: f32) -> u8 {
    let value = thresholds
        .partition_point(|threshold| *threshold <= peak)
        .saturating_sub(1);
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn resident(len: usize) -> Box<[f32]> {
    let mut tape = Vec::new();
    if tape.try_reserve_exact(len).is_err() {
        return Box::default();
    }
    tape.resize(len, 0.0);
    tape.into_boxed_slice()
}

impl LoopModule {
    /// Allocate and touch every page on the control side before rendering.
    #[must_use]
    pub fn new(rate: f32) -> Self {
        Self::with_feed(rate).0
    }

    fn with_feed(rate: f32) -> (Self, LoopFeed) {
        let rate = if rate.is_finite() && (8_000.0..=768_000.0).contains(&rate) {
            rate
        } else {
            48_000.0
        };
        Self::allocate(rate, geometry(rate).0)
    }

    #[cfg(test)]
    fn with_capacity(capacity: usize) -> (Self, LoopFeed) {
        Self::allocate(48_000.0, capacity)
    }

    fn allocate(rate: f32, capacity: usize) -> (Self, LoopFeed) {
        let tail = (SEAM_SECONDS * rate).round() as usize;
        let tape = if capacity == 0 {
            Box::default()
        } else {
            resident(2 * (capacity + tail + 1))
        };
        let capacity = if tape.is_empty() { 0 } else { capacity };
        let (incoming, outgoing, feed) = feed::rings();
        let module = Self {
            tape,
            capacity,
            tail,
            rate,
            splice: None,
            rec: Rec::Idle,
            gain: 0.0,
            last_cell: None,
            seam_carry: false,
            bar_pass: [i64::MIN; MAX_LOOP_BARS],
            written: [false; MAX_LOOP_BARS],
            filter: [0.0; 2],
            wear_bar: None,
            wear_value: -1.0,
            wear_gain: 1.0,
            wear_level: 1.0,
            wear_step: 0.0,
            wear_left: 0,
            coefficient: 1.0,
            pulse_left: 0,
            peak: [0.0; 2],
            bins: [0; LOOP_BINS],
            snapshot: None,
            held: std::array::from_fn(|_| None),
            incoming,
            outgoing,
            gauges: Arc::clone(&feed.gauges),
            previous: None,
            song_bar: None,
            mark: TakeMark {
                frame: 0,
                beat: 0.0,
                pass: 0,
                bars: [false; MAX_LOOP_BARS],
            },
            sine: std::array::from_fn(|i| (i as f32 / 63.0 * std::f32::consts::FRAC_PI_2).sin()),
            bin_threshold: bin_thresholds(),
            pending_take: false,
            spliced_here: false,
        };
        (module, feed)
    }

    fn free_slot(&self) -> bool {
        self.held.iter().any(Option::is_none)
    }

    fn emit(&mut self, item: LoopOut) -> Option<LoopOut> {
        match self.outgoing.try_push(item) {
            Ok(()) => None,
            Err(item) => {
                if let Some(slot) = self.held.iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(item);
                    None
                } else {
                    Some(item)
                }
            }
        }
    }

    /// Hand `item` back, into the ring or a held slot. Every caller has
    /// made sure a slot is free; were none, the item is leaked and counted,
    /// as the engine's retire ring does, never freed on the audio thread.
    fn hand_back(&mut self, item: LoopOut) {
        if let Some(item) = self.emit(item) {
            self.gauges.leaked.fetch_add(1, Ordering::Relaxed);
            std::mem::forget(item);
        }
    }

    fn flush(&mut self) {
        for slot in &mut self.held {
            if let Some(item) = slot.take() {
                match self.outgoing.try_push(item) {
                    Ok(()) => {}
                    Err(item) => {
                        *slot = Some(item);
                        break;
                    }
                }
            }
        }
        if self.pending_take && self.free_slot() {
            self.finish_take();
        }
    }

    fn finish_take(&mut self) {
        if !self.outgoing.read_is_held() {
            // Nobody holds the feed (the catalogue's builder keeps none):
            // the mark has no reader, and waiting for one would never let
            // the loop arm again. It holds no buffer, so nothing is freed.
            self.written.fill(false);
            self.pending_take = false;
            return;
        }
        self.mark.bars = self.written;
        if self.emit(LoopOut::Took(self.mark)).is_none() {
            self.written.fill(false);
            self.pending_take = false;
        } else {
            self.pending_take = true;
        }
    }

    fn requests(&mut self, tick: &Tick) {
        // Leave room for a snapshot completion and a punch mark in this block.
        if self.held.iter().filter(|slot| slot.is_none()).count() < 2 {
            return;
        }
        if self.snapshot.is_some()
            && !matches!(self.incoming.first(), Some(LoopIn::Snapshot { .. }))
        {
            return;
        }
        let Some(request) = self.incoming.try_pop() else {
            return;
        };
        let item = match request {
            LoopIn::Install {
                mut tape,
                only_if_blank,
            } => {
                let len = tape.samples.len();
                if (only_if_blank && (self.splice.is_some() || self.spliced_here))
                    || tape.splice.is_some_and(|s| !splice_fits(s, len))
                    || len > 2 * (MAX_LOOP_FRAMES + MAX_TAIL + 1)
                {
                    self.gauges.refused.fetch_add(1, Ordering::Relaxed);
                    LoopOut::Refused(LoopIn::Install {
                        tape,
                        only_if_blank,
                    })
                } else {
                    // A punch the install interrupts ends here, on the tape
                    // it was made on: its mark goes first (admission kept a
                    // slot for it), and the bars it wrote are not the new
                    // tape's.
                    if self.written.contains(&true) {
                        self.finish_take();
                    }
                    self.written.fill(false);
                    // Swapped, not moved: the box goes back holding what
                    // the loop held, to be freed on the control side.
                    std::mem::swap(&mut self.tape, &mut tape.samples);
                    std::mem::swap(&mut self.splice, &mut tape.splice);
                    std::mem::swap(&mut self.bar_pass, &mut tape.bar_pass);
                    std::mem::swap(&mut self.bins, &mut tape.bins);
                    // A tape made for any rate (see [`tape_geometry`])
                    // records no more than this loop's own seconds.
                    self.capacity = (self.tape.len() / 2)
                        .saturating_sub(self.tail + 1)
                        .min(geometry(self.rate).0);
                    self.rec = Rec::Idle;
                    self.gain = 0.0;
                    self.last_cell = None;
                    self.previous = None;
                    self.wear_bar = None;
                    self.filter = [0.0; 2];
                    self.seam_carry = false;
                    LoopOut::Retired(tape)
                }
            }
            LoopIn::Snapshot { mut into } => {
                let len = self.splice.map_or(0, |s| s.frames + s.tail + 1);
                if into.tape.len() < 2 * len || self.snapshot.is_some() {
                    self.gauges.refused.fetch_add(1, Ordering::Relaxed);
                    LoopOut::Refused(LoopIn::Snapshot { into })
                } else {
                    let pass = self
                        .splice
                        .and_then(|s| head(tick.beat, s.anchor, s.beats))
                        .map_or(0, |h| h.pass);
                    into.at = TakeMark {
                        frame: tick.frame,
                        beat: tick.beat,
                        pass,
                        bars: self.written,
                    };
                    into.splice = self.splice;
                    into.bar_pass = self.bar_pass;
                    self.snapshot = Some(Snap {
                        job: into,
                        copied: 0,
                        len,
                    });
                    return;
                }
            }
        };
        // Request admission reserved an ownership slot before popping.
        self.hand_back(item);
    }

    fn copy_snapshot(&mut self) {
        let Some(snap) = &mut self.snapshot else {
            return;
        };
        let end = (snap.copied + COPY_FRAMES_PER_BLOCK).min(snap.len);
        for k in snap.copied..end {
            if snap.job.tape[2 * k].is_nan() {
                snap.job.tape[2 * k..2 * k + 2].copy_from_slice(&self.tape[2 * k..2 * k + 2]);
            }
        }
        snap.copied = end;
        if end == snap.len && self.free_slot() {
            if let Some(snap) = self.snapshot.take() {
                self.hand_back(LoopOut::Snapped(snap.job));
            }
        }
    }

    fn splice(&mut self, tick: &Tick, io: &Io<'_>, beat: f64) {
        if self.capacity == 0 || !tick.bpm.is_finite() || tick.bpm <= 0.0 {
            return;
        }
        let mut beats = f64::from(io.knob(0)) * BEATS_PER_BAR + f64::from(io.knob(1));
        let frames_per_beat = 60.0 / tick.bpm * f64::from(self.rate);
        let mut frames = (beats * frames_per_beat).round();
        if frames > self.capacity as f64 {
            beats = ((self.capacity as f64 / frames_per_beat / BEATS_PER_BAR).floor()
                * BEATS_PER_BAR)
                .max(4.0);
            frames = (beats * frames_per_beat).round();
            self.gauges.shortened.store(true, Ordering::Relaxed);
        }
        if !frames.is_finite() || frames < 1.0 || frames > self.capacity as f64 {
            return;
        }
        let Ok(frames) = usize::try_from(frames as u64) else {
            return;
        };
        self.splice = Some(Splice {
            beats,
            frames,
            tail: self.tail.min(frames),
            anchor: beat,
            rate: self.rate,
        });
        self.spliced_here = true;
        self.bar_pass.fill(i64::MIN);
        self.bins.fill(0);
        self.rec = Rec::Rolling;
        self.gain = 0.0;
        self.last_cell = None;
        self.previous = None;
    }

    fn hermite(&self, pos: f64, frames: usize, channel: usize) -> f32 {
        let Ok(cell) = usize::try_from(pos.floor() as u64) else {
            return 0.0;
        };
        if cell >= frames {
            return 0.0;
        }
        let fraction = (pos - cell as f64) as f32;
        let before_cell = if cell == 0 { frames - 1 } else { cell - 1 };
        // Past the last cell the tape runs on into the tail (cells N and
        // N + 1, always there: the tail is at least one cell and the guard
        // follows it), what was recorded just after the end. Wrapping to
        // the loop's start instead would lean the last cells into a
        // punch-in's fade and click whenever the head reads at fractions.
        let after_cell = cell + 1;
        let next_cell = cell + 2;
        let before = self.tape[2 * before_cell + channel];
        let current = self.tape[2 * cell + channel];
        let after = self.tape[2 * after_cell + channel];
        let next = self.tape[2 * next_cell + channel];
        let slope = 0.5 * (after - before);
        let quadratic = (-0.5_f32).mul_add(
            next,
            2.0_f32.mul_add(after, (-2.5_f32).mul_add(current, before)),
        );
        let cubic = 1.5_f32.mul_add(current - after, 0.5 * (next - before));
        cubic
            .mul_add(fraction, quadratic)
            .mul_add(fraction, slope)
            .mul_add(fraction, current)
    }

    fn quarter_sine(&self, phase: f32) -> f32 {
        let pos = phase.clamp(0.0, 1.0) * 63.0;
        let Ok(i) = usize::try_from(pos.floor() as u64) else {
            return 0.0;
        };
        let next = (i + 1).min(63);
        (self.sine[next] - self.sine[i]).mul_add(pos - i as f32, self.sine[i])
    }

    fn read(&self, pos: f64, splice: Splice) -> [f32; 2] {
        let mut out = std::array::from_fn(|ch| self.hermite(pos, splice.frames, ch));
        if pos < splice.tail as f64 {
            let Ok(i) = usize::try_from(pos.floor() as u64) else {
                return [0.0; 2];
            };
            let fraction = (pos - i as f64) as f32;
            let phase = (pos / splice.tail as f64) as f32;
            let fade_in = self.quarter_sine(phase);
            let fade_out = self.quarter_sine(1.0 - phase);
            for (ch, sample) in out.iter_mut().enumerate() {
                let a = self.tape[2 * (splice.frames + i) + ch];
                let b = self.tape[2 * (splice.frames + i + 1) + ch];
                *sample = fade_in.mul_add(*sample, fade_out * (b - a).mul_add(fraction, a));
            }
        }
        out
    }

    fn wear(&mut self, h: Head, wear: f32, rate: f32, glide: usize) {
        if self.wear_bar == Some((h.bar, h.pass)) && self.wear_value.to_bits() == wear.to_bits() {
            return;
        }
        self.wear_bar = Some((h.bar, h.pass));
        self.wear_value = wear;
        let age = if self.bar_pass[h.bar] == i64::MIN {
            0
        } else {
            h.pass.saturating_sub(self.bar_pass[h.bar]).max(0)
        };
        let gain = if wear <= 0.0 || age == 0 {
            self.coefficient = 1.0;
            1.0
        } else {
            let loss = WEAR_DB_PER_PASS * wear * age as f32;
            let cutoff = (WEAR_TOP_HZ * (-age as f32 * wear / WEAR_PASSES_PER_OCTAVE).exp2())
                .max(WEAR_FLOOR_HZ);
            self.coefficient = 1.0 - (-std::f32::consts::TAU * cutoff / rate).exp();
            if loss >= -WEAR_SILENT_DB {
                0.0
            } else {
                10.0_f32.powf(-loss / 20.0)
            }
        };
        if gain.to_bits() != self.wear_gain.to_bits() {
            self.wear_gain = gain;
            // A seam's length, as the splice crossfade and the punch fades.
            self.wear_left = u32::try_from(glide.max(1)).unwrap_or(u32::MAX);
            self.wear_step = (gain - self.wear_level) / self.wear_left as f32;
        }
    }

    /// The read at `h` as worn tape plays it: filtered and quietened by its
    /// age, or untouched when it has none.
    fn worn(&mut self, mut out: [f32; 2], h: Head, wear: f32) -> [f32; 2] {
        self.wear_glide();
        let aged = self.bar_pass[h.bar] != i64::MIN && h.pass > self.bar_pass[h.bar];
        if wear > 0.0 && aged {
            for (ch, sample) in out.iter_mut().enumerate() {
                self.filter[ch] = finite(
                    self.coefficient
                        .mul_add(*sample - self.filter[ch], self.filter[ch]),
                );
                *sample = self.filter[ch] * self.wear_level;
            }
        } else {
            self.filter = out;
            // Fresh or unworn tape is read bit-exact once the gain has
            // glided home to exactly 1.
            if self.wear_level.to_bits() != 1.0_f32.to_bits() {
                for sample in &mut out {
                    *sample *= self.wear_level;
                }
            }
        }
        out
    }

    /// One frame of the wear gain's glide; it lands exactly on its target.
    fn wear_glide(&mut self) {
        if self.wear_left == 0 {
            return;
        }
        self.wear_left -= 1;
        self.wear_level = if self.wear_left == 0 {
            self.wear_gain
        } else {
            self.wear_level + self.wear_step
        };
    }

    fn write_cell(&mut self, k: usize, input: [f32; 2], keep: f32) {
        if let Some(snap) = &mut self.snapshot {
            if k >= snap.copied && k < snap.len && snap.job.tape[2 * k].is_nan() {
                snap.job.tape[2 * k..2 * k + 2].copy_from_slice(&self.tape[2 * k..2 * k + 2]);
            }
        }
        for (ch, x) in input.iter().enumerate() {
            let old = self.tape[2 * k + ch];
            let value = old.mul_add(self.gain.mul_add(-(1.0 - keep), 1.0), self.gain * x);
            self.tape[2 * k + ch] = if value.is_finite() {
                value
            } else {
                value.signum() * f32::MAX
            };
        }
    }

    fn write(&mut self, pos: f64, h: Head, splice: Splice, input: [f32; 2], keep: f32) {
        let Ok(w) = usize::try_from(pos.round() as u64) else {
            return;
        };
        let w = if w >= splice.frames { 0 } else { w };
        let (start, count) = match self.last_cell {
            Some(c) => {
                let distance = if w >= c { w - c } else { splice.frames - c + w };
                if distance == 0 {
                    return;
                }
                if distance <= 8 {
                    (if c + 1 == splice.frames { 0 } else { c + 1 }, distance)
                } else {
                    (w, 1)
                }
            }
            None => (w, 1),
        };
        for offset in 0..count {
            let k = start + offset;
            let k = if k >= splice.frames {
                k - splice.frames
            } else {
                k
            };
            // A post-roll past the seam belongs after the loop's end: it
            // writes the tail and its guard, never the loop's own cells.
            // The write head rounds to the nearest cell, so beat arithmetic
            // a hair short of the seam lets the fade start a cell late and
            // reach cell `tail`; that cell is the guard's, not bar 0's.
            let post_tail = matches!(
                self.rec,
                Rec::PostRoll {
                    into_tail: true,
                    ..
                }
            );
            let into_tail = post_tail && k < splice.tail;
            if into_tail {
                self.write_cell(splice.frames + k, input, keep);
            } else if !post_tail {
                self.write_cell(k, input, keep);
            }
            if self.seam_carry && k < splice.tail && !into_tail {
                self.write_cell(splice.frames + k, input, keep);
            }
            // The guard continues the tail's interpolation at its final frame.
            if (post_tail || self.seam_carry) && k == splice.tail {
                self.write_cell(splice.frames + splice.tail, input, keep);
            }
            if k == splice.frames - 1 && matches!(self.rec, Rec::Rolling) {
                self.seam_carry = true;
            }
            if k >= splice.tail && k < splice.frames - 1 {
                self.seam_carry = false;
            }
            // The post-roll's fade belongs to the punch that ended at the bar
            // line: it marks no bar (the next bar, or bar 0 of the next pass
            // when it runs into the tail), so the take names the bars punched
            // and wear ages every bar of one recording together.
            if matches!(self.rec, Rec::Rolling) {
                self.bar_pass[h.bar] = h.pass;
                self.written[h.bar] = true;
            }
            if post_tail {
                // The tail is not in the loop's level bins.
                continue;
            }
            let bin = (k * LOOP_BINS / splice.frames).min(LOOP_BINS - 1);
            let peak = self.tape[2 * k].abs().max(self.tape[2 * k + 1].abs());
            self.bins[bin] = self.bins[bin].max(level_bin(&self.bin_threshold, peak));
        }
        self.last_cell = Some(w);
    }

    fn advance_envelope(&mut self, splice: Splice) {
        match self.rec {
            Rec::Rolling => {
                self.gain = (self.gain + 1.0 / splice.tail as f32).min(1.0);
            }
            Rec::PostRoll {
                frames_left,
                into_tail,
            } => {
                self.gain = (self.gain - 1.0 / splice.tail as f32).max(0.0);
                if frames_left <= 1 {
                    self.rec = Rec::Idle;
                    self.gain = 0.0;
                    self.last_cell = None;
                    self.finish_take();
                } else {
                    self.rec = Rec::PostRoll {
                        frames_left: frames_left - 1,
                        into_tail,
                    };
                }
            }
            Rec::Idle | Rec::Armed => {}
        }
    }

    fn gauges(&mut self) {
        let state = if self.capacity == 0 {
            LoopState::NoTape
        } else {
            match self.rec {
                Rec::Armed => LoopState::Armed,
                Rec::Rolling => LoopState::Rolling,
                Rec::PostRoll { .. } => LoopState::PostRoll,
                Rec::Idle => {
                    if self.splice.is_some() {
                        LoopState::Holding
                    } else {
                        LoopState::Blank
                    }
                }
            }
        };
        self.gauges.state.store(state as u8, Ordering::Relaxed);
        if let Some(s) = self.splice {
            self.gauges.frames.store(s.frames as u64, Ordering::Relaxed);
            self.gauges
                .beats
                .store(s.beats.to_bits(), Ordering::Relaxed);
            self.gauges
                .anchor
                .store(s.anchor.to_bits(), Ordering::Relaxed);
            self.gauges.rate.store(s.rate.to_bits(), Ordering::Relaxed);
        }
        if self.splice.is_none() {
            self.gauges.frames.store(0, Ordering::Relaxed);
            self.gauges.beats.store(0, Ordering::Relaxed);
            self.gauges.anchor.store(0, Ordering::Relaxed);
            self.gauges
                .rate
                .store(self.rate.to_bits(), Ordering::Relaxed);
        }
        self.gauges
            .pass
            .store(self.previous.map_or(0, |head| head.pass), Ordering::Relaxed);
        for word in 0..2 {
            let mut mask = 0_u64;
            for bit in 0..64 {
                if self.written.get(word * 64 + bit).copied().unwrap_or(false) {
                    mask |= 1 << bit;
                }
            }
            self.gauges.written[word].store(mask, Ordering::Relaxed);
        }
        self.gauges
            .peak_left
            .fetch_max(self.peak[0].to_bits(), Ordering::Relaxed);
        self.gauges
            .peak_right
            .fetch_max(self.peak[1].to_bits(), Ordering::Relaxed);
        self.peak = [0.0; 2];
        for (gauge, bin) in self.gauges.bins.iter().zip(self.bins) {
            gauge.store(bin, Ordering::Relaxed);
        }
        for (gauge, pass) in self.gauges.bar_pass.iter().zip(self.bar_pass) {
            gauge.store(pass, Ordering::Relaxed);
        }
    }
}

impl Module for LoopModule {
    fn process(&mut self, tick: &Tick, io: Io<'_>) {
        self.flush();
        self.requests(tick);
        self.copy_snapshot();
        let wear = io.knob(4);
        let record = io.knob(2);
        let level = io.knob(5);
        let keep = io.knob(3);
        let level_moves = io.knob_moves(5);
        let keep_moves = io.knob_moves(3);
        for output in io.outputs.iter_mut() {
            output.fill(0.0);
        }
        if self.wear_value.to_bits() != wear.to_bits() {
            self.wear_bar = None;
        }
        for f in 0..SUB_BLOCK {
            let beat = tick.beat_at(f);
            if !beat.is_finite() {
                continue;
            }
            let song_bar = (beat / BEATS_PER_BAR).floor();
            let song_line = self.song_bar.is_some_and(|old| song_bar > old);
            self.song_bar = Some(song_bar);
            let wanted = finite(io.inputs[2][f]) > 0.5 || record >= 0.5;
            if wanted && matches!(self.rec, Rec::Idle) && !self.pending_take {
                self.rec = Rec::Armed;
            }
            if !wanted && matches!(self.rec, Rec::Armed) {
                self.rec = Rec::Idle;
            }
            if self.splice.is_none() && matches!(self.rec, Rec::Armed) && song_line {
                self.splice(tick, &io, song_bar * BEATS_PER_BAR);
            }
            let Some(splice) = self.splice else {
                continue;
            };
            let Some(h) = head(beat, splice.anchor, splice.beats) else {
                continue;
            };
            let seam = self.previous.is_none_or(|previous| h.pass != previous.pass);
            let bar_line = self
                .previous
                .is_some_and(|previous| h.bar != previous.bar || h.pass != previous.pass);
            if seam {
                self.pulse_left = (PASS_PULSE_SECONDS * tick.sample_rate).round() as u32;
            }
            if matches!(self.rec, Rec::Armed) && bar_line {
                self.rec = Rec::Rolling;
                self.gain = 0.0;
                self.last_cell = None;
            }
            if !wanted && matches!(self.rec, Rec::Rolling) && bar_line {
                self.rec = Rec::PostRoll {
                    frames_left: splice.tail as u32,
                    into_tail: seam,
                };
            }
            if !self.pending_take {
                self.mark.frame = tick.frame.saturating_add(f as u64);
                self.mark.beat = beat;
                self.mark.pass = h.pass;
            }
            let pos = h.phase * splice.frames as f64;
            let read = self.read(pos, splice).map(finite);
            self.wear(h, wear, tick.sample_rate, splice.tail);
            let out = self.worn(read, h, wear);
            let level = if level_moves { io.knob_at(5, f) } else { level };
            for (ch, sample) in out.iter().enumerate() {
                io.outputs[ch][f] = finite(*sample * level);
                self.peak[ch] = self.peak[ch].max(io.outputs[ch][f].abs());
            }
            io.outputs[2][f] = if self.pulse_left > 0 { 1.0 } else { 0.0 };
            self.pulse_left = self.pulse_left.saturating_sub(1);
            io.outputs[3][f] = h.phase as f32;
            if self.gain > 0.0 {
                let left = finite(io.inputs[0][f]).clamp(-16.0, 16.0);
                let right = if io.connected[1] {
                    finite(io.inputs[1][f]).clamp(-16.0, 16.0)
                } else {
                    left
                };
                let keep = if keep_moves { io.knob_at(3, f) } else { keep };
                self.write(pos, h, splice, [left, right], keep);
            }
            self.advance_envelope(splice);
            self.previous = Some(h);
        }
        self.gauges();
    }

    fn reset(&mut self) {
        if matches!(self.rec, Rec::PostRoll { .. }) {
            self.finish_take();
        }
        self.rec = Rec::Idle;
        self.gain = 0.0;
        self.last_cell = None;
        self.seam_carry = false;
        self.filter = [0.0; 2];
        self.wear_bar = None;
        self.wear_left = 0;
        self.wear_level = self.wear_gain;
        self.pulse_left = 0;
    }
}

/// Build a realtime loop and its control-side ownership feed.
#[must_use]
pub fn build_loop(rate: f32) -> (Box<dyn Module>, LoopFeed) {
    let (module, feed) = LoopModule::with_feed(rate);
    (Box::new(module), feed)
}

impl std::fmt::Debug for LoopModule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopModule")
            .field("splice", &self.splice)
            .field("rec", &self.rec)
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}
