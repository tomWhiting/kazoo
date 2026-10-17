//! What the mouth has lost: every loss it counts (its own [`Stats`], its
//! voice judge's and the desk link's), for a header row of its own. A rise
//! is said for ten seconds, and later rises in that time add to it; then the
//! row holds the totals, worst first. The session's totals are said once
//! more when it ends.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use kazoo_core::ipc::link::LinkStatus;
use kazoo_core::voice::judge::{JudgeCounters, JudgeStatsSnapshot};

use crate::engine::Stats;

/// How long a rise is said before the row goes back to the totals.
pub const RISE_LIFETIME: Duration = Duration::from_secs(10);

/// Every loss the mouth counts, at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Losses {
    /// Mic samples lost because the input ring was full.
    pub mic_dropped: u64,
    /// Mic samples skipped to catch the input up when it ran too far ahead.
    pub mic_skipped: u64,
    /// Voice frames the screen missed.
    pub display_dropped: u64,
    /// Voice frames the desk could carry but whose queue was full.
    pub voice_refused: u64,
    /// Audio blocks sent in the instant the desk went away.
    pub audio_refused: u64,
    /// Transport syncs from the desk that could not be followed.
    pub desk_lost: u64,
    /// Effect faults: a pedal's effect was reset after a bad sample.
    pub fx_faults: u64,
    /// Effect changes the engine refused; the UI sends only valid ones.
    pub fx_refused: u64,
    /// Audio blocks the desk link dropped (its ring full, or a bad block).
    pub desk_blocks_dropped: u64,
    /// Messages the desk link dropped.
    pub desk_messages_dropped: u64,
    /// The voice judge's own.
    pub judge: JudgeStatsSnapshot,
    /// Whether the voice judge's thread has stopped while the mouth plays.
    pub judge_stopped: bool,
}

impl Losses {
    /// Read every counter. `desk` is the link's status while there is one.
    /// Every field of [`Stats`] is named here, so a counter added there must
    /// be decided on here.
    #[must_use]
    pub fn read(
        stats: &Stats,
        judge: &JudgeCounters,
        judge_stopped: bool,
        desk: Option<&LinkStatus>,
    ) -> Self {
        let Stats {
            capture: _,
            fx_latency: _,
            mic_dropped,
            mic_skipped,
            display_dropped,
            voice_refused,
            audio_refused,
            synth_fault: _,
            shutdown: _,
            fx_faults,
            fx_refused,
            desk_lost,
            own_bpm: _,
            desk_bpm: _,
            fx_moving: _,
            fx_input: _,
            fx_output: _,
            mic_rms: _,
            output_rms: _,
        } = stats;
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        Self {
            mic_dropped: load(mic_dropped),
            mic_skipped: load(mic_skipped),
            display_dropped: load(display_dropped),
            voice_refused: load(voice_refused),
            audio_refused: load(audio_refused),
            desk_lost: load(desk_lost),
            fx_faults: load(fx_faults),
            fx_refused: load(fx_refused),
            desk_blocks_dropped: desk.map_or(0, |status| status.blocks_dropped),
            desk_messages_dropped: desk.map_or(0, |status| status.messages_dropped),
            judge: judge.snapshot(),
            judge_stopped,
        }
    }
}

/// One counter, as the row names it.
struct Counter {
    /// Its name in the row's totals.
    short: &'static str,
    /// What one of it is, for a rise of one.
    one: &'static str,
    /// What many are, for any other rise.
    many: &'static str,
    /// Read this counter from the losses.
    read: fn(&Losses) -> u64,
    /// Whether a rise in it is said with the first pYIN failure's reason.
    says_pitch_failure: bool,
}

/// Every counter in [`Losses`], worst first. A gap in what is heard comes
/// before a gap in what is seen: the mic (a gap in the sound and the
/// judge's input both), then the desk's audio (heard as silence when on the
/// desk), then the judge (pitch late or lost), then the effects, then what
/// only the desk or the screen misses.
const COUNTERS: [Counter; 13] = [
    Counter {
        short: "mic",
        one: "mic sample dropped",
        many: "mic samples dropped",
        read: |l| l.mic_dropped,
        says_pitch_failure: false,
    },
    Counter {
        short: "mic skip",
        one: "mic sample skipped to catch up",
        many: "mic samples skipped to catch up",
        read: |l| l.mic_skipped,
        says_pitch_failure: false,
    },
    Counter {
        short: "desk audio",
        one: "audio block the desk link dropped (a lost backlog counts once)",
        many: "audio blocks the desk link dropped (a lost backlog counts once)",
        read: |l| l.desk_blocks_dropped,
        says_pitch_failure: false,
    },
    Counter {
        short: "pitch in",
        one: "sample the voice judge missed",
        many: "samples the voice judge missed",
        read: |l| l.judge.samples_dropped,
        says_pitch_failure: false,
    },
    Counter {
        short: "pyin",
        one: "pYIN run failed, read as unvoiced (engine fault)",
        many: "pYIN runs failed, read as unvoiced (engine fault)",
        read: |l| l.judge.pitch_runs_failed,
        says_pitch_failure: true,
    },
    Counter {
        short: "pitch out",
        one: "pitch or onset verdict dropped",
        many: "pitch or onset verdicts dropped",
        read: |l| l.judge.results_dropped,
        says_pitch_failure: false,
    },
    Counter {
        short: "fx",
        one: "effect fault (the effect was reset)",
        many: "effect faults (the effect was reset)",
        read: |l| l.fx_faults,
        says_pitch_failure: false,
    },
    Counter {
        short: "desk msgs",
        one: "message the desk link dropped",
        many: "messages the desk link dropped",
        read: |l| l.desk_messages_dropped,
        says_pitch_failure: false,
    },
    Counter {
        short: "desk voice",
        one: "voice frame the desk's queue had no room for",
        many: "voice frames the desk's queue had no room for",
        read: |l| l.voice_refused,
        says_pitch_failure: false,
    },
    Counter {
        short: "desk gone",
        one: "audio block sent as the desk went away",
        many: "audio blocks sent as the desk went away",
        read: |l| l.audio_refused,
        says_pitch_failure: false,
    },
    Counter {
        short: "desk sync",
        one: "desk transport sync not followed",
        many: "desk transport syncs not followed",
        read: |l| l.desk_lost,
        says_pitch_failure: false,
    },
    Counter {
        short: "fx refused",
        one: "effect change refused (engine fault)",
        many: "effect changes refused (engine fault)",
        read: |l| l.fx_refused,
        says_pitch_failure: false,
    },
    Counter {
        short: "screen",
        one: "voice frame the screen missed",
        many: "voice frames the screen missed",
        read: |l| l.display_dropped,
        says_pitch_failure: false,
    },
];

/// What the row says now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The words.
    pub text: String,
    /// Whether something rose within [`RISE_LIFETIME`].
    pub fresh: bool,
}

/// The losses as last seen, and the rise being said.
#[derive(Debug, Clone, Default)]
pub struct Health {
    seen: Losses,
    /// Whether the judge was ever seen stopped: the losses read after it is
    /// joined cannot tell, and the report must still say so.
    judge_stopped: bool,
    /// The totals when the rise being said began, and when it ends.
    rising: Option<(Losses, Instant)>,
}

impl Health {
    /// Take the latest losses at `now`. A rise starts ten seconds of saying
    /// it; a later rise in that time adds to what is said and starts the ten
    /// seconds again.
    pub fn observe(&mut self, latest: Losses, now: Instant) {
        let rose = COUNTERS
            .iter()
            .any(|counter| (counter.read)(&latest) > (counter.read)(&self.seen));
        if rose {
            let since = match self.rising {
                Some((since, until)) if now < until => since,
                _ => self.seen,
            };
            self.rising = Some((since, now + RISE_LIFETIME));
        }
        self.seen = latest;
        self.judge_stopped |= latest.judge_stopped;
    }

    /// The row: the rise being said, or the totals, or nothing at all when
    /// nothing is lost and the judge runs.
    #[must_use]
    pub fn row(&self, now: Instant, pitch_failure: Option<&str>) -> Option<Row> {
        let stopped = self.judge_stopped.then_some("the voice judge has stopped");
        if let Some((since, until)) = self.rising
            && now < until
        {
            let mut rise = describe(&since, &self.seen, pitch_failure);
            // Whatever the rise does not name (others rising with it, or
            // losses from before it) is in the totals, which stay in sight
            // however long the losses keep rising.
            let named = rises(&since, &self.seen).len();
            let lost = COUNTERS
                .iter()
                .filter(|counter| (counter.read)(&self.seen) > 0)
                .count();
            if (named > 1 || lost > named)
                && let Some(totals) = self.totals()
            {
                rise = format!("{rise} · lost so far: {totals}");
            }
            let text = stopped.map_or_else(|| rise.clone(), |stopped| format!("{stopped}; {rise}"));
            return Some(Row { text, fresh: true });
        }
        let totals = self.totals();
        let text = match (stopped, totals) {
            (None, None) => return None,
            (Some(stopped), None) => stopped.to_owned(),
            (None, Some(totals)) => format!("lost so far: {totals}"),
            (Some(stopped), Some(totals)) => format!("{stopped}; lost so far: {totals}"),
        };
        Some(Row {
            text,
            fresh: self.judge_stopped,
        })
    }

    /// Every non-zero total, worst first: "mic 28 · pyin 1".
    #[must_use]
    pub fn totals(&self) -> Option<String> {
        let parts: Vec<String> = COUNTERS
            .iter()
            .filter_map(|counter| {
                let total = (counter.read)(&self.seen);
                (total > 0).then(|| format!("{} {total}", counter.short))
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// The session's losses in words, for when it ends, or `None` if it
    /// lost nothing.
    #[must_use]
    pub fn report(&self, pitch_failure: Option<&str>) -> Option<String> {
        let all = describe_all(&Losses::default(), &self.seen, pitch_failure);
        let stopped = self.judge_stopped.then_some("the voice judge stopped");
        match (stopped, all) {
            (None, None) => None,
            (Some(stopped), None) => Some(format!("kazoo-mouth: {stopped}")),
            (None, Some(all)) => Some(format!("kazoo-mouth: lost {all}")),
            (Some(stopped), Some(all)) => Some(format!("kazoo-mouth: {stopped}; lost {all}")),
        }
    }
}

fn amount(counter: &Counter, count: u64) -> String {
    let noun = if count == 1 {
        counter.one
    } else {
        counter.many
    };
    format!("{count} {noun}")
}

/// What rose between `before` and `after`: the worst rise and how much,
/// how many others rose (they are in the totals), and the first pYIN
/// failure's reason when a run failed.
fn describe(before: &Losses, after: &Losses, pitch_failure: Option<&str>) -> String {
    let rises: Vec<(&Counter, u64)> = rises(before, after);
    let Some(&(worst, count)) = rises.first() else {
        return String::new();
    };
    let more = match rises.len() - 1 {
        0 => String::new(),
        1 => " (+1 other)".to_owned(),
        n => format!(" (+{n} others)"),
    };
    let (lead, tail) = reason(&rises, worst, pitch_failure);
    format!("{}{lead}{more}{tail}", amount(worst, count))
}

/// Every rise between `before` and `after`, worst first, in words.
fn describe_all(before: &Losses, after: &Losses, pitch_failure: Option<&str>) -> Option<String> {
    let rises = rises(before, after);
    let worst = rises.first()?.0;
    let (lead, tail) = reason(&rises, worst, pitch_failure);
    let mut words: Vec<String> = rises
        .iter()
        .map(|&(counter, count)| amount(counter, count))
        .collect();
    words[0].push_str(&lead);
    Some(words.join(", ") + &tail)
}

fn rises(before: &Losses, after: &Losses) -> Vec<(&'static Counter, u64)> {
    COUNTERS
        .iter()
        .filter_map(|counter| {
            let rise = (counter.read)(after).saturating_sub((counter.read)(before));
            (rise > 0).then_some((counter, rise))
        })
        .collect()
}

/// The pYIN failure's reason: straight after the failure when it is the
/// worst rise, at the end when another is.
fn reason(
    rises: &[(&Counter, u64)],
    worst: &Counter,
    pitch_failure: Option<&str>,
) -> (String, String) {
    match pitch_failure {
        Some(why) if worst.says_pitch_failure => (format!(": {why}"), String::new()),
        Some(why) if rises.iter().any(|(counter, _)| counter.says_pitch_failure) => {
            (String::new(), format!("; pYIN failed: {why}"))
        }
        _ => (String::new(), String::new()),
    }
}

#[cfg(test)]
mod tests;
