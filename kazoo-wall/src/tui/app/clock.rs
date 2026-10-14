use std::time::{Duration, Instant};

use super::App;

impl App {
    pub fn beat_at(&self, now: Instant) -> f64 {
        self.snapshot().map_or(0.0, |snapshot| {
            let since = self
                .look_at
                .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
            super::super::clock::beat_now(snapshot.beat, snapshot.tempo, since)
        })
    }
}
