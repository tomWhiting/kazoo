//! What the mouth has lost, for the footer: every loss counter in [`Stats`]
//! and its voice judge's, each non-zero one by a short name, worst first,
//! and a message the moment one rises.

use std::sync::atomic::Ordering;

use kazoo_core::voice::judge::{JudgeCounters, JudgeStatsSnapshot};

use crate::engine::Stats;

/// Every loss the mouth counts, at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Losses {
    /// Mic samples lost because the input ring was full.
    pub mic_dropped: u64,
    /// Mic samples skipped to catch the input up when it ran too far ahead.
    pub mic_skipped: u64,
    /// Voice frames the screen missed.
    pub display_dropped: u64,
    /// Voice frames the desk did not take.
    pub voice_refused: u64,
    /// Audio blocks the desk did not take while connected.
    pub audio_refused: u64,
    /// Transport syncs from the desk that could not be followed.
    pub desk_lost: u64,
    /// Effect faults: a pedal's effect was reset after a bad sample.
    pub fx_faults: u64,
    /// Effect changes refused (a kind the pedal does not have).
    pub fx_refused: u64,
    /// The voice judge's own.
    pub judge: JudgeStatsSnapshot,
}

impl Losses {
    /// Read every counter; reading takes no lock.
    #[must_use]
    pub fn read(stats: &Stats, judge: Option<&JudgeCounters>) -> Self {
        let load = |counter: &std::sync::atomic::AtomicU64| counter.load(Ordering::Relaxed);
        Self {
            mic_dropped: load(&stats.mic_dropped),
            mic_skipped: load(&stats.mic_skipped),
            display_dropped: load(&stats.display_dropped),
            voice_refused: load(&stats.voice_refused),
            audio_refused: load(&stats.audio_refused),
            desk_lost: load(&stats.desk_lost),
            fx_faults: load(&stats.fx_faults),
            fx_refused: load(&stats.fx_refused),
            judge: judge.map(JudgeCounters::snapshot).unwrap_or_default(),
        }
    }
}

/// One counter, as the footer names it.
struct Counter {
    /// Its name in the footer's summary.
    short: &'static str,
    /// What was lost, for the message.
    long: &'static str,
    read: fn(&Losses) -> u64,
    /// Whether a rise in it is said with the first pYIN failure's reason.
    says_pitch_failure: bool,
}

/// Every counter in [`Losses`], worst first: what is heard before what is
/// only seen.
const COUNTERS: [Counter; 11] = [
    Counter {
        short: "pitch in",
        long: "samples the voice judge missed",
        read: |l| l.judge.samples_dropped,
        says_pitch_failure: false,
    },
    Counter {
        short: "mic",
        long: "mic samples dropped",
        read: |l| l.mic_dropped,
        says_pitch_failure: false,
    },
    Counter {
        short: "mic skip",
        long: "mic samples skipped to catch up",
        read: |l| l.mic_skipped,
        says_pitch_failure: false,
    },
    Counter {
        short: "fx",
        long: "effect faults (the effect was reset)",
        read: |l| l.fx_faults,
        says_pitch_failure: false,
    },
    Counter {
        short: "pyin",
        long: "pYIN runs that failed, read as unvoiced (engine fault)",
        read: |l| l.judge.pitch_runs_failed,
        says_pitch_failure: true,
    },
    Counter {
        short: "pitch out",
        long: "voice judge verdicts dropped",
        read: |l| l.judge.results_dropped,
        says_pitch_failure: false,
    },
    Counter {
        short: "desk audio",
        long: "audio blocks the desk did not take",
        read: |l| l.audio_refused,
        says_pitch_failure: false,
    },
    Counter {
        short: "desk voice",
        long: "voice frames the desk did not take",
        read: |l| l.voice_refused,
        says_pitch_failure: false,
    },
    Counter {
        short: "desk sync",
        long: "desk transport syncs not followed",
        read: |l| l.desk_lost,
        says_pitch_failure: false,
    },
    Counter {
        short: "fx refused",
        long: "effect changes refused",
        read: |l| l.fx_refused,
        says_pitch_failure: false,
    },
    Counter {
        short: "screen",
        long: "voice frames the screen missed",
        read: |l| l.display_dropped,
        says_pitch_failure: false,
    },
];

/// The losses as last seen, to say each rise once and keep the totals.
#[derive(Debug, Clone, Default)]
pub struct Health {
    seen: Losses,
}

impl Health {
    /// Take the latest losses. When any rose, a message naming the worst rise
    /// and how much, how many others rose, and, when a pYIN run failed, why.
    pub fn observe(&mut self, latest: Losses, pitch_failure: Option<&str>) -> Option<String> {
        let rises: Vec<(&Counter, u64)> = COUNTERS
            .iter()
            .filter_map(|counter| {
                let rise = (counter.read)(&latest).saturating_sub((counter.read)(&self.seen));
                (rise > 0).then_some((counter, rise))
            })
            .collect();
        self.seen = latest;
        let (worst, amount) = *rises.first()?;
        let more = match rises.len() - 1 {
            0 => String::new(),
            1 => " (+1 other, see below)".to_owned(),
            n => format!(" (+{n} others, see below)"),
        };
        // Said straight after the failure when it is the worst; after the
        // others' note when it is not.
        let (lead, tail) = match pitch_failure {
            Some(why) if worst.says_pitch_failure => (format!(": {why}"), String::new()),
            Some(why) if rises.iter().any(|(counter, _)| counter.says_pitch_failure) => {
                (String::new(), format!("; pYIN failed: {why}"))
            }
            _ => (String::new(), String::new()),
        };
        Some(format!("lost: {amount} {}{lead}{more}{tail}", worst.long))
    }

    /// Every non-zero total, worst first, for the footer: "mic 28 · pyin 1".
    #[must_use]
    pub fn summary(&self) -> Option<String> {
        let parts: Vec<String> = COUNTERS
            .iter()
            .filter_map(|counter| {
                let total = (counter.read)(&self.seen);
                (total > 0).then(|| format!("{} {total}", counter.short))
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

#[cfg(test)]
mod tests;
