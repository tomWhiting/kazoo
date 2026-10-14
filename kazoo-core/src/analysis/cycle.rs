//! A per-cycle pitch-to-voltage converter in software.
//!
//! Zero crossings of a tracking-filtered copy of the input mark when a cycle
//! ends (the latency); `McLeod`'s normalised square difference (NSDF) over the
//! unfiltered input says what the cycle was (the accuracy). Cycles are marked
//! at both rising and falling crossings, so a period is read every half
//! period, and a steady glide is carried forward between readings.

use std::f32::consts::TAU;

use super::{VOICE_MAX_HZ, VOICE_MIN_HZ};
use crate::{sanitize_sample, voice::VOICE_HOP};

#[cfg(test)]
pub(crate) mod diagnostics;
mod early;
mod level;
mod motion;
mod provisional;

pub use provisional::ProvisionalJudge;

#[cfg(test)]
pub(crate) use early::fixtures;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod hold_tests;

/// A one-pole high-pass at 30 Hz removes mic DC and handling rumble, 0.7
/// octave under `VOICE_MIN_HZ`, costing under 1 dB at 50 Hz.
const DC_HZ: f32 = 30.0;
/// When locked, the two-pole tracking low-pass sits at 3 × f: a two-octave
/// upward jump is still at half amplitude (so the Schmitt trigger still
/// fires), while the 3rd harmonic is halved. The NSDF check, not the filter,
/// decides the period.
const LP_FACTOR: f32 = 3.0;
/// The filter never closes below 100 Hz, so a 50 Hz hum's fundamental passes
/// at full strength.
const LP_MIN_HZ: f32 = 100.0;
/// A cycle mark needs the filtered signal to go below −0.2 × peak and then
/// above +0.2 × peak (or the mirror for a falling mark): well above harmonic
/// ripple on a hummed note, well below the fundamental's swing.
const SCHMITT: f32 = 0.2;
/// The peak follower (instant attack) falls with this time constant: long
/// against a 50 Hz period (20 ms), short against a phrase.
const PEAK_RELEASE_MS: f32 = 30.0;
/// Below a −60 dBFS peak no marks fire: the loudness floor of the whole
/// track, so room noise never starts a search.
const SILENCE_PEAK: f32 = 0.001;
/// A candidate is accepted at NSDF ≥ 0.8: `McLeod` and `Wyvill` pick key maxima
/// between 0.8 and 1.0; 0.8 still accepts a breathy hum.
const CLARITY_ON: f32 = 0.80;
/// A candidate's half period wins only if its NSDF is ≥ `CLARITY_ON + 0.05`,
/// so a loud 2nd harmonic does not read an octave up.
const HALF_MARGIN: f32 = 0.05;
/// With no accepted mark for 2 periods of the last pitch, the tracker
/// unlocks: one lost cycle is bridged, two are not.
const UNLOCK_PERIODS: f32 = 2.0;
/// Three milliseconds bridges a lost 128-sample block at high pitches.
const UNLOCK_MIN_MS: f32 = 3.0;
/// A held period has prior evidence, so noisy cycles can retain it at 0.70.
const HOLD_CLARITY: f32 = 0.70;
/// Fifty cents separates a held period's jitter from a different note.
const HOLD_CENTS: f32 = 50.0;
/// Locked candidates are the last 1, 2, 3 and 4 mark-to-mark intervals: up to
/// four marks per true period (a loud 2nd, 3rd or 4th harmonic) still leave
/// one candidate equal to the period.
const INTERVALS: usize = 4;
/// A buzz through a strong formant (an "ah" at 130 Hz under F1 700 Hz) can
/// mark five or six times per period while the filter is open at 4 kHz; the
/// search reaches back 16 marks, up to the 16th harmonic.
const SEARCH_MARKS: usize = 16;
/// Marks kept per crossing direction: the newest and `SEARCH_MARKS` before it.
const MARK_RING: usize = SEARCH_MARKS + 1;
/// Cycles (rising marks) a hint is tried for before it lapses.
const HINT_MARKS: u8 = 4;
/// A hint is consumed once the chosen period is within 1% of it.
const HINT_MATCH: f32 = 0.01;
/// Input is clipped to full scale. A converter never delivers more, and a
/// wild finite value (1e30) would otherwise hold the DC blocker and the peak
/// follower far from the next note for seconds.
const INPUT_LIMIT: f32 = 1.0;
/// A six-dB pick attack reopens the long-release bass filter. Short-release
/// instruments retain their marks, which already acquire their faster cycles.
const PLUCK_RISE: f32 = 2.0;
/// The NSDF window is half the candidate period, so a new note is read once
/// 1.5 of its periods exist (a reading lands within two periods at any
/// phase, since a mark comes every half period); a window never shrinks
/// under 1 ms while locked, bounded by two periods, so breath noise cannot
/// choose an octave from a single short cycle. Acquisition keeps 0.5 ms to
/// leave room for the voice frame's hop inside the onset budget.
const MIN_WINDOW_MS: f32 = 1.0;
/// A candidate interval is climbed along the NSDF to its peak for at most
/// 1/16 of its length (about a semitone): mark jitter from a filter
/// transient or a formant is reached, another key maximum is not.
const CLIMB_FRACTION: f32 = 1.0 / 16.0;
/// Readings kept to judge a glide: four readings span one and a half periods.
const TREND_READINGS: usize = 4;
/// Two readings more than 50 cents apart (half a period apart in time) are a
/// new note, not a glide: the trend starts again from the newer one.
const STEP_CENTS: f64 = 50.0;
/// A glide is carried forward only when its three successive slopes agree in
/// sign and within a factor of 2, so a step's transitional readings and a
/// steady note's jitter are never extrapolated.
const TREND_AGREEMENT: f64 = 2.0;
const CENTS_PER_OCTAVE: f64 = 1200.0;

/// Instrument-dependent range and filter times; the cycle method stays fixed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CycleSettings {
    /// Lowest fundamental; sizes the history for two periods and a margin.
    pub min_hz: f32,
    /// Highest fundamental and the unlocked tracking-filter cutoff.
    pub max_hz: f32,
    /// DC rejection below the lowest note to reduce DC and handling rumble.
    pub dc_hz: f32,
    /// Peak decay must be long compared with the lowest note's period.
    pub peak_release_ms: f32,
    /// Confirm a negative half-cycle before the normal marks can lock.
    pub early: bool,
}

impl CycleSettings {
    /// The original voice range, DC blocker and peak follower, exactly.
    pub const VOICE: Self = Self {
        min_hz: VOICE_MIN_HZ,
        max_hz: VOICE_MAX_HZ,
        dc_hz: DC_HZ,
        peak_release_ms: PEAK_RELEASE_MS,
        early: false,
    };

    pub(crate) const fn validate(self) {
        assert!(
            self.min_hz.is_finite() && self.min_hz >= 1.0,
            "invalid minimum pitch"
        );
        assert!(
            self.max_hz.is_finite() && self.max_hz > self.min_hz,
            "invalid maximum pitch"
        );
        assert!(
            self.dc_hz.is_finite() && self.dc_hz > 0.0 && self.dc_hz < self.min_hz,
            "invalid DC cutoff"
        );
        assert!(
            self.peak_release_ms.is_finite() && self.peak_release_ms > 0.0,
            "invalid peak release"
        );
    }
}

/// The newest accepted cycle's pitch and periodicity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CycleEstimate {
    /// Period in samples, fractional: the accepted reading, carried forward
    /// along a steady glide until the next one.
    pub period: f32,
    /// `rate / period`.
    pub hz: f32,
    /// Normalised autocorrelation at `period` over the reading's window, 0..=1
    /// (1 = perfectly periodic; `McLeod`'s NSDF, clamped at 0).
    pub clarity: f32,
    /// A half-cycle reading, retained until a normal mark accepts a period.
    pub provisional: bool,
}

/// The last `MARK_RING` mark times of one crossing direction.
#[derive(Debug, Clone, Copy)]
struct Marks {
    times: [f64; MARK_RING],
    newest: usize,
    count: usize,
}

impl Marks {
    const fn new() -> Self {
        Self {
            times: [0.0; MARK_RING],
            newest: 0,
            count: 0,
        }
    }

    fn push(&mut self, time: f64) {
        self.newest = (self.newest + 1) % MARK_RING;
        if let Some(slot) = self.times.get_mut(self.newest) {
            *slot = time;
        }
        self.count = (self.count + 1).min(MARK_RING);
    }

    /// Time from the `k`th earlier mark to the newest, when that mark exists.
    fn interval(&self, k: usize) -> Option<f64> {
        if k == 0 || k >= self.count {
            return None;
        }
        let newest = self.times.get(self.newest)?;
        let earlier = self.times.get((self.newest + MARK_RING - k) % MARK_RING)?;
        Some(newest - earlier)
    }

    const fn clear(&mut self) {
        self.newest = 0;
        self.count = 0;
    }
}

/// One accepted reading: its period as log2 samples, and the sample time its
/// window and lag are centred on.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Reading {
    log_period: f64,
    centre: f64,
}

/// A refined candidate: the NSDF peak, its height, and the window it used.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Peak {
    period: f32,
    clarity: f32,
    window: usize,
}

/// The candidates seen at one mark.
#[derive(Debug, Clone, Copy)]
struct Choice {
    chosen: Option<Peak>,
    best: f32,
    best_peak: Option<Peak>,
    tried: [usize; SEARCH_MARKS + 2],
    tried_count: usize,
}

impl Choice {
    const fn new() -> Self {
        Self {
            chosen: None,
            best: 0.0,
            best_peak: None,
            tried: [0; SEARCH_MARKS + 2],
            tried_count: 0,
        }
    }

    /// Record a starting lag; false when that lag was already refined here.
    fn first_try(&mut self, lag: usize) -> bool {
        let seen = self.tried.iter().take(self.tried_count).any(|&t| t == lag);
        if !seen {
            if let Some(slot) = self.tried.get_mut(self.tried_count) {
                *slot = lag;
                self.tried_count += 1;
            }
        }
        !seen
    }
}

/// A preallocated, sample-by-sample voice pitch converter.
#[derive(Debug)]
pub struct CycleTracker {
    settings: CycleSettings,
    rate: f32,
    min_period: f32,
    max_period: f32,
    /// The largest integer lag the ring can serve beside its window.
    max_lag: usize,
    min_window: usize,
    history: Box<[f32]>,
    mask: usize,
    position: usize,
    time: f64,
    dc_coefficient: f32,
    dc_previous: f32,
    dc_output: f32,
    lp_coefficient: f32,
    lp_first: f32,
    lp_second: f32,
    release: f32,
    peak: f32,
    attack_peak: f32,
    previous: f32,
    armed_rising: bool,
    armed_falling: bool,
    last_rise: f64,
    last_fall: f64,
    rising: Marks,
    falling: Marks,
    marks: u64,
    estimate: Option<CycleEstimate>,
    last_clarity: f32,
    periodic_level: f32,
    last_accepted_mark: f64,
    hint: Option<(f32, u8)>,
    trend: [Reading; TREND_READINGS],
    trend_count: usize,
    /// Along a steady glide: the per-sample factor on the published period,
    /// and the sample time after which it is no longer carried forward.
    glide: Option<(f32, f64)>,
    early: early::Early,
    acquisition: Option<f32>,
}

impl CycleTracker {
    /// Allocates the history once; nothing after this allocates.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        Self::with_settings(rate, CycleSettings::VOICE)
    }

    /// Allocate history for the selected range once, before processing audio.
    /// Invalid settings are rejected at construction.
    #[must_use]
    pub fn with_settings(rate: u32, settings: CycleSettings) -> Self {
        settings.validate();
        let rate = rate.max(1);
        let longest = lag_from(f64::from(rate) / f64::from(settings.min_hz)).unwrap_or(0);
        let length = (2 * longest + 8).next_power_of_two();
        let rate = rate as f32;
        let max_period = rate / settings.min_hz;
        Self {
            settings,
            rate,
            min_period: rate / settings.max_hz,
            max_period,
            max_lag: longest + 2,
            min_window: lag_from(f64::from(rate * MIN_WINDOW_MS / 1000.0)).unwrap_or(1),
            history: vec![0.0; length].into_boxed_slice(),
            mask: length - 1,
            position: 0,
            time: 0.0,
            dc_coefficient: (-TAU * settings.dc_hz / rate).exp(),
            dc_previous: 0.0,
            dc_output: 0.0,
            lp_coefficient: (-TAU * settings.max_hz / rate).exp(),
            lp_first: 0.0,
            lp_second: 0.0,
            release: (-1000.0 / (settings.peak_release_ms * rate)).exp(),
            peak: 0.0,
            attack_peak: 0.0,
            previous: 0.0,
            armed_rising: false,
            armed_falling: false,
            last_rise: 0.0,
            last_fall: 0.0,
            rising: Marks::new(),
            falling: Marks::new(),
            marks: 0,
            estimate: None,
            last_clarity: 0.0,
            periodic_level: 0.0,
            last_accepted_mark: 0.0,
            hint: None,
            trend: [Reading {
                log_period: 0.0,
                centre: 0.0,
            }; TREND_READINGS],
            trend_count: 0,
            glide: None,
            early: early::Early::new(),
            acquisition: None,
        }
    }

    /// One sample (sanitised and clipped to full scale first). Never
    /// allocates, locks or panics.
    pub fn push(&mut self, sample: f32) {
        let x = sanitize_sample(sample).clamp(-INPUT_LIMIT, INPUT_LIMIT);
        let attack = (x - self.dc_previous).abs();
        if self.centred_windows()
            && self.settings.peak_release_ms > PEAK_RELEASE_MS
            && self.estimate.is_some()
            && self.attack_peak >= SILENCE_PEAK
            && attack > PLUCK_RISE * self.attack_peak
        {
            self.unlock();
            self.rising.clear();
            self.falling.clear();
            self.armed_rising = false;
            self.armed_falling = false;
        }
        self.attack_peak = attack.max(self.attack_peak * self.release);
        let dc = sanitize_sample(self.dc_coefficient * (self.dc_output + x - self.dc_previous));
        self.dc_previous = x;
        self.dc_output = dc;
        self.history[self.position & self.mask] = if self.centred_windows() { x } else { dc };
        self.position = self.position.wrapping_add(1);
        let feed = 1.0 - self.lp_coefficient;
        self.lp_first = sanitize_sample(feed.mul_add(dc, self.lp_coefficient * self.lp_first));
        self.lp_second =
            sanitize_sample(feed.mul_add(self.lp_first, self.lp_coefficient * self.lp_second));
        let filtered = self.lp_second;
        self.peak = filtered.abs().max(self.peak * self.release);
        let previous = self.previous;
        let now = self.time;
        let crossing =
            || now - 1.0 + f64::from(previous) / (f64::from(previous) - f64::from(filtered));
        if previous < 0.0 && filtered >= 0.0 {
            self.last_rise = crossing();
        } else if previous >= 0.0 && filtered < 0.0 {
            self.last_fall = crossing();
        }
        self.previous = filtered;
        if self.settings.early && self.estimate.is_none() {
            self.early_push(previous, filtered);
        }
        self.schmitt(filtered);
        if self.position % VOICE_HOP == 0 {
            self.retry_acquisition();
        }
        self.carry_glide();
        if self.estimate.is_some_and(|e| {
            self.time - self.last_accepted_mark
                > f64::from((UNLOCK_PERIODS * e.period).max(UNLOCK_MIN_MS * self.rate / 1000.0))
        }) {
            self.unlock();
        }
        self.time += 1.0;
    }

    /// `push` over a block.
    pub fn process(&mut self, block: &[f32]) {
        for &sample in block {
            self.push(sample);
        }
    }

    /// The current pitch, held between cycles (carried along a steady
    /// glide); `None` when unlocked.
    #[must_use]
    pub const fn estimate(&self) -> Option<CycleEstimate> {
        self.estimate
    }

    /// Clarity of the newest cycle mark's best candidate, accepted or not;
    /// 0 before any mark.
    #[must_use]
    pub const fn last_clarity(&self) -> f32 {
        self.last_clarity
    }

    pub(crate) const fn reading_id(&self) -> u64 {
        self.last_accepted_mark.to_bits()
    }

    /// A period (samples) to try as an extra candidate until 4 more cycles
    /// are marked;
    /// when its NSDF reaches `CLARITY_ON` it is taken over every other
    /// candidate. A period outside the configured range is ignored.
    pub fn hint(&mut self, period: f32) {
        if period.is_finite() && (self.min_period..=self.max_period).contains(&period) {
            self.hint = Some((period, HINT_MARKS));
        }
    }

    /// Drop the lock now, with any hint and glide, and reopen the filter.
    pub fn unlock(&mut self) {
        self.estimate = None;
        self.periodic_level = 0.0;
        self.lp_coefficient = (-TAU * self.settings.max_hz / self.rate).exp();
        self.hint = None;
        self.trend_count = 0;
        self.glide = None;
        self.early = early::Early::new();
        self.acquisition = None;
    }

    fn retry_acquisition(&mut self) {
        // Partial-cycle evidence changes between marks. Retry only the
        // strongest refused peak and the two newest intervals, once per hop.
        if self.instrument_acquisition() && self.estimate.is_none() {
            let mut choice = Choice::new();
            if let Some(period) = self.acquisition {
                self.consider(period, &mut choice);
            }
            self.intervals(&self.rising, 1, 1, &mut choice);
            self.intervals(&self.falling, 1, 1, &mut choice);
            if let Some(peak) = choice.chosen {
                self.accept(self.half_check(peak), self.time);
            }
        }
    }

    fn instrument_acquisition(&self) -> bool {
        // The long-release bass profile's narrow search filter separates
        // useful evidence from its next mark. Short-release instruments
        // retain their original mark cadence and candidate refinement.
        self.centred_windows() && self.settings.peak_release_ms > PEAK_RELEASE_MS
    }

    /// Cycle marks seen since `new` or `reset`: one per cycle, at its rising
    /// crossing (the falling crossing between them is read too, but is the
    /// same cycle seen half a period later).
    #[must_use]
    pub const fn marks(&self) -> u64 {
        self.marks
    }

    /// Clear all history and filter state without allocating.
    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.position = 0;
        self.time = 0.0;
        self.dc_previous = 0.0;
        self.dc_output = 0.0;
        self.lp_first = 0.0;
        self.lp_second = 0.0;
        self.peak = 0.0;
        self.attack_peak = 0.0;
        self.previous = 0.0;
        self.armed_rising = false;
        self.armed_falling = false;
        self.last_rise = 0.0;
        self.last_fall = 0.0;
        self.rising.clear();
        self.falling.clear();
        self.marks = 0;
        self.last_clarity = 0.0;
        self.last_accepted_mark = 0.0;
        self.unlock();
    }

    fn schmitt(&mut self, filtered: f32) {
        if self.peak < SILENCE_PEAK {
            self.armed_rising = false;
            self.armed_falling = false;
            return;
        }
        let threshold = SCHMITT * self.peak;
        if filtered < -threshold {
            self.armed_rising = true;
            if self.armed_falling {
                self.armed_falling = false;
                self.mark(false, self.last_fall);
            }
        } else if filtered >= threshold {
            self.armed_falling = true;
            if self.armed_rising {
                self.armed_rising = false;
                self.mark(true, self.last_rise);
            }
        }
    }

    fn carry_glide(&mut self) {
        if self.centred_windows() {
            if let Some(estimate) = self.estimate {
                let period = self.instrument_period(estimate.period);
                if let Some(estimate) = self.estimate.as_mut() {
                    estimate.period = period;
                    estimate.hz = self.rate / period;
                }
            }
            return;
        }
        let Some((factor, until)) = self.glide else {
            return;
        };
        if self.time > until {
            self.glide = None;
            return;
        }
        if let Some(estimate) = self.estimate.as_mut() {
            let period = estimate.period * factor;
            if (self.min_period..=self.max_period).contains(&period) {
                estimate.period = period;
                estimate.hz = self.rate / period;
            }
        }
    }

    fn mark(&mut self, rising: bool, at: f64) {
        let marks = if rising {
            self.rising.push(at);
            self.rising
        } else {
            self.falling.push(at);
            self.falling
        };
        if rising {
            self.marks = self.marks.saturating_add(1);
        }
        if marks.count < 2 {
            return;
        }
        let mut choice = Choice::new();
        if let Some(estimate) = self.estimate {
            self.consider(estimate.period, &mut choice);
            self.intervals(&marks, 1, INTERVALS, &mut choice);
        }
        let hinted = self.hint.and_then(|(period, _)| self.refine(period));
        if let Some(peak) = hinted {
            choice.best = choice.best.max(peak.clarity);
        }
        if choice.chosen.is_none() {
            let first = if self.estimate.is_some() {
                INTERVALS + 1
            } else {
                1
            };
            self.intervals(&marks, first, SEARCH_MARKS, &mut choice);
        }
        let chosen = hinted
            .filter(|peak| peak.clarity >= CLARITY_ON)
            .or_else(|| {
                choice
                    .chosen
                    .map(|peak| self.half_check(peak))
                    .filter(|peak| {
                        self.estimate.is_none_or(|estimate| {
                            let hold = UNLOCK_MIN_MS * self.rate / 1000.0;
                            // A splice can correlate at a different lag before the
                            // held period recovers. Let the high-note hold bridge it.
                            UNLOCK_PERIODS * estimate.period >= hold
                                || (1200.0 * (peak.period / estimate.period).log2()).abs()
                                    <= HOLD_CENTS
                                || self.time - self.last_accepted_mark >= f64::from(hold)
                        })
                    })
            });
        if let Some(peak) = chosen {
            self.accept(peak, at);
        } else {
            self.last_clarity = choice.best;
            if self.instrument_acquisition() && self.estimate.is_none() {
                self.acquisition = choice.best_peak.map(|peak| peak.period);
            }
        }
        if let Some((period, remaining)) = self.hint {
            let consumed =
                chosen.is_some_and(|peak| (peak.period - period).abs() <= HINT_MATCH * period);
            self.hint = if consumed || (rising && remaining <= 1) {
                None
            } else if rising {
                Some((period, remaining - 1))
            } else {
                Some((period, remaining))
            };
        }
    }

    fn intervals(&self, marks: &Marks, first: usize, last: usize, choice: &mut Choice) {
        for k in first..=last {
            let Some(interval) = marks.interval(k) else {
                break;
            };
            let period = interval as f32;
            if period > self.max_period {
                break;
            }
            self.consider(period, choice);
        }
    }

    fn consider(&self, period: f32, choice: &mut Choice) {
        let Some(start) = lag_from(f64::from(period)) else {
            return;
        };
        if !choice.first_try(start) {
            return;
        }
        if let Some(peak) = self.refine(period) {
            choice.best = choice.best.max(peak.clarity);
            if choice
                .best_peak
                .is_none_or(|best| peak.clarity > best.clarity)
            {
                choice.best_peak = Some(peak);
            }
            let held = self.estimate.is_some_and(|estimate| {
                !estimate.provisional
                    && (1200.0 * (peak.period / estimate.period).log2()).abs() <= HOLD_CENTS
            });
            let strong_hold = self.estimate.is_some_and(|estimate| {
                !estimate.provisional
                    && lag_from(f64::from(estimate.period))
                        .is_some_and(|lag| self.nsdf(lag, self.window(lag)) >= HOLD_CLARITY)
            });
            let threshold = if held {
                HOLD_CLARITY
            } else if strong_hold {
                CLARITY_ON + 0.08
            } else {
                CLARITY_ON
            };
            let replace = choice.chosen.is_none_or(|old| {
                let old_held = self.estimate.is_some_and(|estimate| {
                    (1200.0 * (old.period / estimate.period).log2()).abs() <= HOLD_CENTS
                });
                if held == old_held {
                    peak.period < old.period
                } else {
                    held
                }
            });
            #[cfg(test)]
            if diagnostics::enabled() {
                println!(
                    "candidate at={} start={period:.4} period={:.4} nsdf={:.6} held={held} strong_hold={strong_hold} floor={threshold:.2} replace={replace} rule={}",
                    self.position,
                    peak.period,
                    peak.clarity,
                    if peak.clarity < threshold {
                        "below-floor"
                    } else if replace {
                        "select"
                    } else {
                        "held-or-shorter-wins"
                    }
                );
            }
            if peak.clarity >= threshold && replace {
                choice.chosen = Some(peak);
            }
        }
    }

    fn half_check(&self, peak: Peak) -> Peak {
        let half = peak.period / 2.0;
        if half < self.min_period {
            return peak;
        }
        self.refine(half)
            .filter(|refined| refined.clarity >= CLARITY_ON + HALF_MARGIN)
            .unwrap_or(peak)
    }

    fn accept(&mut self, peak: Peak, at: f64) {
        #[cfg(test)]
        if diagnostics::enabled() {
            println!(
                "accept at={} period={:.4} nsdf={:.6} previous={:?}",
                self.position, peak.period, peak.clarity, self.estimate
            );
        }
        self.periodic_level = self.periodic_level_at(peak);
        let reading = Reading {
            log_period: f64::from(peak.period).log2(),
            centre: (-0.5_f64).mul_add(
                count_f64(peak.window) - 1.0 + f64::from(peak.period),
                self.time,
            ),
        };
        self.remember(reading);
        let slope = self.slope();
        let ahead = self.time - reading.centre;
        let period = slope
            .map_or(reading.log_period, |slope| {
                slope.mul_add(ahead, reading.log_period)
            })
            .exp2() as f32;
        let period = if self.centred_windows() {
            self.instrument_period(peak.period)
        } else if (self.min_period..=self.max_period).contains(&period) {
            period
        } else {
            peak.period
        };
        let until = self.time + f64::from(peak.period);
        self.glide = slope.map(|slope| (slope.exp2() as f32, until));
        self.last_clarity = peak.clarity;
        self.last_accepted_mark = at;
        self.estimate = Some(CycleEstimate {
            period,
            hz: self.rate / period,
            clarity: peak.clarity,
            provisional: false,
        });
        let cutoff = (LP_FACTOR * self.rate / peak.period)
            .clamp(LP_MIN_HZ.min(self.settings.max_hz), self.settings.max_hz);
        self.lp_coefficient = (-TAU * cutoff / self.rate).exp();
    }

    /// Add a reading to the trend, newest first; a step starts it again.
    fn remember(&mut self, reading: Reading) {
        let step = self.trend_count > 0
            && self.trend.first().is_none_or(|newest| {
                (reading.log_period - newest.log_period).abs() * CENTS_PER_OCTAVE > STEP_CENTS
            });
        if step {
            self.trend_count = 0;
        }
        self.trend.copy_within(0..TREND_READINGS - 1, 1);
        if let Some(first) = self.trend.first_mut() {
            *first = reading;
        }
        self.trend_count = (self.trend_count + 1).min(TREND_READINGS);
    }

    /// The glide's slope in log2 samples per sample, when the last readings
    /// move steadily one way.
    fn slope(&self) -> Option<f64> {
        if self.trend_count < TREND_READINGS {
            return None;
        }
        let mut slopes = [0.0_f64; TREND_READINGS - 1];
        for (slope, pair) in slopes.iter_mut().zip(self.trend.windows(2)) {
            let [newer, older] = pair else {
                return None;
            };
            let span = newer.centre - older.centre;
            if span <= 0.0 {
                return None;
            }
            *slope = (newer.log_period - older.log_period) / span;
        }
        let low = slopes.iter().copied().fold(f64::INFINITY, f64::min);
        let high = slopes.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let steady = if low > 0.0 {
            high <= TREND_AGREEMENT * low
        } else if high < 0.0 {
            low >= TREND_AGREEMENT * high
        } else {
            false
        };
        let (newest, oldest) = (self.trend.first()?, self.trend.last()?);
        steady.then(|| (newest.log_period - oldest.log_period) / (newest.centre - oldest.centre))
    }

    /// The NSDF peak nearest `period`, climbed from its rounded lag for at
    /// most `CLIMB_FRACTION` of it and refined by a parabola through the
    /// peak and its neighbours. `None` when no peak is within reach or the
    /// peak leaves the configured range.
    fn refine(&self, period: f32) -> Option<Peak> {
        if !(self.min_period..=self.max_period).contains(&period) {
            return None;
        }
        let mut lag = lag_from(f64::from(period)).filter(|&lag| lag >= 2)?;
        let window = self.window(lag);
        let climb = lag_from(f64::from(period * CLIMB_FRACTION))
            .unwrap_or(0)
            .max(1);
        let mut left = self.nsdf(lag - 1, window);
        let mut centre = self.nsdf(lag, window);
        let mut right = self.nsdf(lag + 1, window);
        let acquisition = self.instrument_acquisition() && self.estimate.is_none();
        if acquisition {
            // A noisy first cycle has small local peaks between the mark's
            // interval and the periodic peak. Search the same bounded reach
            // before choosing one, rather than stopping at the first ripple.
            let start = lag.saturating_sub(climb).max(2);
            let end = (lag + climb).min(self.max_lag - 1);
            for candidate in start..=end {
                let clarity = self.nsdf(candidate, window);
                if clarity > centre {
                    lag = candidate;
                    centre = clarity;
                }
            }
            left = self.nsdf(lag - 1, window);
            right = self.nsdf(lag + 1, window);
        }
        let steps = if acquisition { 0 } else { climb };
        for _ in 0..steps {
            if right > centre && right >= left {
                lag += 1;
                if lag + 1 > self.max_lag {
                    return None;
                }
                left = centre;
                centre = right;
                right = self.nsdf(lag + 1, window);
            } else if left > centre {
                lag -= 1;
                if lag < 2 {
                    return None;
                }
                right = centre;
                centre = left;
                left = self.nsdf(lag - 1, window);
            } else {
                break;
            }
        }
        if centre < left || centre < right {
            return None;
        }
        let curvature = 2.0_f32.mul_add(-centre, left) + right;
        let offset = if curvature < -f32::EPSILON {
            (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        let period = lag as f32 + offset;
        let clarity = (0.25 * (left - right)).mul_add(-offset, centre);
        if !period.is_finite()
            || !clarity.is_finite()
            || !(self.min_period..=self.max_period).contains(&period)
        {
            return None;
        }
        Some(Peak {
            period,
            clarity: clarity.clamp(0.0, 1.0),
            window,
        })
    }

    /// The window a candidate starting at `lag` is read over: half the lag,
    /// never under half `MIN_WINDOW_MS` at acquisition or `MIN_WINDOW_MS`
    /// while locked, and while locked never longer than the
    /// held period's own window, so every candidate at a mark is compared
    /// over the same newest samples (a longer window, reaching back to
    /// louder cycles, would let a sub-harmonic outscore the period in a
    /// fade).
    fn window(&self, lag: usize) -> usize {
        let minimum = if self.estimate.is_some() {
            self.min_window
        } else {
            self.min_window / 2
        };
        let own = |lag: usize| (lag / 2).max((2 * lag).min(minimum));
        let held = self
            .estimate
            .and_then(|estimate| lag_from(f64::from(estimate.period)));
        held.map_or_else(|| own(lag), |held| own(lag).min(own(held)))
    }

    /// `McLeod`'s NSDF at `lag` over the `window` newest samples; 0 when the
    /// window holds no energy.
    fn nsdf(&self, lag: usize, window: usize) -> f32 {
        let mut product = 0.0_f64;
        let mut energy = 0.0_f64;
        let mut current_sum = 0.0_f64;
        let mut previous_sum = 0.0_f64;
        let newest = self.position.wrapping_sub(1);
        for offset in 0..window {
            let index = newest.wrapping_sub(offset);
            let x = f64::from(self.history[index & self.mask]);
            let y = f64::from(self.history[index.wrapping_sub(lag) & self.mask]);
            product = x.mul_add(y, product);
            energy = x.mul_add(x, y.mul_add(y, energy));
            if self.centred_windows() {
                current_sum += x;
                previous_sum += y;
            }
        }
        if self.centred_windows() && window > 0 {
            // Reject each window's DC without the onset transient of a
            // recursive blocker biasing a partial-cycle period reading.
            let count = window as f64;
            product -= current_sum * previous_sum / count;
            energy -= current_sum.mul_add(current_sum, previous_sum * previous_sum) / count;
        }
        if energy < 1e-12 || !energy.is_finite() || !product.is_finite() {
            0.0
        } else {
            (2.0 * product / energy).clamp(0.0, 1.0) as f32
        }
    }
}

/// A non-negative, finite sample count rounded to a lag; `None` otherwise.
fn lag_from(samples: f64) -> Option<usize> {
    let rounded = samples.round();
    if rounded.is_finite() && rounded >= 0.0 && rounded <= f64::from(u32::MAX) {
        Some(rounded as usize)
    } else {
        None
    }
}

fn count_f64(count: usize) -> f64 {
    u32::try_from(count).map_or(f64::NAN, f64::from)
}
