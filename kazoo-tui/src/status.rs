//! User-visible status line.
//!
//! Every failure the TUI encounters while talking to the engine, reading the
//! filesystem, or starting up is reported here so it is shown in the header
//! instead of being silently discarded. Successful user-initiated operations
//! whose outcome is otherwise invisible (e.g. loading a clip) post an
//! informational message.

use std::time::{Duration, Instant};

use kazoo_core::engine::EngineStatsSnapshot;

/// How long an informational message stays visible.
const INFO_LIFETIME: Duration = Duration::from_secs(4);

/// How long an error message stays visible after it was last reported.
const ERROR_LIFETIME: Duration = Duration::from_secs(10);

/// Severity of a status message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLevel {
    /// Something the user asked for happened.
    Info,
    /// Something failed; the user's action did not take effect.
    Error,
}

/// A single message shown in the header status area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    /// Message text.
    pub text: String,
    /// Severity.
    pub level: StatusLevel,
    /// How many consecutive times this exact message was reported.
    pub repeats: u32,
    /// When the message was last reported.
    posted_at: Instant,
}

impl StatusMessage {
    /// Whether the message is still within its display lifetime at `now`.
    #[must_use]
    fn is_live(&self, now: Instant) -> bool {
        let lifetime = match self.level {
            StatusLevel::Info => INFO_LIFETIME,
            StatusLevel::Error => ERROR_LIFETIME,
        };
        now.saturating_duration_since(self.posted_at) < lifetime
    }
}

/// The status line: the most recent message.
#[derive(Debug, Clone, Default)]
pub struct StatusLine {
    current: Option<StatusMessage>,
}

impl StatusLine {
    /// Post an informational message, replacing any current message.
    pub fn info(&mut self, text: impl Into<String>) {
        self.post(text.into(), StatusLevel::Info, Instant::now());
    }

    /// Post an error message, replacing any current message.
    pub fn error(&mut self, text: impl Into<String>) {
        self.post(text.into(), StatusLevel::Error, Instant::now());
    }

    /// Report the outcome of an engine operation.
    ///
    /// Returns `true` when the operation succeeded. On failure an error
    /// message of the form `"{action} failed: {error}"` is posted and
    /// `false` is returned so the caller can leave local state untouched.
    pub fn report(&mut self, action: &str, result: kazoo_core::Result<()>) -> bool {
        match result {
            Ok(()) => true,
            Err(err) => {
                self.error(format!("{action} failed: {err}"));
                false
            }
        }
    }

    /// The message to display at `now`, if one is still live.
    #[must_use]
    pub fn visible(&self, now: Instant) -> Option<&StatusMessage> {
        self.current.as_ref().filter(|msg| msg.is_live(now))
    }

    fn post(&mut self, text: String, level: StatusLevel, now: Instant) {
        if let Some(current) = self.current.as_mut() {
            if current.level == level && current.text == text && current.is_live(now) {
                current.repeats = current.repeats.saturating_add(1);
                current.posted_at = now;
                return;
            }
        }
        self.current = Some(StatusMessage {
            text,
            level,
            repeats: 1,
            posted_at: now,
        });
    }
}

// ---------------------------------------------------------------------------
// Engine health
// ---------------------------------------------------------------------------

/// One engine failure counter: how the header names it, and how the status
/// line describes it when it rises.
struct Counter {
    /// Short name for the persistent header summary.
    short: &'static str,
    /// What was lost, for the status line.
    long: &'static str,
    /// Read this counter from a snapshot.
    read: fn(&EngineStatsSnapshot) -> u64,
}

/// Every counter in [`EngineStatsSnapshot`], most serious first.
const COUNTERS: [Counter; 17] = [
    Counter {
        short: "rec gaps",
        long: "recorded audio lost (the WAV file has gaps)",
        read: |s| s.disk_samples_dropped,
    },
    Counter {
        short: "rec errors",
        long: "recorder errors",
        read: |s| s.disk_errors,
    },
    Counter {
        short: "rec cmds",
        long: "recorder start/stop commands lost",
        read: |s| s.disk_commands_dropped,
    },
    Counter {
        short: "take gaps",
        long: "recorded samples lost (a take has gaps)",
        read: |s| s.take_samples_dropped,
    },
    Counter {
        short: "takes",
        long: "armed tracks that could not start recording",
        read: |s| s.takes_unavailable,
    },
    Counter {
        short: "clips",
        long: "clips that could not be placed",
        read: |s| s.clips_rejected,
    },
    Counter {
        short: "tracks",
        long: "tracks the engine refused",
        read: |s| s.tracks_rejected,
    },
    Counter {
        short: "synths",
        long: "synths, layers or effects the engine refused",
        read: |s| s.processors_rejected,
    },
    Counter {
        short: "cmds",
        long: "engine commands dropped",
        read: |s| s.commands_dropped,
    },
    Counter {
        short: "params",
        long: "parameter changes rejected",
        read: |s| s.params_rejected,
    },
    Counter {
        short: "mic",
        long: "mic samples dropped",
        read: |s| s.mic_samples_dropped,
    },
    Counter {
        short: "desk reqs",
        long: "transport requests the desk never got",
        read: |s| s.desk_requests_dropped,
    },
    Counter {
        short: "desk syncs",
        long: "desk transport syncs not followed",
        read: |s| s.desk_syncs_rejected,
    },
    Counter {
        short: "pitch in",
        long: "samples the pitch tracker missed",
        read: |s| s.analysis_samples_dropped,
    },
    Counter {
        short: "pitch out",
        long: "pitch results dropped",
        read: |s| s.analysis_results_dropped,
    },
    Counter {
        short: "rt frees",
        long: "objects freed on the audio thread (engine fault)",
        read: |s| s.callback_frees,
    },
    Counter {
        short: "screen",
        long: "screen updates skipped",
        read: |s| s.display_frames_dropped,
    },
];

/// Tracks the engine's failure counters ([`EngineHandle::stats`]) so the UI
/// can show every non-zero one, and say so the moment one rises.
///
/// [`EngineHandle::stats`]: kazoo_core::engine::EngineHandle::stats
#[derive(Debug, Clone, Default)]
pub struct EngineHealth {
    /// The counters as last observed.
    seen: EngineStatsSnapshot,
    /// When a counter last rose.
    last_rise: Option<Instant>,
}

impl EngineHealth {
    /// Record the latest counters at `now`. When any rose since the last
    /// observation, returns a status-line message naming what was lost
    /// (worst first) and how much.
    pub fn observe(&mut self, latest: EngineStatsSnapshot, now: Instant) -> Option<String> {
        let mut rises = COUNTERS.iter().filter_map(|counter| {
            let rise = (counter.read)(&latest).saturating_sub((counter.read)(&self.seen));
            (rise > 0).then_some((counter, rise))
        });
        let (worst, amount) = rises.next()?;
        let others = rises.count();
        self.seen = latest;
        self.last_rise = Some(now);
        let more = match others {
            0 => String::new(),
            1 => " (+1 other problem, see header)".to_owned(),
            n => format!(" (+{n} other problems, see header)"),
        };
        Some(format!("Engine: {amount} {}{more}", worst.long))
    }

    /// Every non-zero counter as `(short name, total)`, worst first.
    pub fn nonzero(&self) -> impl Iterator<Item = (&'static str, u64)> + '_ {
        COUNTERS.iter().filter_map(|counter| {
            let total = (counter.read)(&self.seen);
            (total > 0).then_some((counter.short, total))
        })
    }

    /// Whether a counter rose within the error lifetime before `now`, so
    /// the header can draw attention to it.
    #[must_use]
    pub fn is_fresh(&self, now: Instant) -> bool {
        self.last_rise
            .is_some_and(|at| now.saturating_duration_since(at) < ERROR_LIFETIME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_status_shows_nothing() {
        let status = StatusLine::default();
        assert!(status.visible(Instant::now()).is_none());
    }

    #[test]
    fn report_ok_posts_nothing() {
        let mut status = StatusLine::default();
        assert!(status.report("Mute", Ok(())));
        assert!(status.visible(Instant::now()).is_none());
    }

    #[test]
    fn report_err_posts_error() {
        let mut status = StatusLine::default();
        assert!(!status.report("Mute", Err(kazoo_core::Error::EngineNotRunning)));
        let msg = status.visible(Instant::now()).unwrap();
        assert_eq!(msg.level, StatusLevel::Error);
        assert_eq!(msg.text, "Mute failed: Engine not running");
    }

    #[test]
    fn repeated_message_counts_repeats() {
        let mut status = StatusLine::default();
        status.error("boom");
        status.error("boom");
        status.error("boom");
        let msg = status.visible(Instant::now()).unwrap();
        assert_eq!(msg.repeats, 3);
    }

    #[test]
    fn different_message_replaces_current() {
        let mut status = StatusLine::default();
        status.error("first");
        status.info("second");
        let msg = status.visible(Instant::now()).unwrap();
        assert_eq!(msg.text, "second");
        assert_eq!(msg.level, StatusLevel::Info);
        assert_eq!(msg.repeats, 1);
    }

    #[test]
    fn messages_expire() {
        let mut status = StatusLine::default();
        status.info("hello");
        let later = Instant::now() + INFO_LIFETIME + Duration::from_millis(1);
        assert!(status.visible(later).is_none());

        status.error("bad");
        let before_expiry = Instant::now() + INFO_LIFETIME + Duration::from_millis(1);
        assert!(status.visible(before_expiry).is_some());
        let after_expiry = Instant::now() + ERROR_LIFETIME + Duration::from_millis(1);
        assert!(status.visible(after_expiry).is_none());
    }

    #[test]
    fn clean_engine_reports_nothing() {
        let mut health = EngineHealth::default();
        let now = Instant::now();
        assert_eq!(health.observe(EngineStatsSnapshot::default(), now), None);
        assert_eq!(health.nonzero().count(), 0);
        assert!(!health.is_fresh(now));
    }

    #[test]
    fn rising_counter_is_reported_once_and_kept_in_the_summary() {
        let mut health = EngineHealth::default();
        let now = Instant::now();
        let first = EngineStatsSnapshot {
            mic_samples_dropped: 28,
            ..EngineStatsSnapshot::default()
        };
        assert_eq!(
            health.observe(first, now).as_deref(),
            Some("Engine: 28 mic samples dropped")
        );
        assert!(health.is_fresh(now));
        // Unchanged counters are not reported again, but stay visible.
        assert_eq!(health.observe(first, now), None);
        assert_eq!(health.nonzero().collect::<Vec<_>>(), vec![("mic", 28)]);
        assert!(!health.is_fresh(now + ERROR_LIFETIME));

        // Only the rise is reported.
        let second = EngineStatsSnapshot {
            mic_samples_dropped: 30,
            ..first
        };
        assert_eq!(
            health.observe(second, now).as_deref(),
            Some("Engine: 2 mic samples dropped")
        );
    }

    #[test]
    fn several_rises_name_the_worst_and_count_the_rest() {
        let mut health = EngineHealth::default();
        let latest = EngineStatsSnapshot {
            display_frames_dropped: 3,
            params_rejected: 1,
            disk_samples_dropped: 512,
            ..EngineStatsSnapshot::default()
        };
        assert_eq!(
            health.observe(latest, Instant::now()).as_deref(),
            Some(
                "Engine: 512 recorded audio lost (the WAV file has gaps) \
                 (+2 other problems, see header)"
            )
        );
        assert_eq!(
            health.nonzero().collect::<Vec<_>>(),
            vec![("rec gaps", 512), ("params", 1), ("screen", 3)]
        );
    }

    #[test]
    fn every_counter_is_covered() {
        // A snapshot with every counter distinct: each must be read by
        // exactly one entry, so a new counter cannot be silently missed.
        let latest = EngineStatsSnapshot {
            mic_samples_dropped: 1,
            analysis_samples_dropped: 2,
            analysis_results_dropped: 3,
            disk_samples_dropped: 4,
            display_frames_dropped: 5,
            callback_frees: 6,
            commands_dropped: 7,
            disk_commands_dropped: 8,
            params_rejected: 9,
            clips_rejected: 10,
            disk_errors: 11,
            desk_requests_dropped: 12,
            desk_syncs_rejected: 13,
            tracks_rejected: 14,
            takes_unavailable: 15,
            processors_rejected: 16,
            take_samples_dropped: 17,
        };
        let mut read: Vec<u64> = COUNTERS.iter().map(|c| (c.read)(&latest)).collect();
        read.sort_unstable();
        assert_eq!(read, (1..=17).collect::<Vec<u64>>());
    }

    #[test]
    fn command_queue_full_is_reported() {
        let mut status = StatusLine::default();
        assert!(!status.report("Mute", Err(kazoo_core::Error::CommandQueueFull)));
        let msg = status.visible(Instant::now()).unwrap();
        assert_eq!(
            msg.text,
            "Mute failed: engine busy: command queue full, command dropped"
        );
    }
}
