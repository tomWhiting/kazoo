use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::protocol::{
    ErrorCode, LoadResult, MAX_SAVE_NAME, MarkKind, MarkResult, MomentView, Request, TreePlace,
    TreeResult, What, valid_save_name,
};

use super::super::worker::{Failure, Job};
use super::command::Completion;
use super::{App, Mode, SAVES_REFRESH, Tone, step_cursor};

#[derive(Debug, Clone, Default)]
pub struct TreeList {
    pub result: Option<TreeResult>,
    pub fetching: bool,
    pub stale: bool,
    pub asked: Option<Instant>,
    pub problem: Option<String>,
    pub rows: Vec<TreeRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Here,
    Behind,
    Branch,
    Mark,
    Recent,
    More,
}

impl RowKind {
    pub const fn selectable(self) -> bool {
        matches!(self, Self::Branch | Self::Mark | Self::Recent)
    }
}

#[derive(Debug, Clone)]
pub struct TreeRow {
    pub label: String,
    pub moment: String,
    pub commit: String,
    pub kind: RowKind,
}

fn number(change: Option<u64>) -> String {
    change.map_or_else(String::new, |n| format!("#{n} "))
}

fn short(commit: &str) -> String {
    commit.chars().take(12).collect()
}

fn when(at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(at).map_or_else(
        |err| format!("unreadable time: {at} ({err})"),
        |at| {
            let local = at.with_timezone(&chrono::Local);
            let format = if local.date_naive() == chrono::Local::now().date_naive() {
                "%H:%M"
            } else {
                "%Y-%m-%d"
            };
            local.format(format).to_string()
        },
    )
}

fn detail(moment: &MomentView) -> String {
    format!(
        "{}  {}  {}",
        moment
            .change
            .map_or_else(|| moment.short.clone(), |n| format!("#{n}")),
        when(&moment.at),
        moment.seat
    )
}

fn more(rows: &mut Vec<TreeRow>, count: usize, shown: usize) {
    if count > shown {
        rows.push(TreeRow {
            label: format!("and {} more", count - shown),
            moment: String::new(),
            commit: String::new(),
            kind: RowKind::More,
        });
    }
}

pub(super) fn tree_rows(result: &TreeResult) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    let behind = result.revision.saturating_sub(result.through);
    if result.here.is_none() {
        let label = if behind > 0 {
            format!(
                "the tree is {behind} {} behind and holds no moments yet",
                changes(behind)
            )
        } else {
            "the tree has no moments yet: the first change makes one".into()
        };
        rows.push(TreeRow {
            label,
            moment: String::new(),
            commit: String::new(),
            kind: RowKind::Here,
        });
        return rows;
    }
    if let Some(here) = &result.here {
        let at = format!("{}{}", number(here.change), short(&here.commit));
        let label = here.branch.as_ref().map_or_else(
            || format!("here: at {at}, off any line — the next change grows a bud"),
            |branch| format!("here: on {branch} at {at}"),
        );
        rows.push(TreeRow {
            label,
            moment: String::new(),
            commit: here.commit.clone(),
            kind: RowKind::Here,
        });
    }
    if result.through < result.revision {
        rows.push(TreeRow {
            label: format!("the tree is {behind} {} behind the wall", changes(behind)),
            moment: String::new(),
            commit: String::new(),
            kind: RowKind::Behind,
        });
    }
    let limit = crate::protocol::tree::MAX_LISTED;
    for branch in result.branches.iter().take(limit) {
        let grew = branch
            .grew_from
            .as_ref()
            .map_or_else(String::new, |from| format!(" · grew from {from}"));
        rows.push(TreeRow {
            label: format!("{}  {}{grew}", branch.name, detail(&branch.tip)),
            moment: branch.name.clone(),
            commit: branch.tip.commit.clone(),
            kind: RowKind::Branch,
        });
    }
    more(
        &mut rows,
        result.branch_count,
        result.branches.len().min(limit),
    );
    for mark in result.marks.iter().take(limit) {
        let (name, moment) = match mark.kind {
            MarkKind::Mark => (format!("“{}”", mark.name), mark.name.clone()),
            MarkKind::Save => (format!("save “{}”", mark.name), mark.tag.clone()),
        };
        rows.push(TreeRow {
            label: format!("{name}  {}", detail(&mark.moment)),
            moment,
            commit: mark.moment.commit.clone(),
            kind: RowKind::Mark,
        });
    }
    more(&mut rows, result.mark_count, result.marks.len().min(limit));
    rows.extend(result.recent.iter().map(|moment| {
        TreeRow {
            label: format!(
                "{}{}  {}: {}",
                number(moment.change),
                moment.short,
                moment.seat,
                moment.summary
            ),
            moment: moment
                .change
                .map_or_else(|| moment.commit.clone(), |n| format!("#{n}")),
            commit: moment.commit.clone(),
            kind: RowKind::Recent,
        }
    }));
    rows
}

const fn changes(count: u64) -> &'static str {
    if count == 1 { "change" } else { "changes" }
}

pub(super) fn mark_name_problem(name: &str) -> Option<String> {
    (!valid_save_name(name)).then(|| format!(
        "a mark's name is 1 to {MAX_SAVE_NAME} of A-Z a-z 0-9 space _ . -, starting with a letter or digit"
    ))
}

fn backup_words(backup: &str) -> String {
    backup.strip_prefix("tree:").map_or_else(
        || format!("the wall as it was is saved as “{backup}”"),
        |commit| format!("the wall as it was is moment {commit} in the tree"),
    )
}

fn place_words(to: &TreePlace) -> String {
    let mut place = to.mark.as_ref().map_or_else(
        || {
            to.branch.as_ref().map_or_else(
                || {
                    to.change.map_or_else(
                        || format!("moment {}", short(&to.commit)),
                        |n| format!("the moment after change {n}"),
                    )
                },
                Clone::clone,
            )
        },
        |mark| format!("mark \"{mark}\""),
    );
    if to.mark.is_some() || to.branch.is_some() {
        if let Some(n) = to.change {
            place = format!("{place}, after change {n}");
        }
    }
    place
}

pub(super) fn is_tree_result(request: &Request, value: &serde_json::Value) -> bool {
    matches!(
        request,
        Request::Mark { .. } | Request::Go { .. } | Request::Tree { .. }
    ) || matches!(request, Request::Undo { .. }) && value.get("backup").is_some()
}

impl App {
    pub(super) fn tree_completions(&self, lower: &str) -> Vec<Completion> {
        self.tree_rows()
            .iter()
            .filter(|row| {
                matches!(row.kind, RowKind::Branch | RowKind::Mark)
                    && row.moment.to_lowercase().starts_with(lower)
            })
            .map(|row| Completion {
                word: row.moment.clone(),
                note: row.label.clone(),
                open: false,
            })
            .collect()
    }

    pub(super) fn run_tree_command(
        &mut self,
        name: &str,
        rest: &str,
        now: Instant,
    ) -> Result<(), String> {
        match name {
            "mark" => {
                if let Some(problem) = mark_name_problem(rest) {
                    return Err(problem);
                }
                self.mark_as(rest, now);
            }
            "go" => {
                if rest.is_empty() {
                    return Err(format!("type {}", super::usage(name).unwrap_or_default()));
                }
                self.mode = Mode::Go {
                    moment: rest.into(),
                    label: rest.into(),
                };
            }
            "tree" => self.open_tree(now),
            _ => return Err(format!("'{name}' is not a tree command")),
        }
        Ok(())
    }

    pub(in crate::tui) const fn tree(&self) -> &TreeList {
        &self.tree
    }

    pub(in crate::tui) fn tree_rows(&self) -> &[TreeRow] {
        &self.tree.rows
    }

    pub(super) fn open_mark(&mut self, now: Instant) {
        if self.link.is_up() {
            self.status = None;
            self.mode = Mode::Mark {
                text: String::new(),
            };
        } else {
            self.say(
                Tone::Trouble,
                "the wall is not answering, so its moment cannot be marked".into(),
                now,
            );
        }
    }

    pub(super) fn mark_key(&mut self, key: KeyEvent, mut text: String, now: Instant) -> Mode {
        match key.code {
            KeyCode::Esc => {
                self.say(Tone::Info, "nothing was marked".into(), now);
                return Mode::Normal;
            }
            KeyCode::Backspace => {
                text.pop();
                self.status = None;
            }
            KeyCode::Char(c)
                if !c.is_control() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if text.chars().count() < MAX_SAVE_NAME {
                    text.push(c);
                    self.status = None;
                }
            }
            KeyCode::Enter => {
                if let Some(problem) = mark_name_problem(&text) {
                    self.say(Tone::Trouble, problem, now);
                } else {
                    self.mark_as(&text, now);
                    return Mode::Normal;
                }
            }
            _ => {}
        }
        Mode::Mark { text }
    }

    pub(super) fn mark_as(&mut self, name: &str, now: Instant) {
        self.say(Tone::Info, format!("marking “{name}”…"), now);
        self.jobs
            .push(Job::Call(Request::Mark { name: name.into() }));
    }

    pub(super) fn open_tree(&mut self, now: Instant) {
        if self.link.is_up() {
            self.mode = Mode::Tree { index: 0 };
            self.refresh_tree();
            self.tick_tree(now);
        } else {
            self.say(
                Tone::Trouble,
                "the wall is not answering, so its tree cannot be listed".into(),
                now,
            );
        }
    }

    fn fetch_tree(&mut self, now: Instant) {
        self.tree.fetching = true;
        self.tree.stale = false;
        self.tree.asked = Some(now);
        self.tree.problem = None;
        self.jobs.push(Job::Call(Request::Tree { recent: None }));
    }

    pub(super) const fn refresh_tree(&mut self) {
        self.tree.stale = true;
    }

    pub(super) fn tick_tree(&mut self, now: Instant) {
        if matches!(
            self.mode,
            Mode::Tree { .. } | Mode::Go { .. } | Mode::Command(_)
        ) {
            self.fetch_tree_if_due(now);
        }
    }

    fn fetch_tree_if_due(&mut self, now: Instant) {
        let due = self
            .tree
            .asked
            .is_none_or(|at| now.saturating_duration_since(at) >= SAVES_REFRESH);
        if self.tree.stale && !self.tree.fetching && due {
            self.fetch_tree(now);
        }
    }

    pub(super) fn tree_key(&mut self, key: KeyEvent, index: usize, now: Instant) -> Mode {
        let rows: Vec<_> = self
            .tree_rows()
            .iter()
            .filter(|row| row.kind.selectable())
            .collect();
        let index = match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'B') => return Mode::Normal,
            KeyCode::Down | KeyCode::Char('j') => step_cursor(index, rows.len(), true),
            KeyCode::Up | KeyCode::Char('k') => step_cursor(index, rows.len(), false),
            KeyCode::Char('r') => {
                self.refresh_tree();
                self.fetch_tree_if_due(now);
                index
            }
            KeyCode::Enter => {
                if let Some(row) = rows.get(index) {
                    let moment = row.moment.clone();
                    let label = row.label.clone();
                    self.go_to(&moment, &label, now);
                    return Mode::Normal;
                }
                self.say(
                    Tone::Trouble,
                    "there is no moment to choose yet".into(),
                    now,
                );
                index
            }
            KeyCode::Char('z') => {
                self.undo_latest(now);
                index
            }
            _ => index,
        };
        if matches!(
            key.code,
            KeyCode::Down | KeyCode::Up | KeyCode::Char('j' | 'k')
        ) {
            self.say(Tone::Info, self.tree_distance(index), now);
        }
        Mode::Tree { index }
    }

    pub(in crate::tui) fn tree_distance(&self, index: usize) -> String {
        let chosen = self
            .tree_rows()
            .iter()
            .filter(|row| row.kind.selectable())
            .nth(index);
        match (self.tree.result.as_ref(), chosen) {
            (Some(result), Some(row)) => super::tree_distance::words(result, &row.commit),
            _ => "there is no moment to choose yet".into(),
        }
    }

    pub(super) fn go_key(
        &mut self,
        key: KeyEvent,
        moment: String,
        label: String,
        now: Instant,
    ) -> Mode {
        match key.code {
            KeyCode::Char('y') => {
                self.go_to(&moment, &label, now);
                Mode::Normal
            }
            KeyCode::Esc | KeyCode::Char('n') => Mode::Normal,
            _ => Mode::Go { moment, label },
        }
    }

    pub(super) fn go_to(&mut self, moment: &str, label: &str, now: Instant) {
        self.say(
            Tone::Info,
            format!("going to {label} (the wall as it is is kept first)…"),
            now,
        );
        self.jobs.push(Job::Call(Request::Go {
            moment: moment.into(),
        }));
    }

    pub(super) fn tree_result(
        &mut self,
        request: &Request,
        value: serde_json::Value,
        now: Instant,
    ) {
        match request {
            Request::Mark { .. } => {
                if let Some(result) = self.parse::<MarkResult>(request, value, now) {
                    let at = result.at.change.map_or_else(
                        || format!("at {}", short(&result.at.commit)),
                        |n| format!("after change {n}"),
                    );
                    self.say(
                        Tone::Done,
                        format!("marked “{}” ({at}) · B shows the tree", result.name),
                        now,
                    );
                    self.refresh_tree();
                }
            }
            Request::Go { .. } => {
                if let Some(result) = self.parse::<LoadResult>(request, value, now) {
                    self.loaded(&result, now);
                    if let What::Go { to, .. } = &result.change.what {
                        self.say(
                            Tone::Done,
                            format!(
                                "went to {} · {} · z undoes it",
                                place_words(to),
                                backup_words(&result.backup),
                            ),
                            now,
                        );
                    } else {
                        self.say(
                            Tone::Trouble,
                            "the wall's go answer did not name a destination".into(),
                            now,
                        );
                    }
                    self.refresh_tree();
                }
            }
            Request::Tree { .. } => {
                self.tree.fetching = false;
                if let Some(result) = self.parse::<TreeResult>(request, value, now) {
                    self.listed_tree(result);
                }
            }
            Request::Undo { .. } => {
                if let Some(result) = self.parse::<LoadResult>(request, value, now) {
                    self.loaded(&result, now);
                    // `loaded` words a load; an undo of a go or a load says
                    // where the wall went back to, or what it brought back.
                    let what = match &result.change.what {
                        What::Go { to, .. } => format!("went back to {}", place_words(to)),
                        _ => "brought back the wall it replaced".to_string(),
                    };
                    self.say(
                        Tone::Done,
                        format!("{what} · {} · z undoes it", backup_words(&result.backup)),
                        now,
                    );
                    self.refresh_tree();
                }
            }
            _ => {}
        }
    }

    pub(super) fn tree_failure(
        &mut self,
        request: &Request,
        failure: &Failure,
        now: Instant,
    ) -> bool {
        match request {
            Request::Mark { name } if failure.code == Some(ErrorCode::MarkExists) => {
                let words = if self.mode == Mode::Normal {
                    self.mode = Mode::Mark { text: name.clone() };
                    failure.message.clone()
                } else {
                    format!("{} · b to mark again", failure.message)
                };
                self.say(Tone::Trouble, words, now);
                true
            }
            Request::Tree { .. } => {
                self.tree.fetching = false;
                self.tree.problem = Some(failure.message.clone());
                self.refresh_tree();
                true
            }
            _ => false,
        }
    }

    fn listed_tree(&mut self, result: TreeResult) {
        let old_index = match self.mode {
            Mode::Tree { index } => Some(index),
            _ => None,
        };
        let chosen = old_index.and_then(|index| {
            self.tree
                .rows
                .iter()
                .filter(|row| row.kind.selectable())
                .nth(index)
        });
        let rows = tree_rows(&result);
        let selectable: Vec<_> = rows.iter().filter(|row| row.kind.selectable()).collect();
        let found = chosen.and_then(|old| {
            selectable
                .iter()
                .position(|row| {
                    row.commit == old.commit && row.kind == old.kind && row.moment == old.moment
                })
                .or_else(|| {
                    selectable
                        .iter()
                        .position(|row| row.commit == old.commit && row.kind == old.kind)
                })
                .or_else(|| selectable.iter().position(|row| row.commit == old.commit))
        });
        if let Mode::Tree { index } = &mut self.mode {
            *index = found.unwrap_or_else(|| (*index).min(selectable.len().saturating_sub(1)));
        }
        if result.through < result.revision {
            self.refresh_tree();
        }
        self.tree.rows = rows;
        self.tree.result = Some(result);
        self.tree.problem = None;
    }
}
