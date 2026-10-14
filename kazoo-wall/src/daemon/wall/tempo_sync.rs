use super::{Wall, What};
use std::collections::VecDeque;

#[derive(Debug, Default)]
pub(super) struct DeskTempo {
    last: Option<f64>,
    asked: VecDeque<f64>,
}

impl DeskTempo {
    pub(super) fn asked(&mut self, bpm: f64) {
        self.asked.push_back(bpm);
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
        }
        if (bpm - self.patch.tempo).abs() > 1e-9 {
            self.patch.tempo = bpm;
            self.dirty = true;
        }
        if (bpm - from).abs() > 0.01 {
            self.desk_tempo.last = Some(bpm);
            if requested.is_none() {
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
        } else if self.desk_tempo.last.is_none() {
            self.desk_tempo.last = Some(bpm);
        }
    }
}
