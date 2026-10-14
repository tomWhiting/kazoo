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
    history_available: usize,
    quiet: usize,
    active: bool,
    maxima: Vec<Peak>,
    ringing: Vec<Peak>,
    pick: crate::voice::attack::Attack,
    attack_at: usize,
    last_frame: usize,
    last_available: usize,
    replaced: [f32; VOICE_HOP],
    fast_until: usize,
    selected: bool,
}

impl ProvisionalJudge {
    #[must_use]
    pub fn new(rate: u32, settings: CycleSettings) -> Self {
        let tracker = CycleTracker::with_settings(rate, settings);
        let maxima = Vec::with_capacity(tracker.max_lag);
        let ringing = Vec::with_capacity(tracker.max_lag);
        Self {
            tracker,
            available: 0,
            history_available: 0,
            quiet: 0,
            active: false,
            maxima,
            ringing,
            pick: crate::voice::attack::Attack::new(rate),
            attack_at: 0,
            last_frame: 0,
            last_available: 0,
            replaced: [0.0; VOICE_HOP],
            fast_until: 0,
            selected: false,
        }
    }

    /// Call on the judge worker, never on the audio callback.
    pub fn push(&mut self, sample: f32) -> Option<CycleEstimate> {
        if !self.tracker.settings.early {
            return None;
        }
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
            self.history_available = 0;
        }
        self.pick.push(sample);
        self.replaced[self.tracker.position % VOICE_HOP] =
            self.tracker.history[self.tracker.position & self.tracker.mask];
        let held = self.tracker.estimate.is_some();
        self.tracker.push(sample);
        if held && self.tracker.estimate.is_none() {
            // A new attack invalidates correlation with the previous note.
            self.available = 0;
        }
        if self.active {
            self.history_available = self
                .history_available
                .saturating_add(1)
                .min(self.tracker.history.len());
            self.available = self
                .available
                .saturating_add(1)
                .min(self.tracker.history.len());
        }
        let position = self.tracker.position;
        if position % VOICE_HOP == 0
            && self
                .pick
                .frame((-1000.0 / (10.0 * self.tracker.rate)).exp())
        {
            // Preserve the previous frame's full window when the ring wraps.
            let current = self.tracker.position;
            for at in self.last_frame..current {
                std::mem::swap(
                    &mut self.tracker.history[at & self.tracker.mask],
                    &mut self.replaced[at % VOICE_HOP],
                );
            }
            self.tracker.position = self.last_frame;
            self.tracker
                .key_maxima(self.last_available, &mut self.ringing);
            self.tracker.position = current;
            for at in self.last_frame..current {
                std::mem::swap(
                    &mut self.tracker.history[at & self.tracker.mask],
                    &mut self.replaced[at % VOICE_HOP],
                );
            }
            self.tracker.unlock();
            self.tracker.pick_attack(self.pick.behind());
            self.available = position - self.last_frame;
            self.attack_at = position.saturating_sub(self.pick.behind());
            self.fast_until = self.attack_at + (0.020 * self.tracker.rate) as usize;
            self.selected = false;
        }
        let hop = if position < self.fast_until {
            VOICE_HOP / 2
        } else {
            VOICE_HOP
        };
        if (position - self.attack_at) % hop != 0 {
            return None;
        }
        self.last_frame = position;
        self.last_available = self.history_available;
        let attacked = position - self.attack_at < (0.030 * self.tracker.rate) as usize;
        if !self.tracker.settings.early
            || (!attacked && self.tracker.estimate.is_some())
            || (attacked && self.selected)
            || !self.active
        {
            return None;
        }
        let peak = self.tracker.provisional_peak(
            self.available,
            &mut self.maxima,
            if attacked { &self.ringing } else { &[] },
            attacked,
        )?;
        self.tracker.accept(peak, self.tracker.time);
        if let Some(estimate) = self.tracker.estimate.as_mut() {
            estimate.provisional = true;
        }
        self.selected = attacked;
        self.tracker.estimate
    }
}

impl CycleTracker {
    fn key_maxima(&self, available: usize, maxima: &mut Vec<Peak>) {
        maxima.clear();
        let Some(first) = lag_from(f64::from(self.min_period)) else {
            return;
        };
        let Some(last) = lag_from(f64::from(self.max_period)) else {
            return;
        };
        let first = first.max(2);
        let last = last.min(self.max_lag - 1);
        let value = |lag: usize| {
            let window = available.saturating_sub(lag);
            (window >= lag.div_ceil(4)).then(|| self.nsdf(lag, window))
        };
        let mut left = value(first - 1);
        let mut centre = value(first);
        for lag in first..=last {
            let right = value(lag + 1);
            let values = (left, centre, right);
            left = centre;
            centre = right;
            let (Some(left), Some(centre), Some(right)) = values else {
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
    }

    fn provisional_peak(
        &self,
        available: usize,
        maxima: &mut Vec<Peak>,
        ringing: &[Peak],
        attacked: bool,
    ) -> Option<Peak> {
        self.key_maxima(available, maxima);
        maxima.retain(|peak| {
            !ringing.iter().any(|old| {
                (1200.0 * (peak.period / old.period).log2()).abs() <= 50.0
                    && peak.clarity < old.clarity + 0.05
            })
        });
        let highest = maxima
            .iter()
            .map(|peak| peak.clarity)
            .fold(0.0_f32, f32::max);
        let (selected, &chosen) = maxima.iter().enumerate().find(|(_, peak)| {
            peak.clarity
                >= if attacked && !ringing.is_empty() {
                    highest
                } else {
                    0.9 * highest
                }
        })?;
        #[cfg(test)]
        if super::diagnostics::enabled() {
            println!(
                "provisional at={} available={available} maxima={maxima:?} chosen={chosen:?} rule={}",
                self.position,
                if chosen.clarity < 0.60 {
                    "below-0.60"
                } else if (!attacked || ringing.is_empty()) && chosen.period <= self.rate / 120.0 {
                    "short-period"
                } else {
                    "long-period"
                }
            );
        }
        if chosen.clarity < 0.60
            || ((!attacked || ringing.is_empty()) && chosen.period <= self.rate / 120.0)
        {
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

    pub(crate) fn provisional(&mut self, estimate: CycleEstimate) -> bool {
        if !self.settings.early
            || (self.estimate.is_some() && self.time >= self.attack_until)
            || !estimate.provisional
            || !estimate.period.is_finite()
            || !estimate.clarity.is_finite()
            || !estimate.hz.is_finite()
            || estimate.hz <= 0.0
            || !(self.min_period..=self.max_period).contains(&estimate.period)
            || !(0.60..=1.0).contains(&estimate.clarity)
        {
            return false;
        }
        let Some(lag) = lag_from(f64::from(estimate.period)) else {
            return false;
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
        true
    }
}
