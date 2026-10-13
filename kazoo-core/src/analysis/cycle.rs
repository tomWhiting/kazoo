//! Cycle marks cue bounded, one-period autocorrelation searches.

use std::f32::consts::TAU;

use super::{VOICE_MAX_HZ, VOICE_MIN_HZ};
use crate::sanitize_sample;

#[cfg(test)]
mod tests;

/// A 30 Hz DC blocker removes mic DC and handling rumble, 0.7 octave below
/// the voice floor and costing under 1 dB at 50 Hz.
const DC_HZ: f32 = 30.0;
/// Three times the pitch passes an upward two-octave jump at half amplitude
/// and halves the third harmonic; autocorrelation decides the period.
const LP_FACTOR: f32 = 3.0;
/// The marking filter stays open enough to pass a 50 Hz hum's fundamental.
const LP_MIN_HZ: f32 = 100.0;
/// Twenty percent of peak exceeds harmonic ripple but preserves cycle swings.
const SCHMITT: f32 = 0.2;
/// Peak release is long against a 20 ms low-note cycle, short against a phrase.
const PEAK_RELEASE_MS: f32 = 30.0;
/// Below the track's -60 dBFS peak floor, room noise cannot start a search.
const SILENCE_PEAK: f32 = 0.001;
/// `McLeod`'s key maxima span 0.8 to 1.0; this also accepts a breathy hum.
const CLARITY_ON: f32 = 0.80;
/// Extra evidence at half the period prevents a loud second harmonic winning.
const HALF_MARGIN: f32 = 0.05;
/// Bridge one lost cycle, but unlock after two periods without acceptance.
const UNLOCK_PERIODS: f32 = 2.0;
/// Four intervals cover up to four harmonic marks per fundamental period.
const INTERVALS: usize = 4;
/// An open filter can mark strong formants many times per period; sixteen
/// intervals include the fundamental through the sixteenth harmonic.
const SEARCH_MARKS: usize = 16;
/// Unlocked, preserve every possible voice pitch without tracking delay.
const UNLOCKED_CUTOFF: f32 = VOICE_MAX_HZ;

/// The newest accepted cycle's pitch and periodicity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CycleEstimate {
    /// Period in samples, fractional.
    pub period: f32,
    /// Sample rate divided by period.
    pub hz: f32,
    /// `McLeod`'s normalised square difference over one period, clamped to 0..=1.
    pub clarity: f32,
}

/// A preallocated, sample-by-sample voice pitch converter.
#[derive(Debug)]
pub struct CycleTracker {
    rate: f32,
    min_period: f32,
    max_period: f32,
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
    previous: f32,
    armed: bool,
    last_zero: f64,
    mark_times: [f64; SEARCH_MARKS + 1],
    mark_position: usize,
    mark_count: usize,
    marks: u64,
    estimate: Option<CycleEstimate>,
    last_clarity: f32,
    last_accepted_mark: f64,
    hint: Option<(f32, u8)>,
}

impl CycleTracker {
    /// Allocate history once. No subsequent processing operation allocates.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let longest = (f64::from(rate) / f64::from(VOICE_MIN_HZ)).ceil() as usize;
        let length = (2 * longest + 8).next_power_of_two();
        let rate = rate as f32;
        Self {
            rate,
            min_period: rate / VOICE_MAX_HZ,
            max_period: rate / VOICE_MIN_HZ,
            history: vec![0.0; length].into_boxed_slice(),
            mask: length - 1,
            position: 0,
            time: 0.0,
            dc_coefficient: (-TAU * DC_HZ / rate).exp(),
            dc_previous: 0.0,
            dc_output: 0.0,
            lp_coefficient: (-TAU * UNLOCKED_CUTOFF / rate).exp(),
            lp_first: 0.0,
            lp_second: 0.0,
            release: (-1000.0 / (PEAK_RELEASE_MS * rate)).exp(),
            peak: 0.0,
            previous: 0.0,
            armed: false,
            last_zero: 0.0,
            mark_times: [0.0; SEARCH_MARKS + 1],
            mark_position: 0,
            mark_count: 0,
            marks: 0,
            estimate: None,
            last_clarity: 0.0,
            last_accepted_mark: 0.0,
            hint: None,
        }
    }

    /// Sanitize and consume one sample without allocation, locking or I/O.
    pub fn push(&mut self, sample: f32) {
        let x = sanitize_sample(sample);
        let dc = sanitize_sample(self.dc_coefficient * (self.dc_output + x - self.dc_previous));
        self.dc_previous = x;
        self.dc_output = dc;
        self.history[self.position & self.mask] = dc;
        self.position = self.position.wrapping_add(1);
        let feed = 1.0 - self.lp_coefficient;
        self.lp_first = sanitize_sample(feed.mul_add(dc, self.lp_coefficient * self.lp_first));
        self.lp_second =
            sanitize_sample(feed.mul_add(self.lp_first, self.lp_coefficient * self.lp_second));
        let filtered = self.lp_second;
        self.peak = filtered.abs().max(self.peak * self.release);
        if self.previous < 0.0 && filtered >= 0.0 {
            self.last_zero = self.time - 1.0
                + f64::from(self.previous) / (f64::from(self.previous) - f64::from(filtered));
        }
        if self.peak >= SILENCE_PEAK {
            let threshold = SCHMITT * self.peak;
            if filtered < -threshold {
                self.armed = true;
            }
            if self.armed && filtered >= threshold {
                self.armed = false;
                self.mark(self.last_zero);
            }
        } else {
            self.armed = false;
        }
        self.previous = filtered;
        if self.estimate.is_some_and(|e| {
            self.time - self.last_accepted_mark > f64::from(UNLOCK_PERIODS * e.period)
        }) {
            self.unlock();
        }
        self.time += 1.0;
    }

    /// Consume a block using the same sample path.
    pub fn process(&mut self, block: &[f32]) {
        for &sample in block {
            self.push(sample);
        }
    }

    /// Current pitch, held between marks; absent when unlocked.
    #[must_use]
    pub const fn estimate(&self) -> Option<CycleEstimate> {
        self.estimate
    }

    /// Best clarity at the newest mark, whether or not it was accepted.
    #[must_use]
    pub const fn last_clarity(&self) -> f32 {
        self.last_clarity
    }

    /// Add a judge-supplied period candidate for the next four marks.
    pub fn hint(&mut self, period: f32) {
        if period.is_finite() && (self.min_period..=self.max_period).contains(&period) {
            self.hint = Some((period, 4));
        }
    }

    /// Drop the lock and hint, restoring the open marking filter.
    pub fn unlock(&mut self) {
        self.estimate = None;
        self.lp_coefficient = (-TAU * UNLOCKED_CUTOFF / self.rate).exp();
        self.hint = None;
    }

    /// Cycle marks since construction or reset.
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
        self.previous = 0.0;
        self.armed = false;
        self.last_zero = 0.0;
        self.mark_times.fill(0.0);
        self.mark_position = 0;
        self.mark_count = 0;
        self.marks = 0;
        self.last_clarity = 0.0;
        self.last_accepted_mark = 0.0;
        self.unlock();
    }

    fn mark(&mut self, mark: f64) {
        self.mark_times[self.mark_position] = mark;
        self.mark_count = (self.mark_count + 1).min(SEARCH_MARKS + 1);
        self.marks = self.marks.saturating_add(1);
        if self.mark_count < 2 {
            self.mark_position = (self.mark_position + 1) % (SEARCH_MARKS + 1);
            return;
        }
        let mut chosen = None;
        let mut best = 0.0_f32;
        let available = self.mark_count.saturating_sub(1);
        let locked_count = if self.estimate.is_some() {
            INTERVALS
        } else {
            SEARCH_MARKS
        };
        self.candidates(mark, 1, available.min(locked_count), &mut chosen, &mut best);
        if let Some((period, _)) = self.hint {
            self.consider(period, &mut chosen, &mut best);
        }
        if chosen.is_none() && locked_count == INTERVALS {
            self.candidates(mark, INTERVALS + 1, available, &mut chosen, &mut best);
        }
        self.mark_position = (self.mark_position + 1) % (SEARCH_MARKS + 1);
        if let Some(mut candidate) = chosen {
            let half = candidate.period / 2.0;
            if half >= self.min_period {
                if let Some(refined) = self.refine(half) {
                    if refined.clarity >= CLARITY_ON + HALF_MARGIN {
                        candidate = refined;
                    }
                }
            }
            self.last_clarity = candidate.clarity;
            self.estimate = Some(candidate);
            self.last_accepted_mark = mark;
            let cutoff = (LP_FACTOR * candidate.hz).clamp(LP_MIN_HZ, VOICE_MAX_HZ);
            self.lp_coefficient = (-TAU * cutoff / self.rate).exp();
        } else {
            self.last_clarity = best;
        }
        if let Some((period, remaining)) = self.hint {
            let consumed = self
                .estimate
                .is_some_and(|e| (e.period - period).abs() <= 0.01 * period);
            self.hint = if consumed || remaining <= 1 {
                None
            } else {
                Some((period, remaining - 1))
            };
        }
    }

    fn candidates(
        &self,
        mark: f64,
        start: usize,
        end: usize,
        chosen: &mut Option<CycleEstimate>,
        best: &mut f32,
    ) {
        for k in start..=end {
            let index = (self.mark_position + SEARCH_MARKS + 1 - k) % (SEARCH_MARKS + 1);
            let period = (mark - self.mark_times[index]) as f32;
            if period > self.max_period {
                break;
            }
            self.consider(period, chosen, best);
        }
    }

    fn consider(&self, period: f32, chosen: &mut Option<CycleEstimate>, best: &mut f32) {
        if let Some(candidate) = self.refine(period) {
            *best = best.max(candidate.clarity);
            if candidate.clarity >= CLARITY_ON
                && chosen.is_none_or(|old| candidate.period < old.period)
            {
                *chosen = Some(candidate);
            }
        }
    }

    fn refine(&self, period: f32) -> Option<CycleEstimate> {
        if !(self.min_period..=self.max_period).contains(&period) {
            return None;
        }
        let middle = rounded_lag(period, self.mask / 2)?;
        let lower = middle.checked_sub(1)?;
        let upper = middle + 1;
        let left = self.nsdf(lower, middle);
        let centre = self.nsdf(middle, middle);
        let right = self.nsdf(upper, middle);
        let (lag, clarity) = if centre >= left && centre >= right {
            let curvature = 2.0_f32.mul_add(-centre, left) + right;
            let offset = if curvature < -f32::EPSILON {
                (0.5 * (left - right) / curvature).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            (
                middle as f32 + offset,
                (0.25 * (left - right)).mul_add(-offset, centre),
            )
        } else if left >= right {
            (lower as f32, left)
        } else {
            (upper as f32, right)
        };
        if !lag.is_finite()
            || !(self.min_period..=self.max_period).contains(&lag)
            || !clarity.is_finite()
        {
            return None;
        }
        Some(CycleEstimate {
            period: lag,
            hz: self.rate / lag,
            clarity: clarity.clamp(0.0, 1.0),
        })
    }

    fn nsdf(&self, lag: usize, window: usize) -> f32 {
        let mut product = 0.0_f64;
        let mut energy = 0.0_f64;
        for offset in 0..window {
            let index = self.position.wrapping_sub(1).wrapping_sub(offset);
            let x = f64::from(self.history[index & self.mask]);
            let y = f64::from(self.history[index.wrapping_sub(lag) & self.mask]);
            product = x.mul_add(y, product);
            energy = x.mul_add(x, y.mul_add(y, energy));
        }
        if energy < 1e-12 || !energy.is_finite() {
            0.0
        } else {
            (2.0 * product / energy).clamp(0.0, 1.0) as f32
        }
    }
}

fn rounded_lag(period: f32, maximum: usize) -> Option<usize> {
    let rounded = period.round();
    if rounded.is_finite() && rounded >= 1.0 && rounded <= maximum as f32 {
        Some(rounded as usize)
    } else {
        None
    }
}
