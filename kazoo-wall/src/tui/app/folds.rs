use std::collections::BTreeSet;
use std::time::Instant;

use crossterm::event::KeyCode;

use super::super::rack::fold::Tile;
use super::{App, Focus, Tone, View};

#[derive(Debug, Default)]
pub struct Look {
    pub folded: BTreeSet<String>,
}

impl App {
    pub(super) fn fold_representative(&self, index: usize) -> usize {
        if self.view != View::Rack {
            return index;
        }
        self.rack
            .tiles
            .iter()
            .find_map(|(&first, tile)| tile.members.contains(&index).then_some(first))
            .unwrap_or(index)
    }

    pub(super) fn folded_key(&mut self, key: KeyCode, now: Instant) -> bool {
        if self.focus != Focus::Wall
            || self
                .selected_index()
                .is_none_or(|index| self.folded_tile(index).is_none())
        {
            return false;
        }
        match key {
            KeyCode::Char('H' | 'J' | 'K' | 'L') => self.folded_move_blocked(now),
            KeyCode::Char('x' | 'p' | 'u' | '=' | '+' | '-' | '_')
            | KeyCode::Enter
            | KeyCode::PageUp
            | KeyCode::PageDown => {
                self.say(
                    Tone::Info,
                    "unfold the group with Tab before editing a member".to_string(),
                    now,
                );
                true
            }
            _ => false,
        }
    }

    pub fn folded_tile(&self, index: usize) -> Option<&Tile> {
        if self.view == View::Rack {
            self.rack.tiles.get(&index)
        } else {
            None
        }
    }

    pub(super) fn run_fold(
        &mut self,
        name: &str,
        args: &[&str],
        now: Instant,
    ) -> Result<(), String> {
        if args.len() > 1 || (name == "unfold" && args.is_empty()) {
            return Err(format!("type {}", super::usage(name).unwrap_or_default()));
        }
        self.fold_group(args.first().copied(), name == "fold", now)
    }

    pub(super) fn fold_group(
        &mut self,
        name: Option<&str>,
        fold: bool,
        now: Instant,
    ) -> Result<(), String> {
        let group = if let Some(name) = name {
            // A fold kept from before the group went away can still be
            // opened, so a gone group never stays in the settings for good.
            let gone = || {
                (!fold)
                    .then(|| {
                        self.look
                            .folded
                            .iter()
                            .find(|group| group.eq_ignore_ascii_case(name))
                    })
                    .flatten()
            };
            self.groups()
                .and_then(|groups| groups.keys().find(|group| group.eq_ignore_ascii_case(name)))
                .or_else(gone)
                .cloned()
                .ok_or_else(|| format!("there is no group called '{name}' (:groups says them)"))?
        } else {
            let id = &self
                .selected_module()
                .ok_or_else(|| "select a module in a group".to_string())?
                .id;
            let groups: Vec<&String> = self
                .groups()
                .into_iter()
                .flatten()
                .filter(|(_, members)| members.contains(id))
                .map(|(group, _)| group)
                .collect();
            match groups.as_slice() {
                [group] => (*group).clone(),
                [] => return Err("the selected module is not in a group".to_string()),
                _ => {
                    return Err(
                        "the selected module is in several groups: name the group".to_string()
                    );
                }
            }
        };
        if fold {
            self.look.folded.insert(group.clone());
        } else {
            self.look.folded.remove(&group);
        }
        self.rack.drag = None;
        let area = self.rack.area;
        self.lay_out_rack((area.width, area.height));
        self.rack.follow = true;
        self.say(
            Tone::Info,
            format!("{group} {}", if fold { "folded" } else { "unfolded" }),
            now,
        );
        Ok(())
    }

    pub(super) fn unfold_selected(&mut self, now: Instant) -> bool {
        if self.focus != Focus::Wall {
            return false;
        }
        let Some(group) = self
            .selected_index()
            .and_then(|index| self.folded_tile(index))
            .map(|tile| tile.group.clone())
        else {
            return false;
        };
        match self.fold_group(Some(&group), false, now) {
            Ok(()) => true,
            Err(why) => {
                self.say(Tone::Trouble, why, now);
                true
            }
        }
    }

    pub(super) fn normalize_fold_selection(&mut self) {
        if self.view != View::Rack {
            return;
        }
        if let Some(index) = self.selected_index() {
            self.select(index, None);
        }
    }

    pub(super) fn folded_move_blocked(&mut self, now: Instant) -> bool {
        let Some(group) = self
            .selected_index()
            .and_then(|index| self.folded_tile(index))
            .map(|tile| tile.group.clone())
        else {
            return false;
        };
        self.say(
            Tone::Trouble,
            format!(
                "{group} cannot move as a group: this wall only supports individual module moves"
            ),
            now,
        );
        true
    }
}
