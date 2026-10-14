use super::{Wall, What};
use std::collections::VecDeque;

#[derive(Debug, Default)]
pub(super) struct DeskTempo {
    last: Option<f64>,
    asked: VecDeque<f64>,
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
        self.asked.clear();
    }
}

impl Wall {
    pub(super) fn sync_desk_tempo(&mut self, bpm: f64) {
        if !bpm.is_finite() {
            return;
        }
        let from = self.desk_tempo.last.unwrap_or(self.patch.tempo);
        let requested = self
            .desk_tempo
            .asked
            .iter()
            .rposition(|asked| (*asked - bpm).abs() <= 0.01);
        if let Some(position) = requested {
            drop(self.desk_tempo.asked.drain(..=position));
            return;
        }
        self.desk_tempo.asked.clear();
        if (bpm - from).abs() > 0.01 {
            self.desk_tempo.last = Some(bpm);
            let paid = self.commit_own(
                "desk",
                What::Tempo {
                    from,
                    to: bpm,
                    desk: true,
                },
            );
            self.tell_paid(paid.into_iter().collect());
        }
        if self.desk_tempo.last.is_none() {
            self.desk_tempo.last = Some(from);
        }
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
        wall.sync_desk_tempo(105.0);
        assert_eq!(wall.desk_tempo.asked.len(), 1);
        assert_eq!(
            wall.desk_tempo.asked.front().copied().unwrap().to_bits(),
            110.0_f64.to_bits()
        );
        wall.sync_desk_tempo(108.0);
        assert!(wall.desk_tempo.asked.is_empty());
    }
}
