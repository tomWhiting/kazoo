//! Keeping a voice's formants while its pitch moves.
//!
//! The envelope is found, flattened out and put back at an analysis rate
//! between 40 and 80 kHz whatever the host's rate (44.1 or 48 kHz from
//! 88.2 kHz up, by halving), so the envelope has the same detail at every
//! rate: an all-pole envelope's resolution is its rate over its order, and
//! at 192 kHz a 24-pole envelope fitted at the host's rate would put a
//! single resonance across the whole of a voice's formants. Each side is
//! brought down through the drive family's linear-phase halfband
//! oversampler run the other way, shifted at the analysis rate by the same
//! taps as the plain sound (their delays scaled down), and brought back up
//! through its interpolating half. Above 88.2 kHz the kept path has a guard
//! of its own at the analysis rate, after the way down: the host-rate guard
//! keeps the plain sound's raised top below the host's Nyquist, but the kept
//! sound is shifted at the analysis rate, so whatever it raises past the
//! analysis rate's Nyquist would fold back as an alias. A sixteenth-order
//! lowpass (two eighth-order Butterworths) at 35% of the analysis rate over
//! the ratio takes what would land there more than 90 dB down: above the
//! analysis Nyquist the kept path carries nothing (a voice's formants lie far
//! below it). The loudness match compares the kept sound with the plain
//! sound shifted in that same band (read through the same taps at the
//! analysis rate), so a sound the kept path cannot carry is not mistaken for
//! one it has lost and lifted. The round trip delays the kept sound a
//! little (none at 44.1 or 48 kHz beyond one frame, under a millisecond
//! above), measured once in `prepare`, and the plain sound is read that
//! much further back, so the two stay in step for the switch.
//!
//! Each tap's audio is reshaped through a lattice of its own with exactly
//! the envelope it was flattened with. At a splice the new tap's lattice
//! starts from the heard tap's state: the splice has just slid the new
//! tap to where its audio matches the heard tap's, so that state is the
//! one the new tap's audio would have left. Measured on a low sine and on
//! a buzzing vowel, carrying the state over leaves the least behind
//! (an 82 Hz tone a fifth up comes out with no residue at all, against
//! -34 dB for a lattice warmed on the new tap's own recent audio and
//! -32 dB for a silent one; on the vowel the three are within a decibel,
//! the envelope's own movement from one analysis to the next being the
//! larger part there). When the kept sound is switched on there is no
//! other lattice to carry over, so the heard tap's is warmed on the audio
//! it would just have read, under the switch's crossfade.

use super::super::formant::{Formants, Lattice, MAX_ORDER, Set};
use super::super::parts::Sinc;
use super::taps::Taps;
use super::{BUTTERWORTH_8, OPEN_SHARE, Svf};
use crate::drive::oversample::{MAX_FACTOR, Oversampler};
use crate::dsp::{DelayLine, Smoothed, flush};

/// The analysis rate is the host's halved until it is under this.
const HIGHEST_ANALYSIS_RATE: f32 = 80_000.0;
/// The kept path's own guard, above 88.2 kHz: its corner as a share of the
/// analysis rate, over the ratio for a shift up.
const KEPT_SHARE: f32 = 0.35;
/// The level below which (as a power) neither sound counts in the loudness
/// match: -90 dBFS.
const LEVEL_FLOOR: f32 = 1e-9;
/// How much audio a new tap's lattice is warmed with, seconds.
const WARM: f32 = 0.002;
/// How quickly the kept sound's loudness follows the plain sound's,
/// seconds, and the most it is lifted or lowered (30 dB).
const LEVEL_TIME: f32 = 0.005;
const MOST_LIFT: f32 = 31.6;
/// How long the round trip's impulse response is searched for its peak,
/// in frames at the host's rate.
const MEASURED: usize = 4_096;

/// How many host frames make one analysis frame at `rate`.
pub fn analysis_factor(rate: f32) -> usize {
    let mut factor = 1;
    while factor < MAX_FACTOR && rate / factor as f32 >= HIGHEST_ANALYSIS_RATE {
        factor *= 2;
    }
    factor
}

/// One channel of the kept path.
#[derive(Debug, Clone, Default)]
struct Side {
    down: Oversampler,
    up: Oversampler,
    /// This analysis frame's host frames, going down; and the last
    /// analysis frame's, coming up, waiting to be played.
    gathered: [f32; MAX_FACTOR],
    queued: [f32; MAX_FACTOR],
    /// The kept path's guard at the analysis rate (two eighth-order
    /// Butterworths), used above 88.2 kHz.
    guard: [Svf; 8],
    /// The guarded input at the analysis rate, as it is (for the loudness
    /// match) and with its envelope flattened out.
    plain: DelayLine,
    flat: DelayLine,
    flatten: Lattice,
    /// The envelope put back on, one lattice for each tap.
    shapes: [Lattice; 2],
}

impl Side {
    fn clear(&mut self) {
        self.down.reset();
        self.up.reset();
        self.gathered = [0.0; MAX_FACTOR];
        self.queued = [0.0; MAX_FACTOR];
        for section in &mut self.guard {
            section.reset();
        }
        self.plain.clear();
        self.flat.clear();
        self.flatten.clear();
        for lattice in &mut self.shapes {
            lattice.clear();
        }
    }
}

/// How loud the plainly shifted and the kept sound are, and the gain that
/// matches the second to the first. Each loudness is a power through two
/// one-poles in a row: as quick to follow a change as one twice as slow,
/// with a tenth of its ripple on a low note, so the gain does not wobble
/// with the wave and add harmonics of its own.
#[derive(Debug, Clone, Copy)]
struct Level {
    plain: [f32; 2],
    kept: [f32; 2],
    coeff: f32,
    lift: Smoothed,
}

impl Default for Level {
    fn default() -> Self {
        Self {
            plain: [0.0; 2],
            kept: [0.0; 2],
            coeff: 1.0,
            lift: Smoothed::new(1.0),
        }
    }
}

/// `power` through the two one-poles `stages`; the second's output.
fn smoothed_power(stages: &mut [f32; 2], power: f32, coeff: f32) -> f32 {
    stages[0] = (power - stages[0]).mul_add(coeff, stages[0]);
    stages[1] = (stages[0] - stages[1]).mul_add(coeff, stages[1]);
    for stage in stages.iter_mut() {
        flush(stage);
    }
    stages[1]
}

impl Level {
    fn prepare(&mut self, rate: f32) {
        self.coeff = -(-1.0 / (LEVEL_TIME * rate)).exp_m1();
        self.lift.set_time(LEVEL_TIME, rate);
        self.clear();
    }

    const fn clear(&mut self) {
        self.plain = [0.0; 2];
        self.kept = [0.0; 2];
        self.lift.snap(1.0);
    }

    /// The gain for the kept sound, given a frame of each.
    fn follow(&mut self, plain: [f32; 2], kept: [f32; 2]) -> f32 {
        let power = |frame: [f32; 2]| frame[0].mul_add(frame[0], frame[1] * frame[1]);
        let plain = smoothed_power(&mut self.plain, power(plain), self.coeff);
        let kept = smoothed_power(&mut self.kept, power(kept), self.coeff);
        let wanted = ((plain + LEVEL_FLOOR) / (kept + LEVEL_FLOOR)).sqrt();
        self.lift.set(wanted.clamp(1.0 / MOST_LIFT, MOST_LIFT));
        self.lift.step()
    }
}

/// The kept path.
#[derive(Debug, Clone)]
pub struct Keeper {
    factor: usize,
    /// Which host frame of the analysis frame this is.
    phase: usize,
    /// The input's middle, for the envelope, brought down unguarded so the
    /// envelope keeps its top whatever the shift.
    heard: Oversampler,
    heard_gathered: [f32; MAX_FACTOR],
    /// Left and right, each on the heap: with their oversamplers the pair
    /// is larger than a stack frame should carry.
    sides: [Box<Side>; 2],
    formants: Formants,
    now: Set,
    heard_set: Set,
    fading_set: Set,
    /// Which of each side's `shapes` the heard tap uses.
    lead: usize,
    /// Whether a splice has happened that the next analysis frame must
    /// take up.
    spliced: bool,
    /// Whether the kept sound is being made: shaped and level-matched.
    active: bool,
    /// The playback ratio now, for warming a new tap's lattice.
    ratio: f32,
    level: Level,
    warm: usize,
    /// How much further back the plain sound is read, host frames, to stay
    /// in step with the kept sound's round trip.
    later: f32,
}

impl Default for Keeper {
    fn default() -> Self {
        Self {
            factor: 1,
            phase: 0,
            heard: Oversampler::new(),
            heard_gathered: [0.0; MAX_FACTOR],
            sides: [Box::default(), Box::default()],
            formants: Formants::default(),
            now: [0.0; MAX_ORDER],
            heard_set: [0.0; MAX_ORDER],
            fading_set: [0.0; MAX_ORDER],
            lead: 0,
            spliced: false,
            active: false,
            ratio: 1.0,
            level: Level::default(),
            warm: 0,
            later: 0.0,
        }
    }
}

impl Keeper {
    /// Size everything for `rate`, for taps that read up to `longest` host
    /// frames back. Allocates.
    pub fn prepare(&mut self, rate: f32, longest: f32) {
        self.factor = analysis_factor(rate);
        let analysis = rate / self.factor as f32;
        self.warm = ((WARM * analysis) as usize).max(1);
        let reach = longest / self.factor as f32 + self.warm as f32 + 32.0;
        self.heard.configure(self.factor, analysis, 0.0);
        for side in &mut self.sides {
            side.down.configure(self.factor, analysis, 0.0);
            side.up.configure(self.factor, analysis, 0.0);
            side.flat.resize(reach as usize);
            side.plain.resize(reach as usize);
        }
        self.formants.prepare(analysis, reach);
        self.level.prepare(analysis);
        self.later = self.measure_round_trip();
        self.clear();
    }

    /// Forget all audio.
    pub fn clear(&mut self) {
        self.phase = 0;
        self.heard.reset();
        self.heard_gathered = [0.0; MAX_FACTOR];
        for side in &mut self.sides {
            side.clear();
        }
        self.formants.clear();
        self.now = [0.0; MAX_ORDER];
        self.heard_set = [0.0; MAX_ORDER];
        self.fading_set = [0.0; MAX_ORDER];
        self.lead = 0;
        self.spliced = false;
        self.active = false;
        self.level.clear();
    }

    /// How much further back the plain sound is read.
    pub const fn later(&self) -> f32 {
        self.later
    }

    /// The taps have just spliced.
    pub const fn splice(&mut self) {
        self.spliced = true;
    }

    /// The host frames from a sample going down the first side to it
    /// coming back up, read at the analysis rate's shortest delay, with
    /// one host frame taken off (a plain read at the shortest delay is that
    /// frame itself): found by sending a click round.
    fn measure_round_trip(&mut self) -> f32 {
        let factor = self.factor;
        let side = &mut self.sides[0];
        side.clear();
        let mut loudest = (0usize, 0.0f32);
        for n in 0..MEASURED {
            let phase = n % factor;
            side.gathered[phase] = if n == 0 { 1.0 } else { 0.0 };
            let out = side.queued[phase];
            if out.abs() > loudest.1 {
                loudest = (n, out.abs());
            }
            if phase == factor - 1 {
                let down = side.down.reduce(side.gathered);
                side.queued = side.up.expand(down);
            }
        }
        side.clear();
        (loudest.0 as f32 - factor as f32 + 1.0).max(0.0)
    }

    /// One host frame: `guarded` goes in (and `middle`, the unguarded
    /// middle the envelope is found from), the kept sound comes out, made
    /// for the taps `taps` at `gains`, playing back at `ratio`, while
    /// `wanted`, and silence while not.
    pub fn frame(
        &mut self,
        guarded: [f32; 2],
        middle: f32,
        taps: &Taps,
        gains: (f32, f32),
        sinc: &Sinc,
        (ratio, wanted): (f32, bool),
    ) -> [f32; 2] {
        self.ratio = ratio;
        let phase = self.phase;
        self.heard_gathered[phase] = middle;
        let mut out = [0.0; 2];
        for ((side, sample), out) in self.sides.iter_mut().zip(guarded).zip(&mut out) {
            side.gathered[phase] = sample;
            *out = side.queued[phase];
        }
        if phase + 1 == self.factor {
            self.phase = 0;
            self.analysis_frame(taps, gains, sinc, wanted);
        } else {
            self.phase += 1;
        }
        out
    }

    /// One frame at the analysis rate.
    fn analysis_frame(&mut self, taps: &Taps, gains: (f32, f32), sinc: &Sinc, wanted: bool) {
        let order = self.formants.order();
        self.formants.push(self.heard.reduce(self.heard_gathered));
        self.formants.at(0.0, &mut self.now);
        let guarded = self.factor > 1;
        let g = (std::f32::consts::PI * (KEPT_SHARE / self.ratio).min(OPEN_SHARE)).tan();
        for side in &mut self.sides {
            let mut down = side.down.reduce(side.gathered);
            if guarded {
                down = side
                    .guard
                    .iter_mut()
                    .zip(BUTTERWORTH_8.iter().chain(&BUTTERWORTH_8))
                    .fold(down, |signal, (section, k)| section.lowpass(signal, g, *k));
            }
            side.plain.push(down);
            side.flat.push(side.flatten.flatten(down, &self.now, order));
        }
        let factor = self.factor as f32;
        let (heard, fading) = (taps.heard / factor, taps.fading / factor);
        if !wanted {
            // The flat lines and the envelope keep going, ready for the
            // switch; nothing is shaped or matched.
            if self.active {
                self.active = false;
                for side in &mut self.sides {
                    side.queued = [0.0; MAX_FACTOR];
                }
            }
            self.spliced = false;
            return;
        }
        if !self.active {
            self.active = true;
            self.level.clear();
            for side in &mut self.sides {
                side.up.reset();
                side.shapes[1 - self.lead].clear();
            }
            self.warm_lattice(self.lead, heard, sinc);
            self.spliced = false;
        }
        if self.spliced {
            // The new tap takes the other lattice; the fading tap keeps its
            // own.
            self.lead = 1 - self.lead;
            for side in &mut self.sides {
                side.shapes[self.lead] = side.shapes[1 - self.lead];
            }
            self.spliced = false;
        }
        self.formants.at(heard, &mut self.heard_set);
        if gains.1 > 0.0 {
            self.formants.at(fading, &mut self.fading_set);
        }
        let lead = self.lead;
        let mut kept = [0.0f32; 2];
        let mut plain = [0.0f32; 2];
        for ((side, kept), plain) in self.sides.iter_mut().zip(&mut kept).zip(&mut plain) {
            let read = |line: &DelayLine, delay: f32| sinc.read(line, f64::from(delay));
            *kept =
                gains.0 * side.shapes[lead].shape(read(&side.flat, heard), &self.heard_set, order);
            *plain = gains.0 * read(&side.plain, heard);
            if gains.1 > 0.0 {
                let shaped =
                    side.shapes[1 - lead].shape(read(&side.flat, fading), &self.fading_set, order);
                *kept = gains.1.mul_add(shaped, *kept);
                *plain = gains.1.mul_add(read(&side.plain, fading), *plain);
            }
        }
        let lift = self.level.follow(plain, kept);
        for (side, kept) in self.sides.iter_mut().zip(kept) {
            side.queued = side.up.expand(kept * lift);
        }
    }

    /// Start lattice `index` of each side ringing as it would for a tap at
    /// `delay` (analysis frames) that had been playing all along: run it
    /// over the audio that tap would just have read, `back` frames ago at
    /// a delay `back × (1 - ratio)` shorter, so `back × ratio` further back
    /// from now (read at a fixed delay instead, the audio would come at its
    /// own pitch, and ring a resonance fitted to it).
    fn warm_lattice(&mut self, index: usize, delay: f32, sinc: &Sinc) {
        let order = self.formants.order();
        for side in &mut self.sides {
            side.shapes[index].clear();
        }
        for back in (1..=self.warm).rev() {
            let at = (back as f32).mul_add(self.ratio, delay);
            self.formants.at(at, &mut self.fading_set);
            for side in &mut self.sides {
                let excitation = sinc.read(&side.flat, f64::from(at));
                side.shapes[index].shape(excitation, &self.fading_set, order);
            }
        }
    }
}
