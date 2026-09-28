//! The console's own copy of the wall's flood guard.
//!
//! The daemon gives each seat a bucket of [`FLOOD_BURST`] changes that
//! refills at [`FLOOD_REFILL_PER_SECOND`], and refuses changes beyond it
//! with `slow_down`. A knob dragged with the mouse would empty that in a few
//! seconds, so the console keeps the same bucket and holds a turn back
//! (showing it on screen meanwhile) until the wall would take it. Held
//! turns are coalesced, so the value the wall ends on is always the last
//! one Tom chose.

use std::time::Instant;

use kazoo_wall::daemon::wall::{FLOOD_BURST, FLOOD_REFILL_PER_SECOND};

/// Changes the console may send now, refilling over time.
#[derive(Debug, Clone, PartialEq)]
pub struct Flood {
    tokens: f64,
    at: Option<Instant>,
}

impl Default for Flood {
    fn default() -> Self {
        Self::new()
    }
}

impl Flood {
    /// A full bucket.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tokens: FLOOD_BURST,
            at: None,
        }
    }

    fn refill(&mut self, now: Instant) {
        if let Some(at) = self.at {
            let elapsed = now.saturating_duration_since(at).as_secs_f64();
            self.tokens = FLOOD_REFILL_PER_SECOND
                .mul_add(elapsed, self.tokens)
                .min(FLOOD_BURST);
        }
        self.at = Some(self.at.map_or(now, |at| at.max(now)));
    }

    /// Take one change from the bucket, if the wall would accept one now.
    pub fn take(&mut self, now: Instant) -> bool {
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Count a change that is sent whatever the bucket says (a single
    /// press of a key: the wall answers it, `slow_down` or not).
    pub fn spend(&mut self, now: Instant) {
        self.refill(now);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A shade past the time one change takes to refill: a duration holds
    /// whole nanoseconds, and a third of a second rounded down refills a
    /// hair under one (as the wall reckons it too).
    fn one_refill() -> Duration {
        Duration::from_secs_f64(1.0 / FLOOD_REFILL_PER_SECOND) + Duration::from_micros(1)
    }

    #[test]
    fn a_burst_then_the_refill_rate() {
        let start = Instant::now();
        let mut flood = Flood::new();
        let burst = FLOOD_BURST as usize;
        for _ in 0..burst {
            assert!(flood.take(start));
        }
        assert!(!flood.take(start), "the burst is spent");
        let one = one_refill();
        assert!(flood.take(start + one), "one refilled");
        assert!(!flood.take(start + one));
        // Ten quiet seconds refill it, never past the burst.
        let later = start + Duration::from_secs(100);
        for _ in 0..burst {
            assert!(flood.take(later));
        }
        assert!(!flood.take(later));
    }

    #[test]
    fn spending_counts_but_never_goes_below_empty() {
        let start = Instant::now();
        let mut flood = Flood::new();
        for _ in 0..(FLOOD_BURST as usize + 5) {
            flood.spend(start);
        }
        let one = one_refill();
        assert!(flood.take(start + one), "empty, not in debt");
        // Time running backwards (a clock step) refills nothing.
        assert!(!flood.take(start));
    }
}
