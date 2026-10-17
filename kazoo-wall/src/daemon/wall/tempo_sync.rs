use super::{Wall, What};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const TEMPO_THRESHOLD: f64 = 0.05;
const GESTURE_GAP: Duration = Duration::from_secs(2);

#[derive(Debug)]
struct Gesture {
    from: f64,
    to: f64,
    moved: Instant,
}

#[derive(Debug, Default)]
pub(super) struct DeskTempo {
    /// The desk's tempo as the log stands: what the next desk change is from.
    last: Option<f64>,
    /// The desk's tempo as last heard, logged or not. A tick that hears it
    /// again has heard no move: the desk may not have applied an ask yet.
    heard: Option<f64>,
    asked: VecDeque<f64>,
    gesture: Option<Gesture>,
}

impl DeskTempo {
    pub(super) fn asked(&mut self, bpm: f64) {
        self.last = Some(bpm);
        self.asked.push_back(bpm);
        if self.asked.len() > 8 {
            drop(self.asked.drain(..1));
        }
    }

    pub(super) fn disconnected(&mut self) {
        self.last = None;
        self.heard = None;
        self.asked.clear();
        self.gesture = None;
    }
}

impl Wall {
    pub(super) fn sync_desk_tempo(&mut self, bpm: f64, now: Instant) {
        self.sync_desk_tempo_at(bpm, now);
    }

    pub(super) fn sync_desk_tempo_at(&mut self, bpm: f64, now: Instant) {
        self.settle_desk_tempo(now);
        if !bpm.is_finite()
            || self
                .desk_tempo
                .heard
                .is_some_and(|heard| heard.to_bits() == bpm.to_bits())
        {
            return;
        }
        self.desk_tempo.heard = Some(bpm);
        let from = self.desk_tempo.last.unwrap_or(self.patch.tempo);
        let requested = self
            .desk_tempo
            .asked
            .iter()
            .rposition(|asked| (*asked - bpm).abs() <= TEMPO_THRESHOLD);
        if let Some(position) = requested {
            drop(self.desk_tempo.asked.drain(..=position));
            return;
        }
        self.desk_tempo.asked.clear();
        if self.desk_tempo.gesture.is_some() || (bpm - from).abs() > TEMPO_THRESHOLD {
            self.desk_tempo.last = Some(bpm);
            if let Some(gesture) = self.desk_tempo.gesture.as_mut() {
                gesture.to = bpm;
                gesture.moved = now;
            } else {
                self.desk_tempo.gesture = Some(Gesture {
                    from,
                    to: bpm,
                    moved: now,
                });
            }
        }
        if self.desk_tempo.last.is_none() {
            self.desk_tempo.last = Some(from);
        }
    }

    pub(super) fn settle_desk_tempo(&mut self, now: Instant) {
        if self
            .desk_tempo
            .gesture
            .as_ref()
            .is_some_and(|gesture| now.saturating_duration_since(gesture.moved) >= GESTURE_GAP)
        {
            self.finish_desk_tempo();
        }
    }

    pub(super) fn finish_desk_tempo(&mut self) {
        let Some(gesture) = self.desk_tempo.gesture.take() else {
            return;
        };
        if (gesture.to - gesture.from).abs() <= TEMPO_THRESHOLD {
            self.desk_tempo.last = Some(gesture.from);
        } else {
            let paid = self.commit_own(
                "desk",
                What::Tempo {
                    from: gesture.from,
                    to: gesture.to,
                    desk: true,
                    after_beats: 0.0,
                    glide_beats: 0.0,
                },
            );
            self.tell_paid(paid.into_iter().collect());
        }
    }

    pub(super) fn desk_kept_tempo(&mut self, bpm: f64, now: Instant) {
        self.desk_tempo.asked.clear();
        self.desk_tempo.heard = None;
        self.desk_tempo.last = Some(self.patch.tempo);
        self.sync_desk_tempo(bpm, now);
        self.finish_desk_tempo();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_latest_eight_asks_are_kept() {
        let mut tempo = DeskTempo::default();
        for bpm in 100..110 {
            tempo.asked(f64::from(bpm));
        }
        assert_eq!(tempo.asked.len(), 8);
        assert_eq!(
            tempo.asked.front().copied().unwrap().to_bits(),
            102.0_f64.to_bits()
        );
        assert_eq!(tempo.last.unwrap().to_bits(), 109.0_f64.to_bits());
    }

    #[test]
    fn desk_reports_discard_older_asks_and_unmatched_reports_clear_them() {
        let (_engine, control) =
            crate::engine::engine(crate::engine::EngineConfig::new(48_000, 120.0, 0.0), None);
        let mut wall = Wall::new(
            crate::seed::seed().unwrap(),
            control,
            None,
            Vec::new(),
            None,
        );
        for bpm in [100.0, 105.0, 110.0] {
            wall.desk_tempo.asked(bpm);
        }
        wall.sync_desk_tempo(105.0, Instant::now());
        assert_eq!(wall.desk_tempo.asked.len(), 1);
        assert_eq!(
            wall.desk_tempo.asked.front().copied().unwrap().to_bits(),
            110.0_f64.to_bits()
        );
        wall.sync_desk_tempo(108.0, Instant::now());
        assert!(wall.desk_tempo.asked.is_empty());
    }
}
