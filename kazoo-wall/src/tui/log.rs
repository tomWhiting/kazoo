//! The console's live log: who did what, newest first.
//!
//! Changes come from three places: the answers to the console's own
//! requests, the feed's events (other seats), and `log` pages fetched on
//! connecting and whenever the wall's revision runs ahead of what the log
//! holds. Each change is kept once, by its sequence number, in sequence
//! order; notes (seats coming and going, faults, the link) sit where they
//! arrived.

use std::collections::{BTreeSet, VecDeque};

use kazoo_wall::protocol::Change;

/// Most lines the log keeps.
pub const CAPACITY: usize = 500;

/// What a line is about, for its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// A change this console's seat made.
    Mine,
    /// A change another seat made.
    Other,
    /// A seat came or went.
    Seat,
    /// A module produced a non-finite sample.
    Fault,
    /// The console's own news: the link, a start.
    Note,
}

/// One line of the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The change's sequence number, for changes.
    pub seq: Option<u64>,
    /// When, in Unix seconds, if known.
    pub at: Option<i64>,
    /// The sentence.
    pub text: String,
    /// What it is about.
    pub tone: Tone,
    /// The change it undid, for undos.
    pub undoes: Option<u64>,
}

/// The log's lines, newest first.
#[derive(Debug, Clone, Default)]
pub struct Log {
    lines: VecDeque<Line>,
    seen: BTreeSet<u64>,
}

impl Log {
    /// The lines, newest first.
    pub fn lines(&self) -> impl ExactSizeIterator<Item = &Line> {
        self.lines.iter()
    }

    /// Line `index`, counting from the newest.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Line> {
        self.lines.get(index)
    }

    /// How many lines.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The newest change's sequence number.
    #[must_use]
    pub fn latest_seq(&self) -> Option<u64> {
        self.seen.last().copied()
    }

    /// Add a change, made by this console's seat when `mine`. Returns
    /// whether it was new.
    pub fn add_change(&mut self, change: &Change, mine: bool) -> bool {
        if self.seen.contains(&change.seq) {
            return false;
        }
        let line = Line {
            seq: Some(change.seq),
            at: unix_seconds(&change.at),
            text: change.summary.clone(),
            tone: if mine { Tone::Mine } else { Tone::Other },
            undoes: change.undoes,
        };
        // A change newer than any held goes on top, above the notes that
        // came before it; an older one (from a fetched page) goes among
        // the changes in sequence order.
        let newest = self.seen.last().is_none_or(|&latest| change.seq >= latest);
        let position = if newest {
            0
        } else {
            self.lines
                .iter()
                .position(|line| line.seq.is_some_and(|seq| seq < change.seq))
                .unwrap_or(self.lines.len())
        };
        self.seen.insert(change.seq);
        self.lines.insert(position, line);
        self.trim();
        true
    }

    /// Add a note at the top.
    pub fn add_note(&mut self, text: String, tone: Tone, at: Option<i64>) {
        self.lines.push_front(Line {
            seq: None,
            at,
            text,
            tone,
            undoes: None,
        });
        self.trim();
    }

    /// Whether change `seq` has since been undone by a change in the log.
    #[must_use]
    pub fn is_undone(&self, seq: u64) -> bool {
        self.lines.iter().any(|line| line.undoes == Some(seq))
    }

    /// The change `z` undoes: the newest change that is not itself an undo
    /// and has not been undone, so pressing `z` again walks further back.
    #[must_use]
    pub fn last_undoable(&self) -> Option<u64> {
        self.lines
            .iter()
            .filter(|line| line.undoes.is_none())
            .filter_map(|line| line.seq)
            .find(|&seq| !self.is_undone(seq))
    }

    fn trim(&mut self) {
        while self.lines.len() > CAPACITY {
            if let Some(Line { seq: Some(seq), .. }) = self.lines.pop_back() {
                self.seen.remove(&seq);
            }
        }
    }
}

/// `2026-09-26T12:00:01Z` as Unix seconds; `None` if it is not that shape.
#[must_use]
pub fn unix_seconds(at: &str) -> Option<i64> {
    let bytes = at.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || !at.ends_with('Z')
    {
        return None;
    }
    let field = |from: usize, to: usize| digits(at.get(from..to)?);
    let (year, month, day) = (field(0, 4)?, field(5, 7)?, field(8, 10)?);
    let (hour, minute, second) = (field(11, 13)?, field(14, 16)?, field(17, 19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// The decimal number in `text`, which must be all ASCII digits.
fn digits(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(
        text.bytes()
            .fold(0_i64, |total, b| total * 10 + i64::from(b - b'0')),
    )
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// algorithm).
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// How long ago `at` was, from `now` (both Unix seconds), in a few
/// characters: `now`, `42s`, `5m`, `3h`, `2d`.
#[must_use]
pub fn age(at: i64, now: i64) -> String {
    let seconds = now.saturating_sub(at).max(0);
    match seconds {
        0..=4 => "now".to_string(),
        5..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kazoo_wall::protocol::What;

    fn change(seq: u64, seat: &str, undoes: Option<u64>) -> Change {
        Change {
            seq,
            at: "2026-09-26T12:00:01Z".to_string(),
            seat: seat.to_string(),
            what: What::Tempo {
                from: 120.0,
                to: 96.0,
                desk: false,
            },
            summary: format!("{seat} set the tempo ({seq})"),
            undoes,
        }
    }

    #[test]
    fn changes_keep_sequence_order_once_each() {
        let mut log = Log::default();
        assert!(log.add_change(&change(5, "Tom", None), true));
        assert!(log.add_change(&change(7, "Waffles", None), false));
        log.add_note("Vesper joined".to_string(), Tone::Seat, None);
        // An older change fetched later goes below the newer ones.
        assert!(log.add_change(&change(6, "Vesper", None), false));
        assert!(!log.add_change(&change(7, "Waffles", None), false));
        let order: Vec<Option<u64>> = log.lines().map(|line| line.seq).collect();
        assert_eq!(order, vec![None, Some(7), Some(6), Some(5)]);
        assert_eq!(log.latest_seq(), Some(7));
        assert_eq!(log.get(1).map(|line| line.tone), Some(Tone::Other));
        assert_eq!(log.get(3).map(|line| line.tone), Some(Tone::Mine));
    }

    #[test]
    fn undo_walks_back_past_undos_and_undone_changes() {
        let mut log = Log::default();
        log.add_change(&change(1, "Tom", None), true);
        log.add_change(&change(2, "Waffles", None), false);
        assert_eq!(log.last_undoable(), Some(2));
        // Change 3 undid 2: the next undo is 1, not 3 (which would redo 2).
        log.add_change(&change(3, "Tom", Some(2)), true);
        assert!(log.is_undone(2));
        assert_eq!(log.last_undoable(), Some(1));
        log.add_change(&change(4, "Tom", Some(1)), true);
        assert_eq!(log.last_undoable(), None);
    }

    #[test]
    fn the_log_is_capped_and_forgets_what_it_drops() {
        let mut log = Log::default();
        for seq in 1..=(CAPACITY as u64 + 10) {
            log.add_change(&change(seq, "Tom", None), true);
        }
        assert_eq!(log.len(), CAPACITY);
        assert_eq!(log.latest_seq(), Some(CAPACITY as u64 + 10));
        assert!(log.add_change(&change(1, "Tom", None), true));
        assert_eq!(log.len(), CAPACITY);
    }

    #[test]
    fn timestamps_read_as_unix_seconds_and_ages() {
        assert_eq!(unix_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_seconds("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(unix_seconds("2026-09-26T12:00:01Z"), Some(1_790_424_001));
        assert_eq!(unix_seconds("2026-13-26T12:00:01Z"), None);
        assert_eq!(unix_seconds("yesterday"), None);
        assert_eq!(unix_seconds("2026-09-2xT12:00:01Z"), None);
        assert_eq!(age(100, 101), "now");
        assert_eq!(age(100, 142), "42s");
        assert_eq!(age(0, 300), "5m");
        assert_eq!(age(0, 3 * 3_600), "3h");
        assert_eq!(age(0, 2 * 86_400), "2d");
        assert_eq!(age(500, 100), "now");
    }
}
