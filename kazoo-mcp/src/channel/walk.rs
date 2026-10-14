use kazoo_wall::change::describe;
use kazoo_wall::protocol::{Change, What};

use super::{Duration, Instant, clean, who};

const WALK_REST: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Go {
    seq: u64,
    seat: String,
    summary: String,
    place: String,
}

#[derive(Debug)]
struct Walk {
    seat: String,
    count: u64,
    summary: String,
    place: String,
    last: Instant,
}

impl Walk {
    fn line(self) -> String {
        if self.count == 1 {
            return self.summary;
        }
        format!(
            "{} walked the tree: {} moments, now at {}",
            who(self.seat),
            self.count,
            self.place
        )
    }
}

/// Where a go took the wall, in the wall's own words for it: `the moment
/// after change 6825 (140 modules, 189 cables, 96 BPM)`. Read from the
/// structured go, never from the summary, so a mark or branch whose name
/// holds the summary's own words cannot move where the place is cut.
fn place(what: &What) -> String {
    let words = describe(what);
    let What::Go { backup, .. } = what else {
        return clean(&words);
    };
    let saved = format!("; the wall as it was is saved as \"{backup}\"");
    words
        .strip_prefix("went to ")
        .and_then(|rest| rest.strip_suffix(&saved))
        .and_then(|rest| rest.rsplit_once(": "))
        .map_or_else(
            || clean(&words),
            |(place, size)| clean(&format!("{place} ({size})")),
        )
}

#[derive(Debug)]
pub(super) struct Walks {
    pending: Option<Go>,
    active: Vec<Walk>,
}

impl Walks {
    pub(super) const fn new() -> Self {
        Self {
            pending: None,
            active: Vec::new(),
        }
    }

    /// Hold `change` until its replacement event when it is a go; false for
    /// anything else.
    pub(super) fn changed(&mut self, change: &Change) -> bool {
        if matches!(change.what, What::Go { .. }) {
            self.pending = Some(Go {
                seq: change.seq,
                seat: change.seat.clone(),
                summary: clean(&change.summary),
                place: place(&change.what),
            });
            true
        } else {
            false
        }
    }

    /// The summary of a go held for a replacement event that never came
    /// (the line dropped between them, or another change arrived first).
    pub(super) fn orphan(&mut self) -> Option<String> {
        self.pending.take().map(|go| go.summary)
    }

    /// The wall was replaced by change `seq` of `seat`: `Some(true)` when it
    /// starts a walk, `Some(false)` when it continues one, `None` when it
    /// was not the go held (a load, or a go whose change was not heard),
    /// leaving any go held for [`Self::orphan`].
    pub(super) fn loaded(&mut self, seq: u64, seat: &str, now: Instant) -> Option<bool> {
        if !self
            .pending
            .as_ref()
            .is_some_and(|go| go.seq == seq && go.seat == seat)
        {
            return None;
        }
        let go = self.pending.take()?;
        if let Some(walk) = self.active.iter_mut().find(|walk| walk.seat == seat) {
            walk.count += 1;
            walk.summary = go.summary;
            walk.place = go.place;
            walk.last = now;
            Some(false)
        } else {
            self.active.push(Walk {
                seat: go.seat,
                count: 1,
                summary: go.summary,
                place: go.place,
                last: now,
            });
            Some(true)
        }
    }

    /// The walk of `seat`, who left the wall, told now: it has ended.
    pub(super) fn left(&mut self, seat: &str) -> Option<String> {
        let index = self.active.iter().position(|walk| walk.seat == seat)?;
        Some(self.active.remove(index).line())
    }

    pub(super) fn due(&self) -> Option<Instant> {
        self.active.iter().map(|walk| walk.last + WALK_REST).min()
    }

    pub(super) fn rest(&mut self, now: Instant) -> Vec<String> {
        let mut lines = Vec::new();
        let mut index = 0;
        while index < self.active.len() {
            if self.active[index].last + WALK_REST <= now {
                lines.push(self.active.remove(index).line());
            } else {
                index += 1;
            }
        }
        lines
    }

    pub(super) fn end(&mut self) -> Vec<String> {
        self.active.drain(..).map(Walk::line).collect()
    }
}
