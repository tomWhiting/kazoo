//! A bounded half-cycle search before the normal cycle marks lock.

use super::{CLARITY_ON, CycleTracker, HALF_MARGIN, Peak, SCHMITT, SILENCE_PEAK, lag_from};

/// Half-period onset evidence must confirm a period inferred from one lobe,
/// so only NSDF >= 0.90 confirms it. A plucked string's clean first cycles
/// can meet this floor.
pub(super) const EARLY_CLARITY: f32 = 0.90;
/// A strong second harmonic makes the two lobes unequal. These five lags
/// bracket errors of 10 to 25 percent in twice the negative lobe's length.
pub(super) const EARLY_LAGS: [f32; 5] = [0.80, 0.90, 1.00, 1.11, 1.25];

#[cfg(test)]
pub mod fixtures;

#[derive(Debug, Clone, Copy)]
pub(super) struct Candidate {
    pub(super) period: f32,
    pub(super) deadline: f64,
}

/// Only scalar state is added to the preallocated cycle history.
#[derive(Debug, Clone, Copy)]
pub(super) struct Early {
    first_crossing: Option<f64>,
    downward: Option<f64>,
    armed: bool,
    pub(super) pending: Option<Candidate>,
    pub(super) tried: bool,
}

impl Early {
    pub(super) const fn new() -> Self {
        Self {
            first_crossing: None,
            downward: None,
            armed: false,
            pending: None,
            tried: false,
        }
    }

    fn crossing(&mut self, previous: f32, filtered: f32, now: f64) {
        if previous == 0.0 && filtered != 0.0 && self.first_crossing.is_none() {
            self.first_crossing = Some(now - 1.0);
        }
        if previous >= 0.0 && filtered < 0.0 {
            let at = now - 1.0 + f64::from(previous) / (f64::from(previous) - f64::from(filtered));
            if at.is_finite() {
                self.first_crossing.get_or_insert(at);
                self.downward = Some(at);
                self.armed = false;
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Probe {
    lag: usize,
    left: f32,
    centre: f32,
    right: f32,
}

impl Probe {
    /// The same three-point parabola as a normal reading, without climbing.
    fn local(self) -> f32 {
        self.vertex(0.5)
    }

    fn vertex(self, reach: f32) -> f32 {
        let curvature = 2.0_f32.mul_add(-self.centre, self.left) + self.right;
        let offset = if curvature < -f32::EPSILON {
            (0.5 * (self.left - self.right) / curvature).clamp(-reach, reach)
        } else {
            0.0
        };
        self.lag as f32 + offset
    }
}

impl CycleTracker {
    pub(super) fn early_push(&mut self, previous: f32, filtered: f32) {
        if self.early.tried {
            return;
        }
        self.early.crossing(previous, filtered, self.time);
        if self.peak >= SILENCE_PEAK && filtered < -SCHMITT * self.peak {
            self.early.armed = true;
        }
        if previous < 0.0 && filtered >= 0.0 && self.early.armed {
            self.early.armed = false;
            if let (Some(down), Some(first)) = (self.early.downward, self.early.first_crossing) {
                let period = (2.0 * (self.last_rise - down)) as f32;
                if self.early.pending.is_none()
                    && period.is_finite()
                    && (self.min_period..=self.max_period).contains(&period)
                {
                    self.early.pending = Some(Candidate {
                        period,
                        deadline: 1.5_f64.mul_add(f64::from(period), first),
                    });
                }
            }
        }
        let Some(candidate) = self.early.pending else {
            return;
        };
        if self.time < candidate.deadline {
            return;
        }
        // Spending the search before evaluating bounds refused notes too.
        self.early.tried = true;
        self.early.pending = None;
        if let Some(peak) = self.early_peak(candidate.period) {
            self.accept(peak, self.time);
            if let Some(estimate) = self.estimate.as_mut() {
                estimate.provisional = true;
            }
        }
    }

    fn early_probe(&self, period: f32, window: usize) -> Option<Probe> {
        if !period.is_finite() || !(self.min_period..=self.max_period).contains(&period) {
            return None;
        }
        let lag = lag_from(f64::from(period)).filter(|&lag| lag >= 2 && lag < self.max_lag)?;
        Some(Probe {
            lag,
            left: self.nsdf(lag - 1, window),
            centre: self.nsdf(lag, window),
            right: self.nsdf(lag + 1, window),
        })
    }

    fn early_peak(&self, period: f32) -> Option<Peak> {
        let window = lag_from(f64::from(period / 2.0)).filter(|&window| window > 0)?;
        let mut probes = [None; EARLY_LAGS.len()];
        let mut best: Option<(usize, Probe)> = None;
        for (index, (&factor, slot)) in EARLY_LAGS.iter().zip(&mut probes).enumerate() {
            *slot = lag_from(f64::from(period * factor))
                .filter(|&lag| lag >= 2 && lag < self.max_lag)
                .filter(|_| (self.min_period..=self.max_period).contains(&(period * factor)))
                .map(|lag| {
                    let centre = self.nsdf(lag, window);
                    Probe {
                        lag,
                        left: centre,
                        centre,
                        right: centre,
                    }
                });
            if let Some(probe) = *slot {
                if best.is_none_or(|(_, old)| probe.centre > old.centre) {
                    best = Some((index, probe));
                }
            }
        }
        let (index, probe) = best.filter(|(_, probe)| probe.centre >= EARLY_CLARITY)?;
        let left = index
            .checked_sub(1)
            .and_then(|i| probes.get(i))
            .copied()
            .flatten();
        let right = probes.get(index + 1).copied().flatten();
        let refined = match (left, right) {
            (Some(left), Some(right)) => between(left, probe, right)?,
            _ => probe.lag as f32,
        };
        if !refined.is_finite() || !(self.min_period..=self.max_period).contains(&refined) {
            return None;
        }
        // The coarse parabola only brackets the peak. Refit locally before
        // publishing: its asymmetric spacing can leave several samples of
        // bias. Five coarse lags, three local triplets and the half check
        // use seventeen lag evaluations, within the eighteen-lag bound.
        let mut local = self.early_probe(refined, window)?;
        for _ in 0..2 {
            local = self.early_probe(local.vertex(local.lag as f32 / 16.0), window)?;
        }
        if local.centre < EARLY_CLARITY || local.centre < local.left || local.centre < local.right {
            return None;
        }
        let mut peak = Peak {
            period: local.local(),
            clarity: local.centre,
            window,
        };
        let half_period = peak.period / 2.0;
        let half_window = self.window(lag_from(f64::from(half_period))?);
        if let Some(half) = self.early_probe(half_period, half_window) {
            if half.centre >= CLARITY_ON + HALF_MARGIN
                && half.centre >= half.left
                && half.centre >= half.right
            {
                peak.period = half.local();
                peak.clarity = half.centre;
                peak.window = half_window;
            }
        }
        (peak.period.is_finite()
            && peak.clarity.is_finite()
            && (self.min_period..=self.max_period).contains(&peak.period))
        .then_some(peak)
    }
}

/// Unequal lobe-search spacing needs divided differences, rather than the
/// equally spaced parabola used around a single integer lag.
fn between(left: Probe, centre: Probe, right: Probe) -> Option<f32> {
    let low = left.lag as f32 - centre.lag as f32;
    let high = right.lag as f32 - centre.lag as f32;
    if low >= 0.0 || high <= 0.0 {
        return None;
    }
    let low_slope = (left.centre - centre.centre) / low;
    let high_slope = (right.centre - centre.centre) / high;
    let curvature = (high_slope - low_slope) / (high - low);
    let slope = (-curvature).mul_add(high, high_slope);
    if !curvature.is_finite() || !slope.is_finite() {
        return None;
    }
    if curvature >= -f32::EPSILON {
        return Some(centre.local());
    }
    let offset = (-slope / (2.0 * curvature)).clamp(low / 2.0, high / 2.0);
    Some(centre.lag as f32 + offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::live::InputProfile;

    #[test]
    fn a_coarse_lobe_search_preserves_fractional_pitch() {
        let mut tracker =
            CycleTracker::with_settings(48_000, InputProfile::Guitar.tracking().cycle);
        let period = 48_000.0_f32 / 110.0;
        let length = tracker.history.len();
        for (index, sample) in tracker.history.iter_mut().enumerate() {
            let phase = 0.3 - (length - 1 - index) as f64 / f64::from(period);
            *sample = (std::f64::consts::TAU * phase).sin() as f32;
        }
        tracker.position = length;
        let estimate = tracker
            .early_peak(0.96 * period)
            .expect("clean lobe confirms");
        let error = (1200.0 * (estimate.period / period).log2()).abs();
        assert!(error <= 3.0, "{error} cents: {estimate:?}");
    }
}
