//! Clock division, swing, and the sample-accurate step clock of the
//! arpeggiator.
//!
//! The clock keeps song time the way the kazoo-mix desk does: every step has
//! a fixed place on the song's beat grid (step `n` of the division, counted
//! from song beat 0), and the clock fires each step on the first sample at or
//! after that place. Step times are computed from an anchor, never
//! accumulated sample by sample, so the arpeggiator cannot drift off the grid
//! however long it runs, and [`ArpClock::seek`] lands it exactly where a
//! clock started on beat 0 would be.

use kazoo_core::ipc::follow::beats_in;

use crate::engine::{Arpeggiator, NoteEvent};

/// Slowest tempo, in BPM: the desk's range.
pub const MIN_BPM: f64 = 20.0;
/// Fastest tempo, in BPM: the desk's range.
pub const MAX_BPM: f64 = 300.0;
/// Straight time: both steps of a pair are equally long.
pub const MIN_SWING: f64 = 0.5;
/// Heaviest swing: the first step of a pair takes three quarters of it.
pub const MAX_SWING: f64 = 0.75;

/// Farthest song position the clock accepts, in beats either side of beat
/// 0: years of music at any tempo, and far inside the range where step
/// counts and beat positions stay exact.
const MAX_BEAT: f64 = 1.0e9;

/// Slack when deciding which sample a step lands on, in samples. A step
/// computed a rounding error after a sample boundary still lands on that
/// sample, so a clock that sought to a position and one that ran there from
/// beat 0 fire on the same samples.
const TICK_EPSILON: f64 = 1.0e-4;

/// Clock division relative to a quarter note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClockDivision {
    /// Whole note (1 step per 4 beats).
    Whole,
    /// Half note (1 step per 2 beats).
    Half,
    /// Quarter note (1 step per beat).
    Quarter,
    /// Eighth note (2 steps per beat).
    Eighth,
    /// Sixteenth note (4 steps per beat).
    Sixteenth,
    /// Thirty-second note (8 steps per beat).
    ThirtySecond,
    /// Eighth-note triplet (3 steps per beat).
    EighthTriplet,
    /// Sixteenth-note triplet (6 steps per beat).
    SixteenthTriplet,
}

impl ClockDivision {
    /// All divisions in display order.
    pub const ALL: [Self; 8] = [
        Self::Whole,
        Self::Half,
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
        Self::EighthTriplet,
        Self::SixteenthTriplet,
    ];

    /// Steps per beat (quarter note).
    #[must_use]
    pub const fn steps_per_beat(self) -> f64 {
        match self {
            Self::Whole => 0.25,
            Self::Half => 0.5,
            Self::Quarter => 1.0,
            Self::Eighth => 2.0,
            Self::Sixteenth => 4.0,
            Self::ThirtySecond => 8.0,
            Self::EighthTriplet => 3.0,
            Self::SixteenthTriplet => 6.0,
        }
    }

    /// Whether swing applies. Swing delays the second step of each pair,
    /// and a pair of straight steps always starts on the grid of the beat.
    /// Triplets are already a swung feel, and their pairs straddle the
    /// beat: swinging them would move the downbeat, so they play straight.
    #[must_use]
    pub const fn swings(self) -> bool {
        !matches!(self, Self::EighthTriplet | Self::SixteenthTriplet)
    }

    /// Length of one unswung step in samples at the given sample rate and
    /// tempo; `f64::INFINITY` for a rate or tempo that is not a positive
    /// number.
    #[must_use]
    pub fn period_samples(self, sample_rate: f64, bpm: f64) -> f64 {
        if !bpm.is_finite() || bpm <= 0.0 || !sample_rate.is_finite() || sample_rate <= 0.0 {
            return f64::INFINITY;
        }
        sample_rate * 60.0 / (bpm * self.steps_per_beat())
    }

    /// Human-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Whole => "1/1",
            Self::Half => "1/2",
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
            Self::EighthTriplet => "1/8T",
            Self::SixteenthTriplet => "1/16T",
        }
    }

    /// Next division (faster).
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Whole => Self::Half,
            Self::Half => Self::Quarter,
            Self::Quarter => Self::Eighth,
            Self::Eighth => Self::Sixteenth,
            Self::Sixteenth => Self::ThirtySecond,
            Self::ThirtySecond => Self::EighthTriplet,
            Self::EighthTriplet => Self::SixteenthTriplet,
            Self::SixteenthTriplet => Self::Whole,
        }
    }

    /// Previous division (slower).
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Whole => Self::SixteenthTriplet,
            Self::Half => Self::Whole,
            Self::Quarter => Self::Half,
            Self::Eighth => Self::Quarter,
            Self::Sixteenth => Self::Eighth,
            Self::ThirtySecond => Self::Sixteenth,
            Self::EighthTriplet => Self::ThirtySecond,
            Self::SixteenthTriplet => Self::EighthTriplet,
        }
    }
}

/// Song position of step `step` of `division`, in beats from beat 0.
///
/// Steps come in pairs from beat 0. The first of each pair sits on the
/// straight grid; the second is delayed by swing: at `swing` 0.5 it is
/// halfway through the pair, at 0.75 three quarters of the way. A pair
/// always spans two straight steps, so swing never changes the tempo.
/// Divisions that do not swing (triplets) ignore `swing`.
#[must_use]
pub fn step_beat(division: ClockDivision, swing: f64, step: u64) -> f64 {
    let pair_start = (step - step % 2) as f64;
    let offset = if step % 2 == 0 {
        0.0
    } else if division.swings() {
        2.0 * swing.clamp(MIN_SWING, MAX_SWING)
    } else {
        1.0
    };
    (pair_start + offset) / division.steps_per_beat()
}

/// Events returned by a single clock tick.
#[derive(Debug, Clone, Copy, Default)]
pub struct TickEvents {
    /// Note-off for the previously sounding note (its gate closed, a new
    /// step started, or the clock stopped).
    pub note_off: Option<NoteEvent>,
    /// Note-on for the new step (if a step boundary was reached).
    pub note_on: Option<NoteEvent>,
}

impl TickEvents {
    /// Whether this tick produced any events.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.note_off.is_none() && self.note_on.is_none()
    }
}

/// Sample-accurate arpeggiator clock locked to the song's beat grid.
///
/// Call [`tick`](Self::tick) once per audio sample. While running it keeps
/// song time whether or not any notes are held: steps with an empty pool
/// pass silently, so notes pressed later play on the grid, not whenever
/// they happened to be pressed. Each step that plays a note closes its gate
/// after `gate_pct` of the step.
///
/// O(1) per tick. No allocation.
#[derive(Debug, Clone)]
pub struct ArpClock {
    sample_rate: f64,
    bpm: f64,
    division: ClockDivision,
    swing: f64,
    running: bool,
    /// Song position, in beats, of tick 0.
    anchor_beat: f64,
    /// Ticks since the anchor. Tick `k` is the sample at song position
    /// `anchor_beat + beats_in(k)`.
    ticks: u64,
    /// The next step to fire, counted from song beat 0.
    next_step: u64,
    /// The tick `next_step` fires on.
    next_fire: u64,
    /// Song position at which the sounding note's gate closes, while one
    /// is open.
    gate_beat: Option<f64>,
    /// The tick the open gate closes on.
    gate_fire: u64,
}

impl ArpClock {
    /// A stopped clock at song beat 0 with the given sample rate (Hz) and
    /// tempo (BPM, clamped to the desk's range; a non-finite tempo gives
    /// 120).
    #[must_use]
    pub fn new(sample_rate: f64, bpm: f64) -> Self {
        let sample_rate = if sample_rate.is_finite() && sample_rate >= 1.0 {
            sample_rate
        } else {
            1.0
        };
        let bpm = if bpm.is_finite() {
            bpm.clamp(MIN_BPM, MAX_BPM)
        } else {
            120.0
        };
        let mut clock = Self {
            sample_rate,
            bpm,
            division: ClockDivision::Sixteenth,
            swing: MIN_SWING,
            running: false,
            anchor_beat: 0.0,
            ticks: 0,
            next_step: 0,
            next_fire: 0,
            gate_beat: None,
            gate_fire: 0,
        };
        clock.seek(0.0);
        clock
    }

    /// Sample rate in Hz.
    #[must_use]
    pub const fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Tempo in BPM.
    #[must_use]
    pub const fn bpm(&self) -> f64 {
        self.bpm
    }

    /// Clock division.
    #[must_use]
    pub const fn division(&self) -> ClockDivision {
        self.division
    }

    /// Swing (0.5 straight to 0.75 heavy).
    #[must_use]
    pub const fn swing(&self) -> f64 {
        self.swing
    }

    /// Whether the clock is running.
    #[must_use]
    pub const fn running(&self) -> bool {
        self.running
    }

    /// Song position of the next tick, in beats from beat 0.
    #[must_use]
    pub fn position(&self) -> f64 {
        // Tick counts stay far below 2^53: exact as f64.
        self.anchor_beat + beats_in(self.ticks as f64, self.bpm, self.sample_rate)
    }

    /// Play from song beat 0: the first step fires on the next tick.
    pub fn start(&mut self) {
        self.play_from(0.0);
    }

    /// Play from song position `beat` (see [`Self::seek`]).
    pub fn play_from(&mut self, beat: f64) {
        self.seek(beat);
        self.running = true;
    }

    /// Stop. The sounding note is released on the next tick; the position
    /// is kept.
    pub const fn stop(&mut self) {
        self.running = false;
    }

    /// Move to song position `beat` (in beats from the first downbeat), so
    /// the clock is exactly where a clock started on beat 0 at this tempo,
    /// division and swing would be: every later step fires on the same
    /// sample. A step that falls on `beat` (or within the sample before it,
    /// which that clock would fire on this same sample) fires on the next
    /// tick. A NaN position counts as beat 0; positions are limited to a
    /// billion beats either side of it.
    ///
    /// The sounding note keeps its gate while the gate's close lies ahead
    /// of the new position, as when the desk re-states the same timeline;
    /// otherwise it closes on the next tick. It never outlasts the next
    /// step.
    pub fn seek(&mut self, beat: f64) {
        self.anchor_beat = if beat.is_nan() {
            0.0
        } else {
            beat.clamp(-MAX_BEAT, MAX_BEAT)
        };
        self.ticks = 0;
        self.next_step = self.first_step_ahead();
        self.replan();
    }

    /// Set the tempo (clamped to the desk's range). The position is kept:
    /// steps already due stay due, later ones move to the new tempo. A
    /// non-finite value (for example a corrupt transport message) is
    /// ignored.
    pub fn set_bpm(&mut self, bpm: f64) {
        if bpm.is_finite() {
            self.reanchor();
            self.bpm = bpm.clamp(MIN_BPM, MAX_BPM);
            self.replan();
        }
    }

    /// Set the clock division. The next step is the new grid's first at or
    /// after the current position.
    pub fn set_division(&mut self, division: ClockDivision) {
        if division != self.division {
            self.reanchor();
            self.division = division;
            self.next_step = self.first_step_ahead();
            self.replan();
        }
    }

    /// Set swing (clamped to 0.5 straight - 0.75 heavy). The next step is
    /// the new grid's first at or after the current position. A non-finite
    /// value is ignored.
    pub fn set_swing(&mut self, swing: f64) {
        if swing.is_finite() {
            self.reanchor();
            self.swing = swing.clamp(MIN_SWING, MAX_SWING);
            self.next_step = self.first_step_ahead();
            self.replan();
        }
    }

    /// Process one audio sample. Returns any note events that fire on it.
    ///
    /// O(1), no allocation.
    pub fn tick(&mut self, arp: &mut Arpeggiator) -> TickEvents {
        let mut events = TickEvents::default();
        if !self.running {
            // Stopping releases whatever was sounding.
            self.gate_beat = None;
            events.note_off = arp.gate_off();
            return events;
        }

        if self.gate_beat.is_some() && self.ticks >= self.gate_fire {
            self.gate_beat = None;
            events.note_off = arp.gate_off();
        }

        if self.ticks >= self.next_fire {
            let step = self.next_step;
            // A new step always releases the previous note first.
            if let Some(off) = arp.gate_off() {
                events.note_off = Some(off);
            }
            // An empty pool plays nothing, but the step still passes.
            events.note_on = arp.step();
            let start = self.beat_of(step);
            let end = self.beat_of(step.saturating_add(1));
            self.next_step = step.saturating_add(1);
            self.next_fire = self.fire_tick(end);
            self.gate_beat = events
                .note_on
                .map(|_| (end - start).mul_add(f64::from(arp.gate_pct), start));
            self.plan_gate();
        }

        self.ticks = self.ticks.saturating_add(1);
        events
    }

    /// Song position of step `step` at the current division and swing.
    fn beat_of(&self, step: u64) -> f64 {
        step_beat(self.division, self.swing, step)
    }

    /// Samples per beat at the current tempo.
    fn samples_per_beat(&self) -> f64 {
        self.sample_rate * 60.0 / self.bpm
    }

    /// Ticks from the anchor to the sample a point at song position `beat`
    /// lands on: the first sample at or after it. Negative when it landed
    /// before the anchor.
    fn landing_tick(&self, beat: f64) -> f64 {
        (beat - self.anchor_beat)
            .mul_add(self.samples_per_beat(), -TICK_EPSILON)
            .ceil()
    }

    /// The tick a point at `beat` fires on; the next tick for one already
    /// behind.
    fn fire_tick(&self, beat: f64) -> u64 {
        let tick = self.landing_tick(beat);
        // Positions are bounded, so the tick fits a u64.
        if tick > 0.0 { tick as u64 } else { 0 }
    }

    /// The first step that lands on or after tick 0.
    fn first_step_ahead(&self) -> u64 {
        // Start from the pair holding the position one sample back: the
        // answer is at most a pair or so from there.
        let one_sample_back = self.anchor_beat - 1.0 / self.samples_per_beat();
        let mut step = if one_sample_back <= 0.0 {
            0
        } else {
            let pair = (one_sample_back * self.division.steps_per_beat() / 2.0).floor();
            // Positions are bounded, so the pair index fits a u64.
            (pair as u64).saturating_mul(2)
        };
        // Rounding in the estimate is settled against the exact rule.
        while step > 0 && self.lands_ahead(step - 1) {
            step -= 1;
        }
        while !self.lands_ahead(step) {
            step = step.saturating_add(1);
        }
        step
    }

    /// Whether step `step` lands on or after tick 0.
    fn lands_ahead(&self, step: u64) -> bool {
        self.landing_tick(self.beat_of(step)) >= 0.0
    }

    /// Make the current position the anchor, so a new tempo, division or
    /// swing applies from here on.
    fn reanchor(&mut self) {
        self.anchor_beat = self.position();
        self.ticks = 0;
    }

    /// Work out, from the anchor, the ticks the next step and the open
    /// gate fire on.
    fn replan(&mut self) {
        self.next_fire = self.fire_tick(self.beat_of(self.next_step));
        self.plan_gate();
    }

    /// The tick the open gate closes on: never after the next step, which
    /// releases the note anyway.
    fn plan_gate(&mut self) {
        if let Some(beat) = self.gate_beat {
            self.gate_fire = self.fire_tick(beat).max(self.ticks).min(self.next_fire);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ArpMode;

    /// An arpeggiator holding C4, E4 and G4.
    fn chord() -> Arpeggiator {
        let mut arp = Arpeggiator::new();
        for note in [60, 64, 67] {
            arp.note_on(note, 100);
        }
        arp
    }

    fn clock(sample_rate: f64, bpm: f64, division: ClockDivision, swing: f64) -> ArpClock {
        let mut clock = ArpClock::new(sample_rate, bpm);
        clock.set_division(division);
        clock.set_swing(swing);
        clock
    }

    /// Samples on which note-ons and note-offs fire over `samples` ticks,
    /// numbered from `offset`.
    fn run(
        clock: &mut ArpClock,
        arp: &mut Arpeggiator,
        samples: u64,
        offset: u64,
    ) -> (Vec<u64>, Vec<u64>) {
        let mut ons = Vec::new();
        let mut offs = Vec::new();
        for sample in 0..samples {
            let events = clock.tick(arp);
            if events.note_on.is_some() {
                ons.push(sample + offset);
            }
            if events.note_off.is_some() {
                offs.push(sample + offset);
            }
        }
        (ons, offs)
    }

    #[test]
    fn division_period_at_120bpm() {
        let p = ClockDivision::Quarter.period_samples(44_100.0, 120.0);
        assert!((p - 22_050.0).abs() < 1e-9, "quarter at 120 BPM: {p}");
        let p16 = ClockDivision::Sixteenth.period_samples(44_100.0, 120.0);
        assert!((p16 - 5_512.5).abs() < 1e-9, "sixteenth at 120 BPM: {p16}");
        let p8t = ClockDivision::EighthTriplet.period_samples(48_000.0, 120.0);
        assert!((p8t - 8_000.0).abs() < 1e-9, "eighth triplet: {p8t}");
    }

    #[test]
    fn division_period_zero_bpm_never_fires() {
        let period = ClockDivision::Quarter.period_samples(44_100.0, 0.0);
        assert!(period.is_infinite(), "period {period}");
    }

    #[test]
    fn swing_delays_the_off_beat_and_keeps_the_pair() {
        let d = ClockDivision::Sixteenth;
        assert!((step_beat(d, 0.5, 1) - 0.25).abs() < f64::EPSILON);
        assert!((step_beat(d, 0.75, 1) - 0.375).abs() < f64::EPSILON);
        // The pair still spans two straight steps.
        assert!((step_beat(d, 0.75, 2) - 0.5).abs() < f64::EPSILON);
        // Out-of-range swing is held to the range.
        assert!((step_beat(d, 0.1, 1) - 0.25).abs() < f64::EPSILON);
        assert!((step_beat(d, 2.0, 1) - 0.375).abs() < f64::EPSILON);
    }

    #[test]
    fn triplets_play_straight_so_the_beat_stays_on_the_beat() {
        for swing in [0.5, 0.6, 0.75] {
            for step in 0..12 {
                let beat = step_beat(ClockDivision::EighthTriplet, swing, step);
                assert!((beat - step as f64 / 3.0).abs() < 1e-12, "step {step}");
            }
        }
        assert!(!ClockDivision::SixteenthTriplet.swings());
        assert!(ClockDivision::Quarter.swings());
    }

    #[test]
    fn non_finite_tempo_and_swing_are_ignored() {
        let mut arp = chord();
        let mut clock = ArpClock::new(44_100.0, 120.0);
        clock.set_swing(0.6);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            clock.set_bpm(bad);
            clock.set_swing(bad);
        }
        assert!((clock.bpm() - 120.0).abs() < f64::EPSILON);
        assert!((clock.swing() - 0.6).abs() < f64::EPSILON);
        clock.start();
        let (ons, _) = run(&mut clock, &mut arp, 20_000, 0);
        assert!(ons.len() >= 3, "clock must keep stepping, got {ons:?}");
    }

    #[test]
    fn tempo_is_held_to_the_desks_range() {
        let mut clock = ArpClock::new(48_000.0, 5.0);
        assert!((clock.bpm() - MIN_BPM).abs() < f64::EPSILON);
        clock.set_bpm(900.0);
        assert!((clock.bpm() - MAX_BPM).abs() < f64::EPSILON);
        assert!((ArpClock::new(48_000.0, f64::NAN).bpm() - 120.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_downbeat_fires_on_the_first_tick() {
        let mut arp = chord();
        let mut clock = ArpClock::new(48_000.0, 120.0);
        clock.start();
        // 120 BPM sixteenths at 48 kHz: 6 000 samples apart.
        let (ons, _) = run(&mut clock, &mut arp, 13_000, 0);
        assert_eq!(ons, [0, 6_000, 12_000]);
    }

    #[test]
    fn steps_never_drift_off_the_grid() {
        // 120 BPM sixteenths at 44.1 kHz are 5 512.5 samples apart: a clock
        // that rounds each step to whole samples gains half a sample a
        // step. Step n must land on the first sample at or after n * 5512.5.
        let mut arp = chord();
        let mut clock = ArpClock::new(44_100.0, 120.0);
        clock.start();
        let steps = 2_000_u64;
        let (ons, _) = run(&mut clock, &mut arp, steps * 5_513, 0);
        let expected: Vec<u64> = (0..=steps)
            .map(|n| (n * 11_025).div_ceil(2))
            .take_while(|&sample| sample < steps * 5_513)
            .collect();
        assert_eq!(ons, expected);

        // An awkward tempo: 97 BPM sixteenths at 48 kHz are
        // 2 880 000 / 388 samples apart, exactly.
        let mut clock = ArpClock::new(48_000.0, 97.0);
        clock.start();
        let (ons, _) = run(&mut clock, &mut arp, 5_000_000, 0);
        let expected: Vec<u64> = (0_u64..=1_000)
            .map(|n| (n * 2_880_000).div_ceil(388))
            .take_while(|&sample| sample < 5_000_000)
            .collect();
        assert_eq!(ons, expected);
    }

    #[test]
    fn seeking_lands_on_the_grid_a_clock_from_beat_zero_keeps() {
        for division in [
            ClockDivision::Sixteenth,
            ClockDivision::Eighth,
            ClockDivision::EighthTriplet,
            ClockDivision::SixteenthTriplet,
            ClockDivision::ThirtySecond,
        ] {
            for swing in [0.5, 0.58, 0.666] {
                for bpm in [97.0, 120.0, 173.5] {
                    let mut arp = chord();
                    let mut from_start = clock(48_000.0, bpm, division, swing);
                    from_start.start();
                    let (whole_on, whole_off) = run(&mut from_start, &mut arp, 200_000, 0);

                    for join_at in [1_u64, 5_999, 6_000, 77_777, 123_457] {
                        let mut arp = chord();
                        let mut joined = clock(48_000.0, bpm, division, swing);
                        joined.play_from(beats_in(join_at as f64, bpm, 48_000.0));
                        let (on, off) = run(&mut joined, &mut arp, 200_000 - join_at, join_at);
                        let later_on: Vec<u64> =
                            whole_on.iter().copied().filter(|&s| s >= join_at).collect();
                        assert_eq!(
                            on, later_on,
                            "{division:?} swing {swing} bpm {bpm} at {join_at}"
                        );
                        // Gates close where the running clock's did (the
                        // joined clock plays nothing before its first step).
                        let first = later_on.first().copied().unwrap_or(u64::MAX);
                        let later_off: Vec<u64> =
                            whole_off.iter().copied().filter(|&s| s > first).collect();
                        let off: Vec<u64> = off.into_iter().filter(|&s| s > first).collect();
                        assert_eq!(
                            off, later_off,
                            "{division:?} swing {swing} bpm {bpm} at {join_at}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_step_on_the_position_fires_at_once() {
        let mut arp = chord();
        let mut clock = ArpClock::new(48_000.0, 120.0);
        clock.play_from(1.0);
        assert!(clock.tick(&mut arp).note_on.is_some());
        clock.play_from(f64::NAN);
        assert!((clock.position() - 0.0).abs() < f64::EPSILON);
        assert!(clock.tick(&mut arp).note_on.is_some());
        // Half a sample past a step: that sample is the step's.
        clock.play_from(1.0 + beats_in(0.5, 120.0, 48_000.0));
        assert!(clock.tick(&mut arp).note_on.is_some());
    }

    #[test]
    fn a_count_in_waits_for_the_downbeat() {
        let mut arp = chord();
        let mut clock = ArpClock::new(48_000.0, 120.0);
        // Half a beat before the song starts: 12 000 samples at 120 BPM.
        clock.play_from(-0.5);
        let (ons, _) = run(&mut clock, &mut arp, 13_000, 0);
        assert_eq!(ons, [12_000]);
    }

    #[test]
    fn swing_delays_the_second_step_of_each_pair() {
        let mut arp = chord();
        let mut clock = clock(48_000.0, 120.0, ClockDivision::Sixteenth, 0.75);
        clock.start();
        let (ons, _) = run(&mut clock, &mut arp, 24_001, 0);
        // Pairs of 12 000 samples, split 9 000 / 3 000.
        assert_eq!(ons, [0, 9_000, 12_000, 21_000, 24_000]);
    }

    #[test]
    fn keeps_time_with_no_notes_held() {
        let mut arp = Arpeggiator::new();
        let mut clock = ArpClock::new(48_000.0, 120.0);
        clock.start();
        // Two and a half steps with nothing held.
        let (ons, offs) = run(&mut clock, &mut arp, 15_000, 0);
        assert!(ons.is_empty() && offs.is_empty());
        // A note pressed mid-step plays on the next step of the grid.
        arp.note_on(60, 100);
        let (ons, _) = run(&mut clock, &mut arp, 10_000, 15_000);
        assert_eq!(ons, [18_000, 24_000]);
    }

    #[test]
    fn gates_close_after_their_share_of_the_step() {
        let mut arp = chord();
        arp.set_gate_pct(0.5);
        let mut clock = clock(48_000.0, 120.0, ClockDivision::Sixteenth, 0.75);
        clock.start();
        let (ons, offs) = run(&mut clock, &mut arp, 12_001, 0);
        assert_eq!(ons, [0, 9_000, 12_000]);
        // The long step's gate closes after 4 500, the short one's after
        // 1 500, and the next step releases nothing more.
        assert_eq!(offs, [4_500, 10_500]);

        // A full gate is released by the next step, on the same sample.
        let mut arp = chord();
        arp.set_gate_pct(1.0);
        let mut clock = ArpClock::new(48_000.0, 120.0);
        clock.start();
        let (ons, offs) = run(&mut clock, &mut arp, 12_001, 0);
        assert_eq!(ons, [0, 6_000, 12_000]);
        assert_eq!(offs, [6_000, 12_000]);
    }

    #[test]
    fn a_tempo_change_keeps_the_beat_and_moves_later_steps() {
        let mut arp = chord();
        let mut clock = ArpClock::new(48_000.0, 120.0);
        clock.start();
        let (ons, _) = run(&mut clock, &mut arp, 3_000, 0);
        assert_eq!(ons, [0]);
        // Half a step in, the tempo doubles: the rest of the step takes
        // half as long.
        clock.set_bpm(240.0);
        let (ons, _) = run(&mut clock, &mut arp, 7_000, 3_000);
        assert_eq!(ons, [4_500, 7_500]);
        // 3 000 samples at 120 BPM, then 7 000 at 240 (12 000 a beat).
        assert!((clock.position() - (0.125 + 7_000.0 / 12_000.0)).abs() < 1e-12);
    }

    #[test]
    fn changing_division_moves_to_the_new_grid() {
        let mut arp = chord();
        let mut clock = ArpClock::new(48_000.0, 120.0);
        clock.start();
        let (ons, _) = run(&mut clock, &mut arp, 7_000, 0);
        assert_eq!(ons, [0, 6_000]);
        // Quarter notes from here: the next is beat 1, at 24 000.
        clock.set_division(ClockDivision::Quarter);
        let (ons, _) = run(&mut clock, &mut arp, 42_000, 7_000);
        assert_eq!(ons, [24_000, 48_000]);
    }

    #[test]
    fn stopping_releases_the_note_and_keeps_the_place() {
        let mut arp = chord();
        let mut clock = ArpClock::new(48_000.0, 120.0);
        clock.start();
        assert!(clock.tick(&mut arp).note_on.is_some());
        clock.stop();
        assert!(!clock.running());
        let events = clock.tick(&mut arp);
        assert!(events.note_off.is_some() && events.note_on.is_none());
        for _ in 0..50_000 {
            assert!(clock.tick(&mut arp).is_empty());
        }
        assert!((clock.position() - beats_in(1.0, 120.0, 48_000.0)).abs() < 1e-12);
    }

    #[test]
    fn a_resent_position_keeps_the_sounding_gate() {
        let mut arp = chord();
        let mut clock = ArpClock::new(48_000.0, 120.0);
        clock.start();
        let (_, offs) = run(&mut clock, &mut arp, 1_000, 0);
        assert!(offs.is_empty());
        // The desk re-states the same timeline: the gate (at 4 500) holds.
        clock.play_from(clock.position());
        let (ons, offs) = run(&mut clock, &mut arp, 5_001, 1_000);
        assert_eq!(ons, [6_000]);
        assert_eq!(offs, [4_500]);
    }

    #[test]
    fn clock_cycles_through_notes() {
        let mut arp = chord();
        let mut clock = ArpClock::new(44_100.0, 120.0);
        clock.start();
        let mut notes = Vec::new();
        for _ in 0..500_000 {
            if let Some(NoteEvent::NoteOn { midi_note, .. }) = clock.tick(&mut arp).note_on {
                notes.push(midi_note);
                if notes.len() >= 6 {
                    break;
                }
            }
        }
        assert_eq!(notes, [60, 64, 67, 60, 64, 67]);
    }

    #[test]
    fn clock_stopped_emits_nothing() {
        let mut arp = chord();
        let mut clock = ArpClock::new(44_100.0, 120.0);
        for _ in 0..1000 {
            assert!(clock.tick(&mut arp).is_empty());
        }
    }

    #[test]
    fn division_labels_unique() {
        let labels: Vec<_> = ClockDivision::ALL.iter().map(|d| d.label()).collect();
        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn division_next_and_prev_cycle() {
        let mut d = ClockDivision::Whole;
        for _ in 0..8 {
            d = d.next();
        }
        assert_eq!(d, ClockDivision::Whole);
        for _ in 0..8 {
            d = d.prev();
        }
        assert_eq!(d, ClockDivision::Whole);
    }

    #[test]
    fn clock_random_mode_no_repeat() {
        let mut arp = chord();
        arp.set_mode(ArpMode::Random);
        arp.no_repeat = true;
        let mut clock = clock(44_100.0, 120.0, ClockDivision::ThirtySecond, 0.5);
        clock.start();
        let mut notes = Vec::new();
        for _ in 0..500_000 {
            if let Some(NoteEvent::NoteOn { midi_note, .. }) = clock.tick(&mut arp).note_on {
                notes.push(midi_note);
                if notes.len() >= 20 {
                    break;
                }
            }
        }
        for window in notes.windows(2) {
            assert_ne!(
                window[0], window[1],
                "consecutive repeat in random: {notes:?}"
            );
        }
    }
}
