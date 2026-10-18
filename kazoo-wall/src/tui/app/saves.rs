//! Saving the wall and the saves list: what their keys do, and what the
//! wall's answers and events about saves show.
//!
//! `w` names a save of the wall as it is now (a save of that name is only
//! replaced after asking); `o` opens the saves list, where a save is loaded
//! or deleted after asking, and an empty wall can be started. Loading
//! never loses anything: the wall saves itself first, and `z` undoes the
//! load.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::protocol::{
    DeleteSaveResult, LoadResult, MAX_SAVE_NAME, Request, SaveResult, SavesResult, is_auto_save,
    valid_save_name,
};

use super::super::log::Tone as LogTone;
use super::super::worker::Job;
use super::{App, Mode, PICKER_PAGE, SAVES_REFRESH, SavesList, Tone, step_cursor};

/// The name a save starts with at the prompt: the time, as
/// `wall-2026-10-09-1430`, which sorts and reads well.
#[must_use]
pub fn default_save_name() -> String {
    chrono::Local::now()
        .format("wall-%Y-%m-%d-%H%M")
        .to_string()
}

/// Why `name` cannot name a save made by hand, if it cannot.
pub(super) fn save_name_problem(name: &str) -> Option<String> {
    if !valid_save_name(name) {
        Some(format!(
            "a save's name is 1 to {MAX_SAVE_NAME} of A-Z a-z 0-9 space _ . -, \
             starting with a letter or digit"
        ))
    } else if is_auto_save(name) {
        Some("names starting with \"before-\" are the wall's own saves: choose another".to_string())
    } else {
        None
    }
}

impl App {
    /// The saves, as the saves list knows them.
    #[must_use]
    pub const fn saves(&self) -> &SavesList {
        &self.saves
    }

    /// The saves the list shows, in its order (see [`SavesList::shown`]):
    /// their names.
    pub(super) fn shown_names(&self) -> Vec<String> {
        self.saves
            .shown()
            .into_iter()
            .map(|save| save.name.clone())
            .collect()
    }

    /// `w`: name a save of the wall as it is now.
    pub(super) fn open_save(&mut self, now: Instant) {
        if self.link.is_up() {
            self.mode = Mode::Save {
                text: default_save_name(),
            };
        } else {
            self.say(
                Tone::Trouble,
                "the wall is not answering, so there is nothing to save".to_string(),
                now,
            );
        }
    }

    /// `o`: open the saves list, asking the wall for it afresh.
    pub(super) fn open_saves(&mut self, now: Instant) {
        if self.link.is_up() {
            self.mode = Mode::Saves { index: 0 };
            self.fetch_saves(now);
        } else {
            self.say(
                Tone::Trouble,
                "the wall is not answering, so its saves cannot be listed".to_string(),
                now,
            );
        }
    }

    /// Ask the wall for the saves.
    pub(super) fn fetch_saves(&mut self, now: Instant) {
        self.saves.fetching = true;
        self.saves.stale = false;
        self.saves.asked = Some(now);
        self.saves.problem = None;
        self.jobs.push(Job::Call(Request::Saves));
    }

    /// Whether the saves list is on screen (or a question over it).
    const fn list_open(&self) -> bool {
        matches!(
            self.mode,
            Mode::Saves { .. }
                | Mode::LoadSave { .. }
                | Mode::DeleteSave { .. }
                | Mode::Clear { .. }
        )
    }

    /// Something changed the saves: the list, when it is open, is asked
    /// for again by [`Self::tick_saves`], at most once a
    /// [`SAVES_REFRESH`], so a storm of saves from other seats costs the
    /// wall one listing a second, not one each.
    const fn refresh_saves(&mut self) {
        self.saves.stale = true;
    }

    /// Ask again for a list gone stale, when it is open, nothing is on its
    /// way, and the last asking was a [`SAVES_REFRESH`] ago.
    pub(super) fn tick_saves(&mut self, now: Instant) {
        let due = self
            .saves
            .asked
            .is_none_or(|at| now.saturating_duration_since(at) >= SAVES_REFRESH);
        if self.saves.stale && !self.saves.fetching && due && self.list_open() {
            self.fetch_saves(now);
        }
    }

    pub(super) fn save_key(&mut self, key: KeyEvent, mut text: String, now: Instant) -> Mode {
        match key.code {
            KeyCode::Esc => {
                self.say(Tone::Info, "nothing was saved".to_string(), now);
                return Mode::Normal;
            }
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c)
                if (c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '.' | '-'))
                    && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if text.len() < MAX_SAVE_NAME {
                    text.push(c);
                }
            }
            KeyCode::Enter => {
                let name = text.trim();
                if let Some(problem) = save_name_problem(name) {
                    self.say(Tone::Trouble, problem, now);
                } else {
                    self.save_as(name, now);
                    return Mode::Normal;
                }
            }
            _ => {}
        }
        Mode::Save { text }
    }

    /// Ask the wall to save itself as `name` (a good name), not over a
    /// save of that name: the wall refuses that, and then the console asks.
    pub(super) fn save_as(&mut self, name: &str, now: Instant) {
        self.say(Tone::Info, format!("saving the wall as \"{name}\"…"), now);
        self.jobs.push(Job::Call(Request::Save {
            name: name.to_string(),
            overwrite: false,
        }));
    }

    pub(super) fn confirm_overwrite(&mut self, key: KeyEvent, name: String, now: Instant) -> Mode {
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.say(Tone::Info, format!("saving over \"{name}\"…"), now);
            self.jobs.push(Job::Call(Request::Save {
                name,
                overwrite: true,
            }));
            Mode::Normal
        } else {
            self.say(
                Tone::Info,
                format!("\"{name}\" is kept as it was; w saves under another name"),
                now,
            );
            Mode::Save { text: name }
        }
    }

    pub(super) fn saves_key(&mut self, key: KeyEvent, index: usize, now: Instant) -> Mode {
        let names = self.shown_names();
        let count = names.len();
        let chosen = names.get(index).cloned();
        let index = match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'o') => return Mode::Normal,
            KeyCode::Up | KeyCode::Char('k') => step_cursor(index, count, false),
            KeyCode::Down | KeyCode::Char('j') => step_cursor(index, count, true),
            KeyCode::PageUp => index.saturating_sub(PICKER_PAGE),
            KeyCode::PageDown => (index + PICKER_PAGE).min(count.saturating_sub(1)),
            KeyCode::Home => 0,
            KeyCode::End => count.saturating_sub(1),
            KeyCode::Enter => {
                if let Some(name) = chosen {
                    return Mode::LoadSave { name, index };
                }
                self.nothing_chosen(now);
                index
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some(name) = chosen {
                    return Mode::DeleteSave { name, index };
                }
                self.nothing_chosen(now);
                index
            }
            KeyCode::Char('n') => return Mode::Clear { index },
            KeyCode::Char('w') => {
                return Mode::Save {
                    text: default_save_name(),
                };
            }
            KeyCode::Char('r') => {
                self.fetch_saves(now);
                index
            }
            KeyCode::Char('a') => {
                self.saves.show_auto = !self.saves.show_auto;
                let words = if self.saves.show_auto {
                    "the wall's own saves are shown, after yours (a hides them)"
                } else {
                    "the wall's own saves are hidden (a shows them)"
                };
                self.say(Tone::Info, words.to_string(), now);
                index.min(self.shown_names().len().saturating_sub(1))
            }
            _ => {
                self.say(
                    Tone::Info,
                    "↑↓ choose · ⏎ loads it · d deletes it · n empty wall · w saves · a the wall's \
                     own · r reads again · Esc closes"
                        .to_string(),
                    now,
                );
                index
            }
        };
        Mode::Saves { index }
    }

    fn nothing_chosen(&mut self, now: Instant) {
        let words = if self.saves.result.is_none() {
            "the saves have not arrived from the wall yet"
        } else {
            "there is no save to choose: w saves the wall"
        };
        self.say(Tone::Trouble, words.to_string(), now);
    }

    pub(super) fn confirm_load(
        &mut self,
        key: KeyEvent,
        (name, index): (String, usize),
        now: Instant,
    ) -> Mode {
        // Only y: Enter chose the save, and a second Enter must not load it.
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.say(
                Tone::Info,
                format!("loading \"{name}\" (the wall as it is is saved first)…"),
                now,
            );
            self.jobs.push(Job::Call(Request::Load { name }));
            Mode::Normal
        } else {
            self.say(Tone::Info, "the wall stays as it is".to_string(), now);
            Mode::Saves { index }
        }
    }

    pub(super) fn confirm_delete(
        &mut self,
        key: KeyEvent,
        (name, index): (String, usize),
        now: Instant,
    ) -> Mode {
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.say(Tone::Info, format!("deleting the save \"{name}\"…"), now);
            self.jobs.push(Job::Call(Request::DeleteSave { name }));
        } else {
            self.say(Tone::Info, format!("\"{name}\" is kept"), now);
        }
        Mode::Saves { index }
    }

    pub(super) fn confirm_clear(&mut self, key: KeyEvent, index: usize, now: Instant) -> Mode {
        if matches!(key.code, KeyCode::Char('y' | 'Y')) {
            self.say(
                Tone::Info,
                "clearing the wall (the wall as it is is saved first)…".to_string(),
                now,
            );
            self.jobs.push(Job::Call(Request::New));
            Mode::Normal
        } else {
            self.say(Tone::Info, "the wall stays as it is".to_string(), now);
            Mode::Saves { index }
        }
    }

    // -----------------------------------------------------------------------
    // Answers and events
    // -----------------------------------------------------------------------

    /// A fresh list of the saves arrived.
    pub(super) fn listed(&mut self, result: SavesResult) {
        // Saves the wall has not read yet: asked for again in a moment.
        if !result.pending.is_empty() {
            self.refresh_saves();
        }
        self.saves.result = Some(result);
        self.saves.problem = None;
        let count = self.saves.shown().len();
        if let Mode::Saves { index }
        | Mode::LoadSave { index, .. }
        | Mode::DeleteSave { index, .. }
        | Mode::Clear { index } = &mut self.mode
        {
            *index = (*index).min(count.saturating_sub(1));
        }
    }

    /// The wall was saved, as this console asked.
    pub(super) fn saved(&mut self, result: &SaveResult, now: Instant) {
        let name = &result.save.name;
        let words = if result.replaced {
            format!("saved over \"{name}\" (o lists the saves)")
        } else {
            format!("saved the wall as \"{name}\" (o lists the saves)")
        };
        self.note(words.clone(), LogTone::Note);
        self.say(Tone::Done, words, now);
        for note in &result.notes {
            self.note(note.clone(), LogTone::Fault);
        }
        if let Some(snapshot) = self.snapshot.as_mut() {
            snapshot.save = Some(crate::protocol::SaveMark {
                name: name.clone(),
                revision: snapshot.revision,
                changed: false,
            });
        }
        self.refresh_saves();
    }

    /// A save (or an empty wall) replaced the wall, as this console asked.
    pub(super) fn loaded(&mut self, result: &LoadResult, now: Instant) {
        self.forget_wall();
        // The wall as it was went to a save, and old ones may have gone.
        self.refresh_saves();
        self.log.add_change(&result.change, true);
        for note in &result.notes {
            self.note(note.clone(), LogTone::Note);
        }
        let backup = &result.backup;
        let words = match &result.change.what {
            crate::protocol::What::Load {
                save: Some(save), ..
            } => format!(
                "loaded \"{save}\" · the wall as it was is saved as \"{backup}\" · z undoes it"
            ),
            _ => format!(
                "cleared the wall · the wall as it was is saved as \"{backup}\" · z undoes it"
            ),
        };
        self.say(Tone::Done, words, now);
    }

    /// A save was deleted, as this console asked.
    pub(super) fn deleted(&mut self, result: &DeleteSaveResult, now: Instant) {
        let words = format!("deleted the save \"{}\"", result.name);
        self.note(words.clone(), LogTone::Note);
        self.say(Tone::Done, words, now);
        for note in &result.notes {
            self.note(note.clone(), LogTone::Fault);
        }
        if let Some(snapshot) = self.snapshot.as_mut()
            && snapshot
                .save
                .as_ref()
                .is_some_and(|mark| mark.name == result.name)
        {
            snapshot.save = None;
        }
        self.refresh_saves();
    }

    /// The wall was replaced, by this console or another seat: everything
    /// held about the old one is let go, and another seat's load is told.
    pub(super) fn on_loaded(&mut self, seat: &str, save: Option<&str>, backup: &str, now: Instant) {
        self.forget_wall();
        // The wall as it was went to a save, and old ones may have gone.
        self.refresh_saves();
        if seat == self.seat {
            return;
        }
        let what = save.map_or_else(
            || "replaced the wall".to_string(),
            |save| format!("loaded \"{save}\""),
        );
        let words = format!("{seat} {what}; the wall as it was is saved as \"{backup}\"");
        self.note(words.clone(), LogTone::Seat);
        self.say(Tone::Info, words, now);
    }

    /// A seat saved or deleted a save: told when it was another seat, and
    /// the list asked for again if it is open.
    pub(super) fn on_saves_changed(&mut self, seat: &str, name: &str, did: &str) {
        if seat != self.seat {
            self.note(format!("{seat} {did} \"{name}\""), LogTone::Seat);
        }
        self.refresh_saves();
    }

    /// Let go of everything held about the wall that was just replaced:
    /// turns gathered or on their way, a drag, a move to undo, and a patch
    /// or a typed value under way (its modules may share ids with the new
    /// wall's, but they are not the same modules).
    fn forget_wall(&mut self) {
        self.pending.clear();
        self.sent.clear();
        self.drag_sends.clear();
        self.rack.drag = None;
        self.rack.sent = None;
        self.rack.last_move = None;
        self.select_after_add = None;
        if matches!(
            self.mode,
            Mode::Patch(_) | Mode::Value { .. } | Mode::Remove { .. }
        ) {
            self.mode = Mode::Normal;
        }
    }
}
