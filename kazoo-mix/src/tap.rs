//! Tap tempo: tap a key on the beat and the studio follows.

use std::time::{Duration, Instant};

/// Taps further apart than this start a new count.
pub const TAP_TIMEOUT: Duration = Duration::from_secs(2);

/// Most recent taps averaged into the tempo.
const TAPS_REMEMBERED: usize = 5;

/// Tap tempo over the last few taps.
#[derive(Debug, Clone, Default)]
pub struct TapTempo {
    taps: [Option<Instant>; TAPS_REMEMBERED],
    count: usize,
}

impl TapTempo {
    /// Register a tap at `at`. Returns the tempo in BPM once there are two
    /// taps close enough together, averaging the intervals of the last
    /// few taps so one sloppy tap does not throw it.
    pub fn tap(&mut self, at: Instant) -> Option<f32> {
        let last = self
            .count
            .checked_sub(1)
            .and_then(|i| self.taps[i % TAPS_REMEMBERED]);
        let fresh = last.is_none_or(|last| {
            at.checked_duration_since(last)
                .is_none_or(|gap| gap > TAP_TIMEOUT)
        });
        if fresh {
            self.taps = [None; TAPS_REMEMBERED];
            self.count = 0;
        }
        self.taps[self.count % TAPS_REMEMBERED] = Some(at);
        self.count += 1;

        let held = self.count.min(TAPS_REMEMBERED);
        if held < 2 {
            return None;
        }
        let oldest = self.taps[(self.count - held) % TAPS_REMEMBERED]?;
        let span = at.checked_duration_since(oldest)?.as_secs_f32();
        if span <= 0.0 {
            return None;
        }
        Some(60.0 * (held - 1) as f32 / span)
    }

    /// Taps in the current count.
    #[must_use]
    pub const fn taps(&self) -> usize {
        self.count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn two_taps_half_a_second_apart_are_120_bpm() {
        let start = Instant::now();
        let mut tap = TapTempo::default();
        assert_eq!(tap.tap(start), None);
        let bpm = tap.tap(at(start, 500)).unwrap();
        assert!((bpm - 120.0).abs() < 1e-3, "{bpm}");
    }

    #[test]
    fn the_last_few_taps_are_averaged() {
        let start = Instant::now();
        let mut tap = TapTempo::default();
        // Ten taps at 100 BPM (600 ms), one of them 40 ms late.
        let times = [0, 600, 1200, 1840, 2400, 3000, 3600, 4200, 4800, 5400];
        let mut bpm = None;
        for ms in times {
            bpm = tap.tap(at(start, ms));
        }
        let bpm = bpm.unwrap();
        assert!((bpm - 100.0).abs() < 1e-2, "{bpm}");
        assert_eq!(tap.taps(), 10);
    }

    #[test]
    fn a_long_pause_starts_a_new_count() {
        let start = Instant::now();
        let mut tap = TapTempo::default();
        tap.tap(start);
        tap.tap(at(start, 500));
        assert_eq!(tap.tap(at(start, 3_000)), None);
        let bpm = tap.tap(at(start, 3_750)).unwrap();
        assert!((bpm - 80.0).abs() < 1e-2, "{bpm}");
    }
}
