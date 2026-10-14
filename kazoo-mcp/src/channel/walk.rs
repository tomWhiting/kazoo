use kazoo_wall::protocol::{Change, What};

use super::{Duration, Instant, clean, who};

const WALK_REST: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Go {
    seq: u64,
    seat: String,
    summary: String,
}

#[derive(Debug)]
struct Walk {
    seat: String,
    count: u64,
    summary: String,
    last: Instant,
}

impl Walk {
    fn line(self) -> String {
        if self.count == 1 {
            return self.summary;
        }
        let words = self
            .summary
            .strip_prefix(&self.seat)
            .unwrap_or(&self.summary);
        let place = words
            .split_once("went to ")
            .or_else(|| words.split_once("went back to "))
            .map_or(words, |(_, place)| place);
        let place = place
            .split_once("; the wall as it was is saved as ")
            .map_or(place, |(place, _)| place);
        let place = place.rsplit_once(": ").map_or_else(
            || place.to_string(),
            |(place, size)| format!("{place} ({size})"),
        );
        format!(
            "{} walked the tree: {} moments, now at {place}",
            who(self.seat),
            self.count
        )
    }
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

    pub(super) fn changed(&mut self, change: &Change) -> bool {
        if matches!(change.what, What::Go { .. }) {
            self.pending = Some(Go {
                seq: change.seq,
                seat: change.seat.clone(),
                summary: clean(&change.summary),
            });
            true
        } else {
            false
        }
    }

    pub(super) fn loaded(&mut self, seq: u64, seat: &str, now: Instant) -> Option<bool> {
        let go = self.pending.take()?;
        if go.seq != seq || go.seat != seat {
            return None;
        }
        if let Some(walk) = self.active.iter_mut().find(|walk| walk.seat == seat) {
            walk.count += 1;
            walk.summary = go.summary;
            walk.last = now;
            Some(false)
        } else {
            self.active.push(Walk {
                seat: go.seat,
                count: 1,
                summary: go.summary,
                last: now,
            });
            Some(true)
        }
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
