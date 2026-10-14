use super::cancellation::Cancellation;
use super::{CycleEstimate, CycleSettings, CycleTracker, Peak};
use crate::voice::{VOICE_HOP, VoiceFrame};

#[derive(Debug)]
pub(super) struct Residual {
    pub(super) tracker: CycleTracker,
    pub(super) cancellation: Cancellation,
    pub(super) published: Vec<(u64, Peak)>,
    pub(super) sources: Vec<(u64, Peak)>,
    pub(super) ringing: Vec<Peak>,
    pub(super) maxima: Vec<Peak>,
    pub(super) pitch: Option<crate::voice::judge::JudgeResult>,
    until: usize,
    pub(super) available: usize,
    note: Option<u64>,
}

impl Residual {
    pub(super) fn new(rate: u32, settings: CycleSettings) -> Self {
        let tracker = CycleTracker::with_settings(rate, settings);
        Self {
            cancellation: Cancellation::new(tracker.history.len(), tracker.max_lag),
            published: Vec::with_capacity(tracker.max_lag),
            sources: Vec::with_capacity(tracker.max_lag),
            ringing: Vec::with_capacity(tracker.max_lag),
            maxima: Vec::with_capacity(tracker.max_lag),
            tracker,
            pitch: None,
            until: 0,
            available: 0,
            note: None,
        }
    }

    pub(super) fn observe(&mut self, frame: VoiceFrame) {
        if !frame.gate {
            self.published.clear();
            self.note = None;
        } else if frame.voiced && frame.hz.is_finite() && frame.hz > 0.0 {
            let peak = Peak {
                period: self.tracker.rate / frame.hz,
                clarity: frame.confidence,
                window: 0,
            };
            if frame.onset || self.note.is_none() {
                self.note = Some(frame.at);
                self.published.push((frame.at, peak));
            } else if let Some((_, last)) = self.published.last_mut() {
                // Corrections belong to the same articulated note.
                *last = peak;
            }
        }
    }

    pub(super) fn push(&mut self, sample: f32, at: usize) {
        self.pitch = None;
        let sample = crate::sanitize_sample(sample).clamp(-1.0, 1.0);
        self.cancellation.raw[at & self.tracker.mask] = sample;
        if at < self.until {
            let value = self.cancellation.push(sample, at);
            self.tracker.history[at & self.tracker.mask] = value;
            self.tracker.position = at + 1;
            self.available += 1;
        }
    }

    pub(super) fn begin_attack(
        &mut self,
        attack: usize,
        position: usize,
        history_available: usize,
        pre_attack: &[Peak],
    ) {
        self.sources.clear();
        self.sources.extend_from_slice(&self.published);
        self.ringing.clear();
        self.ringing
            .extend(self.published.iter().map(|(_, peak)| *peak));
        self.ringing.extend(
            pre_attack
                .iter()
                .copied()
                .filter(|peak| peak.clarity >= 0.60),
        );
        self.cancellation.configure(&mut self.ringing, attack);
        self.tracker.reset();
        for at in position.saturating_sub(history_available)..position {
            let raw = self.cancellation.raw[at & self.tracker.mask];
            self.tracker.history[at & self.tracker.mask] = self.cancellation.push(raw, at);
        }
        self.tracker.position = position;
        self.available = position - attack;
        self.until = attack + (0.040 * self.tracker.rate) as usize;
    }

    pub(super) fn read(&mut self, position: usize, attack: usize) {
        if position >= self.until || (position - attack) % (VOICE_HOP / 2) != 0 {
            return;
        }
        let peak = if self.ringing.is_empty() {
            None
        } else {
            self.tracker.residual_peak(self.available, &mut self.maxima)
        };
        self.pitch = Some(crate::voice::judge::JudgeResult::Residual(peak.map(
            |peak| CycleEstimate {
                hz: self.tracker.rate / peak.period,
                period: peak.period,
                clarity: peak.clarity,
                provisional: true,
            },
        )));
    }
}

impl CycleTracker {
    fn residual_peak(&self, available: usize, maxima: &mut Vec<Peak>) -> Option<Peak> {
        self.key_maxima(available, maxima);
        let highest = maxima
            .iter()
            .map(|peak| peak.clarity)
            .fold(0.0_f32, f32::max);
        let (selected, &chosen) = maxima
            .iter()
            .enumerate()
            .find(|(_, peak)| peak.clarity >= 0.9 * highest)?;
        if chosen.clarity < 0.60 {
            return None;
        }
        let next = maxima
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != selected)
            .map(|(_, peak)| peak)
            .max_by(|a, b| a.clarity.total_cmp(&b.clarity));
        if next.is_some_and(|peak| {
            peak.clarity > chosen.clarity
                && ((1200.0 * (peak.period / chosen.period).log2()).abs() - 1200.0).abs() <= 5.0
        }) {
            return None;
        }
        Some(chosen)
    }
}
