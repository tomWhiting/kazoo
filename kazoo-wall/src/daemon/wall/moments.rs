//! Ordered tree requests and their deferred wall-side actions.

#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use super::{Bucket, Kept, Outcome, Source, Wall, encode, error, save_error, unchanged};
use super::{SAVE_BURST, SAVE_REFILL_PER_SECOND, SAVES_BURST, SAVES_REFILL_PER_SECOND};
use crate::patch::Patch;
use crate::protocol::tree::MAX_MOMENT;
use crate::protocol::{ErrorCode, Event, TreePlace, WallError, What, valid_save_name};
use crate::store::{AUTO_SAVES_KEPT, ReadSave, SaveError};
use crate::tree::thread::{Answered, HereError, TreeItem};

/// Tree tickets occupy the upper half so speech and loop tickets cannot collide.
pub(super) const TREE_TICKETS: u64 = 1 << 63;
/// A commit read takes milliseconds; five seconds exposes a stuck tree before
/// a seat's ten-second socket deadline.
pub const TREE_ANSWER_WITHIN: Duration = Duration::from_secs(5);
/// Three resolutions bound retries while other seats keep changing the wall.
pub const GO_TRIES: u8 = 3;

#[derive(Debug)]
pub(super) struct Waiting {
    seat: String,
    console: bool,
    asked: Instant,
    what: Asked,
}

#[derive(Debug)]
enum Asked {
    Mark,
    List,
    Go {
        made: u64,
        tries: u8,
        moment: String,
    },
    Back {
        go: u64,
        backup: String,
        fingerprint: String,
        back_to: TreePlace,
        made: u64,
        tries: u8,
    },
}

impl Asked {
    const fn label(&self) -> &'static str {
        match self {
            Self::Mark => "mark",
            Self::List => "the tree",
            Self::Go { .. } => "go",
            Self::Back { .. } => "undo of a go",
        }
    }
}

fn here_error(err: &HereError) -> WallError {
    let code = match err {
        HereError::UnknownMoment(_) => ErrorCode::UnknownMoment,
        HereError::MarkExists(_) => ErrorCode::MarkExists,
        HereError::Full(_) => ErrorCode::Full,
        HereError::Broken(_) => ErrorCode::BrokenSave,
        HereError::Down(_) => ErrorCode::Internal,
    };
    error(code, err.to_string())
}

impl Wall {
    pub(super) fn remember_tree_moment(&mut self, source: &Source, fingerprint: String) {
        self.tree_kept = match source {
            Source::Moment { to, .. } => Some((to.commit.clone(), fingerprint)),
            Source::Back { to, kept, .. } if kept.name.starts_with("tree:") => {
                Some((to.commit.clone(), fingerprint))
            }
            Source::Save(_) | Source::Empty | Source::Undo { .. } | Source::Back { .. } => None,
        };
    }

    pub(super) fn keeping_before(
        &self,
        source: &Source,
    ) -> Result<(Kept, Option<Vec<u8>>), SaveError> {
        if let Source::Moment { from, .. } | Source::Back { from, .. } = source {
            if let Some((commit, kept)) = &self.tree_kept {
                let fingerprint = self.patch.fingerprint();
                if *commit == from.commit && !fingerprint.is_empty() && fingerprint == *kept {
                    return Ok((
                        Kept {
                            name: format!("tree:{:.12}", from.commit),
                            hash: fingerprint,
                        },
                        None,
                    ));
                }
            }
        }
        self.keeping(source.how())
    }

    pub(super) fn read_backup(
        &mut self,
        seq: u64,
        backup: &str,
        fingerprint: &str,
    ) -> Result<ReadSave, WallError> {
        let gone = |why: &str| {
            error(
                ErrorCode::NotAllowed,
                format!(
                    "the wall change {seq} replaced was kept as \"{backup}\", {why}, so it cannot \
                     come back exactly"
                ),
            )
        };
        let read = match self.saves_store()?.read_save(backup) {
            Ok(read) => read,
            Err(SaveError::Unknown(_)) => {
                return Err(gone(&format!(
                    "which is no longer among the saves (the wall's own are let go beyond the \
                     newest {AUTO_SAVES_KEPT}, and a console can delete any)"
                )));
            }
            Err(SaveError::Broken { reason, .. }) => {
                return Err(gone(&format!("which cannot be read ({reason})")));
            }
            Err(err) => return Err(save_error(&err)),
        };
        if fingerprint.is_empty() || read.hash != fingerprint {
            return Err(gone("and that save has changed since"));
        }
        Ok(read)
    }

    fn tree_ready(&self) -> Result<(), WallError> {
        let Some(tree) = &self.tree else {
            return Err(error(
                ErrorCode::Internal,
                "this wall keeps no tree yet (kazoo wall tree import plants it); nothing was changed",
            ));
        };
        if let Some(why) = tree.health().error {
            return Err(error(
                ErrorCode::Internal,
                format!("the tree is stopped ({why}); nothing was changed"),
            ));
        }
        Ok(())
    }

    fn tree_send(&mut self, job: TreeItem) -> Result<(), WallError> {
        self.tree_ready()?;
        if self.tree.as_mut().is_some_and(|tree| tree.ask(job)) {
            Ok(())
        } else {
            Err(error(
                ErrorCode::SlowDown,
                "the tree is behind (its queue is full); ask again in a moment",
            ))
        }
    }

    fn tree_ticket(&mut self) -> Result<u64, WallError> {
        let ticket = self.next_tree_ticket;
        self.next_tree_ticket = ticket.checked_add(1).ok_or_else(|| {
            error(
                ErrorCode::Internal,
                "the tree's answer tickets are exhausted; nothing was changed",
            )
        })?;
        Ok(ticket)
    }

    fn tree_pending(&mut self, ticket: u64, waiting: Waiting) -> Outcome {
        self.tree_waiting.insert(ticket, waiting);
        Outcome {
            pending: Some(ticket),
            ..unchanged(serde_json::Value::Null)
        }
    }

    pub(super) fn mark(
        &mut self,
        (seat, console): (&str, bool),
        name: &str,
        now: Instant,
    ) -> Result<Outcome, WallError> {
        self.tree_ready()?;
        if !valid_save_name(name) {
            return Err(error(
                ErrorCode::BadName,
                "a mark's name is 1 to 64 of A-Z a-z 0-9 space _ . -, starting with a letter or digit",
            ));
        }
        if !console
            && !self
                .mark_buckets
                .entry(seat.into())
                .or_insert_with(|| Bucket::of(now, SAVE_BURST, SAVE_REFILL_PER_SECOND))
                .take(now)
        {
            return Err(error(
                ErrorCode::SlowDown,
                format!("{seat} has marked three moments in a minute; wait a moment"),
            ));
        }
        let waiting = Waiting {
            seat: seat.into(),
            console,
            asked: now,
            what: Asked::Mark,
        };
        let ticket = self.tree_ticket()?;
        if let Err(err) = self.tree_send(TreeItem::Mark {
            ticket,
            name: name.into(),
            console,
        }) {
            self.refund_tree(&waiting, &err, false);
            return Err(err);
        }
        Ok(self.tree_pending(ticket, waiting))
    }

    pub(super) fn go(
        &mut self,
        (seat, console): (&str, bool),
        moment: &str,
        now: Instant,
    ) -> Result<Outcome, WallError> {
        self.tree_ready()?;
        if moment.is_empty() || moment.len() > MAX_MOMENT || moment.chars().any(char::is_control) {
            return Err(error(
                ErrorCode::BadRequest,
                "a moment is a branch (trunk, bud/…), a mark, #<change> or a commit id",
            ));
        }
        let ticket = self.tree_ticket()?;
        self.tree_send(TreeItem::Resolve {
            ticket,
            moment: moment.into(),
        })?;
        Ok(self.tree_pending(
            ticket,
            Waiting {
                seat: seat.into(),
                console,
                asked: now,
                what: Asked::Go {
                    made: self.made,
                    tries: 0,
                    moment: moment.into(),
                },
            },
        ))
    }

    pub(super) fn tree_allow(&mut self, seat: &str, now: Instant) -> Result<(), WallError> {
        if self
            .tree_buckets
            .entry(seat.into())
            .or_insert_with(|| Bucket::of(now, SAVES_BURST, SAVES_REFILL_PER_SECOND))
            .take(now)
        {
            Ok(())
        } else {
            Err(error(
                ErrorCode::SlowDown,
                format!(
                    "{seat} has asked for the tree more than 10 times in a few seconds; wait a moment"
                ),
            ))
        }
    }

    pub(super) fn tree(
        &mut self,
        seat: &str,
        recent: Option<u32>,
        now: Instant,
    ) -> Result<Outcome, WallError> {
        self.tree_ready()?;
        let ticket = self.tree_ticket()?;
        self.tree_send(TreeItem::List { ticket, recent })?;
        Ok(self.tree_pending(
            ticket,
            Waiting {
                seat: seat.into(),
                console: false,
                asked: now,
                what: Asked::List,
            },
        ))
    }

    pub(super) fn undo_go(
        &mut self,
        (seat, console): (&str, bool),
        go: u64,
        backup: String,
        fingerprint: String,
        back_to: TreePlace,
    ) -> Result<Outcome, WallError> {
        self.tree_ready()?;
        let ticket = self.tree_ticket()?;
        let job = if backup.starts_with("tree:") {
            if backup != format!("tree:{:.12}", back_to.commit) {
                return Err(error(
                    ErrorCode::BrokenSave,
                    "the go's tree backup names a different moment; nothing was changed",
                ));
            }
            TreeItem::Resolve {
                ticket,
                moment: back_to.commit.clone(),
            }
        } else {
            TreeItem::Here { ticket }
        };
        self.tree_send(job)?;
        Ok(self.tree_pending(
            ticket,
            Waiting {
                seat: seat.into(),
                console,
                asked: Instant::now(),
                what: Asked::Back {
                    go,
                    backup,
                    fingerprint,
                    back_to,
                    made: self.made,
                    tries: 0,
                },
            },
        ))
    }

    fn refund_tree(&mut self, waiting: &Waiting, err: &WallError, expired: bool) {
        let refundable = err.code != ErrorCode::Internal
            || (expired && matches!(waiting.what, Asked::Go { .. } | Asked::Back { .. }));
        if waiting.console || !refundable {
            return;
        }
        let buckets = match waiting.what {
            Asked::Mark => &mut self.mark_buckets,
            Asked::Go { .. } | Asked::Back { .. } => &mut self.load_buckets,
            Asked::List => return,
        };
        if let Some(bucket) = buckets.get_mut(&waiting.seat) {
            bucket.refund();
        }
    }

    fn retry_tree(&mut self, ticket: u64, waiting: &mut Waiting) -> Result<bool, WallError> {
        let (made, tries, job) = match &mut waiting.what {
            Asked::Go {
                made,
                tries,
                moment,
            } => (
                made,
                tries,
                TreeItem::Resolve {
                    ticket,
                    moment: moment.clone(),
                },
            ),
            Asked::Back {
                made,
                tries,
                backup,
                back_to,
                ..
            } => (
                made,
                tries,
                if backup.starts_with("tree:") {
                    TreeItem::Resolve {
                        ticket,
                        moment: back_to.commit.clone(),
                    }
                } else {
                    TreeItem::Here { ticket }
                },
            ),
            Asked::Mark | Asked::List => return Ok(false),
        };
        if *made == self.made {
            return Ok(false);
        }
        *tries += 1;
        if *tries >= GO_TRIES {
            return Err(error(
                ErrorCode::SlowDown,
                "the wall kept changing while the tree found the moment; ask again",
            ));
        }
        *made = self.made;
        self.tree_send(job)?;
        Ok(true)
    }

    fn act_tree(&mut self, waiting: &Waiting, answer: Answered) -> Result<Outcome, WallError> {
        let seat = (waiting.seat.as_str(), waiting.console);
        match (&waiting.what, answer) {
            (Asked::Mark, Answered::Marked(mark)) => Ok(Outcome {
                result: encode(&mark)?,
                change: None,
                event: Some(Event::Marked {
                    seat: waiting.seat.clone(),
                    name: mark.name,
                    tag: mark.tag,
                    at: mark.at,
                }),
                pending: None,
            }),
            (Asked::List, Answered::Listed(mut tree)) => {
                tree.revision = self.revision();
                encode(&tree).map(unchanged)
            }
            (Asked::Go { .. }, Answered::Resolved { to, here, wall }) => {
                if to.commit == here.commit {
                    let place = here
                        .branch
                        .as_ref()
                        .or(here.mark.as_ref())
                        .unwrap_or(&here.commit);
                    return Err(error(
                        ErrorCode::NotAllowed,
                        format!("the wall is at that moment already ({place}); nothing changed"),
                    ));
                }
                let (incoming, notes) =
                    Patch::load(&wall).map_err(|err| error(ErrorCode::BrokenSave, err))?;
                self.replace(seat, incoming, &Source::Moment { to, from: here }, notes)
            }
            (
                Asked::Back {
                    go,
                    backup,
                    fingerprint,
                    back_to,
                    ..
                },
                Answered::Resolved { to, here, wall },
            ) if backup.starts_with("tree:") => {
                let (incoming, notes) =
                    Patch::load(&wall).map_err(|err| error(ErrorCode::BrokenSave, err))?;
                if to.commit != back_to.commit
                    || fingerprint.is_empty()
                    || incoming.fingerprint() != *fingerprint
                {
                    return Err(error(
                        ErrorCode::BrokenSave,
                        "the go's tree backup no longer holds the same wall; nothing was changed",
                    ));
                }
                self.replace(
                    seat,
                    incoming,
                    &Source::Back {
                        go: *go,
                        kept: Kept {
                            name: backup.clone(),
                            hash: fingerprint.clone(),
                        },
                        to: back_to.clone(),
                        from: here,
                    },
                    notes,
                )
            }
            (
                Asked::Back {
                    go,
                    backup,
                    fingerprint,
                    back_to,
                    ..
                },
                Answered::Here(here),
            ) => {
                let read = self.read_backup(*go, backup, fingerprint)?;
                self.replace(
                    seat,
                    *read.patch,
                    &Source::Back {
                        go: *go,
                        kept: Kept {
                            name: read.name,
                            hash: read.hash,
                        },
                        to: back_to.clone(),
                        from: here,
                    },
                    read.notes,
                )
            }
            _ => Err(error(
                ErrorCode::Internal,
                "the tree answered a different request; nothing was changed",
            )),
        }
    }

    /// Complete tree requests without waiting; expired requests are answered once.
    pub fn tree_answers(&mut self, now: Instant) -> Vec<(u64, Result<Outcome, WallError>)> {
        let mut replies = Vec::new();
        for (ticket, _, answer) in std::mem::take(&mut self.tree_answered) {
            let Some(mut waiting) = self.tree_waiting.remove(&ticket) else {
                continue;
            };
            if now.saturating_duration_since(waiting.asked) >= TREE_ANSWER_WITHIN {
                let err = Self::tree_expired(&waiting);
                self.refund_tree(&waiting, &err, true);
                replies.push((ticket, Err(err)));
                continue;
            }
            let result = match answer {
                Err(err) => Err(here_error(&err)),
                Ok(answer) => match self.retry_tree(ticket, &mut waiting) {
                    Ok(true) => {
                        self.tree_waiting.insert(ticket, waiting);
                        continue;
                    }
                    Ok(false) => self.act_tree(&waiting, answer),
                    Err(err) => Err(err),
                },
            };
            if let Err(err) = &result {
                self.refund_tree(&waiting, err, false);
            }
            replies.push((ticket, result));
        }
        let expired: Vec<_> = self
            .tree_waiting
            .iter()
            .filter_map(|(&ticket, waiting)| {
                (now.saturating_duration_since(waiting.asked) >= TREE_ANSWER_WITHIN)
                    .then_some(ticket)
            })
            .collect();
        for ticket in expired {
            if let Some(waiting) = self.tree_waiting.remove(&ticket) {
                let err = Self::tree_expired(&waiting);
                self.refund_tree(&waiting, &err, true);
                replies.push((ticket, Err(err)));
            }
        }
        replies
    }

    fn tree_expired(waiting: &Waiting) -> WallError {
        error(
            ErrorCode::Internal,
            format!(
                "the tree did not answer within 5 s ({}): nothing was changed, and the wall plays on",
                waiting.what.label()
            ),
        )
    }

    pub(super) fn save_tree_mark(&mut self, name: &str, at: &str, notes: &mut Vec<String>) {
        if let Err(err) = self.tree_send(TreeItem::SaveMark {
            name: name.into(),
            at: at.into(),
        }) {
            notes.push(format!("it is not marked in the tree ({})", err.message));
        }
    }
}

impl Source {
    pub(super) fn replacement(
        &self,
        backup: &Kept,
        incoming: &Patch,
        playing: f64,
        console: bool,
    ) -> What {
        let modules = u32::try_from(incoming.modules().len()).unwrap_or(u32::MAX);
        let cables = u32::try_from(incoming.cables().len()).unwrap_or(u32::MAX);
        match self {
            Self::Moment { to, from } | Self::Back { to, from, .. } => What::Go {
                to: to.clone(),
                from: from.clone(),
                backup: backup.name.clone(),
                fingerprint: backup.hash.clone(),
                modules,
                cables,
                tempo: playing,
                by_console: console,
            },
            Self::Save(_) | Self::Empty | Self::Undo { .. } => What::Load {
                save: self.save().map(str::to_string),
                backup: backup.name.clone(),
                fingerprint: backup.hash.clone(),
                modules,
                cables,
                tempo: playing,
                by_console: console,
            },
        }
    }
}
