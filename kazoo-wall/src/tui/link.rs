//! The console's link to the daemon: whether it is up, and when to try
//! again when it is not.
//!
//! The daemon can restart under the console at any time (someone ran
//! `kazoo-wall stop` and started it again, or it crashed). [`Reconnect`]
//! keeps the state the console shows and paces the attempts: quickly at
//! first, so a restart is picked up within a second, then once a second
//! for as long as it takes.

use std::time::{Duration, Instant};

/// Wait before each retry: the first few come quickly, then once a second.
const BACKOFF: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
];

/// The steady retry interval once the quick retries are spent.
pub const RETRY: Duration = Duration::from_secs(1);

/// Whether the console can reach the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkState {
    /// Trying for the first time.
    Connecting,
    /// Talking to the daemon.
    Connected {
        /// The daemon and its version, as it said hello.
        daemon: String,
    },
    /// Not reachable; trying again.
    Down {
        /// Why, in the socket's words.
        reason: String,
        /// Attempts since the link was last up.
        attempts: u32,
        /// Whether the link was up before (the daemon went away) rather
        /// than never reached.
        was_up: bool,
    },
}

impl LinkState {
    /// Whether the console is talking to the daemon.
    #[must_use]
    pub const fn is_up(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

/// The link's state and the pacing of reconnection attempts.
#[derive(Debug, Clone)]
pub struct Reconnect {
    state: LinkState,
    next_try: Instant,
    ever_up: bool,
}

impl Reconnect {
    /// A link that has not tried yet: the first attempt is due at `now`.
    #[must_use]
    pub const fn new(now: Instant) -> Self {
        Self {
            state: LinkState::Connecting,
            next_try: now,
            ever_up: false,
        }
    }

    /// The state to show.
    #[must_use]
    pub const fn state(&self) -> &LinkState {
        &self.state
    }

    /// Whether an attempt to connect is due at `now` (never while up).
    #[must_use]
    pub fn due(&self, now: Instant) -> bool {
        !self.state.is_up() && now >= self.next_try
    }

    /// How long until the next attempt is due (zero if it is due now; the
    /// steady interval while up, when no attempt is ever due).
    #[must_use]
    pub fn wait(&self, now: Instant) -> Duration {
        if self.state.is_up() {
            RETRY
        } else {
            self.next_try.saturating_duration_since(now)
        }
    }

    /// An attempt succeeded.
    pub fn connected(&mut self, daemon: String) {
        self.state = LinkState::Connected { daemon };
        self.ever_up = true;
    }

    /// An attempt failed, or the link broke, at `now`: wait and try again.
    pub fn failed(&mut self, reason: String, now: Instant) {
        let attempts = match &self.state {
            LinkState::Down { attempts, .. } => attempts.saturating_add(1),
            LinkState::Connecting | LinkState::Connected { .. } => 1,
        };
        let pause = BACKOFF
            .get(attempts.saturating_sub(1) as usize)
            .copied()
            .unwrap_or(RETRY);
        self.state = LinkState::Down {
            reason,
            attempts,
            was_up: self.ever_up,
        };
        self.next_try = now + pause;
    }

    /// Try again at once (the daemon was just started).
    pub const fn retry_now(&mut self, now: Instant) {
        if !self.state.is_up() {
            self.next_try = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_attempt_is_due_at_once() {
        let now = Instant::now();
        let link = Reconnect::new(now);
        assert_eq!(link.state(), &LinkState::Connecting);
        assert!(link.due(now));
        assert_eq!(link.wait(now), Duration::ZERO);
    }

    #[test]
    fn failures_back_off_to_once_a_second_and_count() {
        let start = Instant::now();
        let mut link = Reconnect::new(start);
        let mut now = start;
        let mut pauses = Vec::new();
        for _ in 0..5 {
            link.failed("no such file".to_string(), now);
            let pause = link.wait(now);
            assert!(!link.due(now));
            assert!(link.due(now + pause));
            pauses.push(pause);
            now += pause;
        }
        assert_eq!(
            pauses,
            vec![
                Duration::from_millis(100),
                Duration::from_millis(250),
                Duration::from_millis(500),
                RETRY,
                RETRY
            ]
        );
        assert_eq!(
            link.state(),
            &LinkState::Down {
                reason: "no such file".to_string(),
                attempts: 5,
                was_up: false
            }
        );
    }

    #[test]
    fn a_daemon_restart_goes_down_then_up_again_quickly() {
        let now = Instant::now();
        let mut link = Reconnect::new(now);
        link.connected("kazoo-wall 0.1.0".to_string());
        assert!(link.state().is_up());
        assert!(!link.due(now + Duration::from_secs(60)));

        // The daemon goes away: the link says so and retries quickly.
        link.failed("the wall closed the connection".to_string(), now);
        assert_eq!(
            link.state(),
            &LinkState::Down {
                reason: "the wall closed the connection".to_string(),
                attempts: 1,
                was_up: true
            }
        );
        assert!(link.due(now + Duration::from_millis(100)));

        // It comes back.
        link.connected("kazoo-wall 0.1.0".to_string());
        assert!(link.state().is_up());

        // Going down again starts the quick retries afresh.
        link.failed("gone".to_string(), now);
        assert_eq!(link.wait(now), Duration::from_millis(100));
    }

    #[test]
    fn a_fresh_start_retries_at_once() {
        let now = Instant::now();
        let mut link = Reconnect::new(now);
        for _ in 0..6 {
            link.failed("gone".to_string(), now);
        }
        assert!(!link.due(now));
        link.retry_now(now);
        assert!(link.due(now));
    }
}
