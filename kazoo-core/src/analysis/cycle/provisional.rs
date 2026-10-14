use super::{CycleEstimate, CycleSettings, CycleTracker, Peak, SILENCE_PEAK, lag_from};
use crate::voice::VOICE_HOP;

#[cfg(test)]
#[path = "provisional_tests.rs"]
mod tests;

/// Worker-owned history for the full-range partial-cycle search.
#[derive(Debug)]
pub struct ProvisionalJudge {
    tracker: CycleTracker,
    available: usize,
    quiet: usize,
    active: bool,
    maxima: Vec<Peak>,
}

impl ProvisionalJudge {
    #[must_use]
    pub fn new(rate: u32, settings: CycleSettings) -> Self {
        let tracker = CycleTracker::with_settings(rate, settings);
        let maxima = Vec::with_capacity(tracker.max_lag);
        Self {
            tracker,
            available: 0,
            quiet: 0,
            active: false,
            maxima,
        }
    }

    /// Call on the judge worker, never on the audio callback.
    pub fn push(&mut self, sample: f32) -> Option<CycleEstimate> {
        let sample = crate::sanitize_sample(sample).clamp(-1.0, 1.0);
        if sample.abs() >= SILENCE_PEAK {
            self.active = true;
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(1);
        }
        if self.quiet >= self.tracker.history.len() {
            self.active = false;
            self.available = 0;
        }
        let held = self.tracker.estimate.is_some();
        self.tracker.push(sample);
        if held && self.tracker.estimate.is_none() {
            // A new attack invalidates correlation with the previous note.
            self.available = 0;
        }
        if self.active {
            self.available = self
                .available
                .saturating_add(1)
                .min(self.tracker.history.len());
        }
        if !self.tracker.settings.early
            || self.tracker.estimate.is_some()
            || self.tracker.position % VOICE_HOP != 0
            || !self.active
        {
            return None;
        }
        let peak = self
            .tracker
            .provisional_peak(self.available, &mut self.maxima)?;
        self.tracker.accept(peak, self.tracker.time);
        if let Some(estimate) = self.tracker.estimate.as_mut() {
            estimate.provisional = true;
        }
        self.tracker.estimate
    }
}

impl CycleTracker {
    fn provisional_peak(&self, available: usize, maxima: &mut Vec<Peak>) -> Option<Peak> {
        maxima.clear();
        let first = lag_from(f64::from(self.min_period))?.max(2);
        let last = lag_from(f64::from(self.max_period))?.min(self.max_lag - 1);
        let value = |lag: usize| {
            let window = available.saturating_sub(lag);
            (window >= lag.div_ceil(4)).then(|| self.nsdf(lag, window))
        };
        for lag in first..=last {
            let (Some(left), Some(centre), Some(right)) =
                (value(lag - 1), value(lag), value(lag + 1))
            else {
                continue;
            };
            if centre > left && centre >= right {
                let curvature = 2.0_f32.mul_add(-centre, left) + right;
                let offset = if curvature < -f32::EPSILON {
                    (0.5 * (left - right) / curvature).clamp(-0.5, 0.5)
                } else {
                    0.0
                };
                maxima.push(Peak {
                    period: lag as f32 + offset,
                    clarity: centre,
                    window: available - lag,
                });
            }
        }
        let highest = maxima
            .iter()
            .map(|peak| peak.clarity)
            .fold(0.0_f32, f32::max);
        let (selected, &chosen) = maxima
            .iter()
            .enumerate()
            .find(|(_, peak)| peak.clarity >= 0.9 * highest)?;
        #[cfg(test)]
        if super::diagnostics::enabled() {
            println!(
                "provisional at={} available={available} maxima={maxima:?} chosen={chosen:?} rule={}",
                self.position,
                if chosen.clarity < 0.60 {
                    "below-0.60"
                } else if chosen.period <= self.rate / 120.0 {
                    "short-period"
                } else {
                    "long-period"
                }
            );
        }
        if chosen.clarity < 0.60 || chosen.period <= self.rate / 120.0 {
            return None;
        }
        let next = maxima
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != selected)
            .map(|(_, peak)| peak)
            .max_by(|a, b| a.clarity.total_cmp(&b.clarity));
        if next.is_some_and(|peak| {
            let cents = (1200.0 * (peak.period / chosen.period).log2()).abs();
            peak.clarity > chosen.clarity && (cents - 1200.0).abs() <= 5.0
        }) {
            return None;
        }
        Some(chosen)
    }

    pub(crate) fn provisional(&mut self, estimate: CycleEstimate) {
        if !self.settings.early
            || self.estimate.is_some()
            || !estimate.provisional
            || !estimate.period.is_finite()
            || !estimate.clarity.is_finite()
            || !(self.min_period..=self.max_period).contains(&estimate.period)
            || !(0.60..=1.0).contains(&estimate.clarity)
        {
            return;
        }
        let Some(lag) = lag_from(f64::from(estimate.period)) else {
            return;
        };
        self.accept(
            Peak {
                period: estimate.period,
                clarity: estimate.clarity,
                window: self.window(lag),
            },
            self.time,
        );
        if let Some(held) = self.estimate.as_mut() {
            held.provisional = true;
        }
    }
}
