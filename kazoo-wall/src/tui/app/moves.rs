//! Moving modules on the rack: `H J K L` shove the selected module along
//! its row or to the row above or below, a faceplate dragged by its title
//! bar drops where the mouse lets go, and `z` puts the latest move back.
//!
//! The rows are the wall's own, shared by every console. A move shows at
//! once (see [`super::rack::Move`]), and once sent, until the wall's rows
//! show it; moves made with keys are gathered like turns, and go to the wall as
//! one `arrange` when the keys rest, within the flood guard. A move is not
//! a change to the sound: it is not in the log.

use std::time::Instant;

use kazoo_wall::patch::{arrange_rows, place_for};
use kazoo_wall::protocol::{Place, Request};

use super::super::worker::Job;
use super::rack::{Due, MOVE_LONGEST, Move};
use super::{App, Focus, TURN_LONGEST, TURN_SETTLE, Tone, View};

/// Which way `H J K L` shove a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shove {
    /// Along its row, to the left (`H`).
    Left,
    /// To the row below (`J`).
    Down,
    /// To the row above (`K`).
    Up,
    /// Along its row, to the right (`L`).
    Right,
}

impl App {
    /// The wall's own rows, if it keeps them.
    pub(super) fn wall_rows(&self) -> Option<&[Vec<String>]> {
        self.snapshot.as_ref()?.rack.as_deref()
    }

    /// The rows the rack shows: a move under way, or the wall's own.
    pub(super) fn shown_rows(&self) -> Option<&[Vec<String>]> {
        self.rack.rows(self.wall_rows())
    }

    /// Whether modules can be moved on this wall; says why not when not.
    pub(super) fn can_move(&mut self, now: Instant) -> bool {
        if self.wall_rows().is_some() {
            return true;
        }
        self.cannot_move(now);
        false
    }

    /// Say why modules cannot be moved: the wall keeps no rows.
    pub(super) fn cannot_move(&mut self, now: Instant) {
        let words = if self.snapshot.is_some() {
            "this wall is older than the rack's shared rows, so its modules cannot be moved \
             (a newer kazoo-wall daemon keeps them)"
        } else {
            "the wall has not been seen yet, so nothing can be moved"
        };
        self.say(Tone::Trouble, words.to_string(), now);
    }

    /// Shove the selected module one place `way`.
    pub(super) fn shove(&mut self, way: Shove, now: Instant) {
        if self.view != View::Rack {
            self.say(
                Tone::Info,
                "H J K L move modules in the rack view (v)".to_string(),
                now,
            );
            return;
        }
        if self.focus != Focus::Wall {
            self.say(
                Tone::Info,
                "move modules on the wall: Tab back to it".to_string(),
                now,
            );
            return;
        }
        let Some(id) = self.selected_module().map(|module| module.id.clone()) else {
            self.say(Tone::Trouble, "no module is selected".to_string(), now);
            return;
        };
        if !self.can_move(now) {
            return;
        }
        let Some(rows) = self.shown_rows().map(<[Vec<String>]>::to_vec) else {
            return;
        };
        match self.shoved(&rows, &id, way) {
            Ok(moved) => {
                self.hold_move(
                    &id,
                    moved,
                    Due::Keys {
                        first: now,
                        last: now,
                    },
                    now,
                );
                self.rack.follow = true;
            }
            Err(why) => self.say(Tone::Info, why, now),
        }
    }

    /// `rows` with module `id` shoved one place `way`, or why it cannot go.
    fn shoved(
        &self,
        rows: &[Vec<String>],
        id: &str,
        way: Shove,
    ) -> Result<Vec<Vec<String>>, String> {
        let Some((row, at)) = rows
            .iter()
            .enumerate()
            .find_map(|(row, ids)| ids.iter().position(|other| other == id).map(|at| (row, at)))
        else {
            return Err(format!("{id} is not on the rack yet"));
        };
        let mut moved = rows.to_vec();
        let alone = moved[row].len() == 1;
        match way {
            Shove::Left if at == 0 => return Err(format!("{id} is at the start of its row")),
            Shove::Right if at + 1 == moved[row].len() => {
                return Err(format!("{id} is at the end of its row"));
            }
            Shove::Left => moved[row].swap(at, at - 1),
            Shove::Right => moved[row].swap(at, at + 1),
            Shove::Up if row == 0 && alone => return Err(format!("{id} is on the top row")),
            Shove::Down if row + 1 == moved.len() && alone => {
                return Err(format!("{id} is on the bottom row"));
            }
            // Off the top or bottom row, onto a row of its own.
            Shove::Up if row == 0 => {
                let place = Place {
                    row: 0,
                    before: None,
                    own: true,
                };
                arrange_rows(&mut moved, id, &place);
            }
            Shove::Down if row + 1 == moved.len() => {
                let bottom = Place::end_of(moved.len());
                arrange_rows(&mut moved, id, &bottom);
            }
            Shove::Up | Shove::Down => {
                let target = if way == Shove::Up { row - 1 } else { row + 1 };
                let before = self.nearest_in(&moved[target], id, at);
                let place = Place {
                    row: target,
                    before,
                    own: false,
                };
                arrange_rows(&mut moved, id, &place);
            }
        }
        Ok(moved)
    }

    /// The module in `row` that module `id` goes in front of when shoved
    /// into it: the first whose faceplate's middle is right of the middle
    /// of its own, as last drawn (or, not drawn, the one at `at` along).
    fn nearest_in(&self, row: &[String], id: &str, at: usize) -> Option<String> {
        let modules = self.modules();
        let index = |id: &str| modules.iter().position(|module| module.id == id);
        let middle = |id: &str| {
            let plate = self.rack.layout.plate(index(id)?)?;
            Some(plate.at.x * 2 + u32::from(plate.width))
        };
        middle(id).map_or_else(
            || row.get(at).cloned(),
            |mine| {
                row.iter()
                    .find(|other| middle(other).is_some_and(|theirs| theirs > mine))
                    .cloned()
            },
        )
    }

    /// Show module `id` moved into `rows`, and send it when `due`. A move of
    /// another module still waiting goes first; a further move of the same
    /// one gathers with it.
    pub(super) fn hold_move(&mut self, id: &str, rows: Vec<Vec<String>>, due: Due, now: Instant) {
        let waiting = self
            .rack
            .moving
            .as_ref()
            .map(|moving| moving.module.clone());
        if waiting.is_some_and(|module| module != id) {
            if !self.flood.take(now) {
                self.say(
                    Tone::Trouble,
                    "too many changes at once: wait a moment before moving another module"
                        .to_string(),
                    now,
                );
                return;
            }
            self.send_move(now);
        }
        let (from, due) = match self.rack.moving.take() {
            // The same module again: the move gathers, and puts back (with
            // z) to where it was before the first shove.
            Some(earlier) if earlier.module == id => {
                let due = match (earlier.due, due) {
                    (Due::Keys { first, .. }, Due::Keys { last, .. }) => Due::Keys { first, last },
                    (_, due) => due,
                };
                (earlier.from, due)
            }
            _ => (self.shown_rows().map(<[Vec<String>]>::to_vec), due),
        };
        self.rack.moving = Some(Move {
            module: id.to_string(),
            rows,
            from,
            due,
        });
    }

    /// Put the latest move made here back, if nothing else has been done
    /// since. Whether there was one.
    pub(super) fn undo_move(&mut self, now: Instant) -> bool {
        let Some((id, from)) = self.rack.last_move.take() else {
            return false;
        };
        let Some(rows) = self.shown_rows().map(<[Vec<String>]>::to_vec) else {
            return false;
        };
        let Some(place) = place_for(&rows, &from, &id) else {
            // It has left the wall since: the log's undo is next.
            return false;
        };
        let mut back = rows;
        arrange_rows(&mut back, &id, &place);
        self.hold_move(&id, back, Due::Now, now);
        if let Some(moving) = self.rack.moving.as_mut() {
            moving.from = None;
        }
        self.say(Tone::Info, format!("putting {id} back where it was…"), now);
        self.tick(now);
        true
    }

    /// Send a move that is due, within the flood guard; let moves sent go
    /// once the wall has had long enough to show them.
    pub(super) fn tick_moves(&mut self, now: Instant) {
        if self
            .rack
            .sent
            .as_ref()
            .is_some_and(|(_, at)| now.saturating_duration_since(*at) >= MOVE_LONGEST)
        {
            self.rack.sent = None;
        }
        let Some(moving) = self.rack.moving.as_ref() else {
            return;
        };
        let due = match moving.due {
            Due::Keys { first, last } => {
                now.saturating_duration_since(last) >= TURN_SETTLE
                    || now.saturating_duration_since(first) >= TURN_LONGEST
            }
            Due::Now => true,
        };
        if due && self.flood.take(now) {
            self.send_move(now);
        }
    }

    /// Send the move waiting (its place in the flood guard already taken),
    /// as a place in the rows the wall will have by then: its own, or those
    /// the moves already sent make.
    fn send_move(&mut self, now: Instant) {
        let Some(moving) = self.rack.moving.take() else {
            return;
        };
        let wall = self
            .rack
            .sent
            .as_ref()
            .map(|(sent, _)| sent.as_slice())
            .or_else(|| self.wall_rows());
        let Some(wall) = wall else {
            return;
        };
        if wall == moving.rows.as_slice() {
            // Shoved and shoved back: the wall has it where it is.
            return;
        }
        let Some(place) = place_for(wall, &moving.rows, &moving.module) else {
            return;
        };
        self.jobs.push(Job::Call(Request::Arrange {
            module: moving.module.clone(),
            row: place.row,
            before: place.before,
            own: place.own,
        }));
        if let Some(from) = moving.from {
            self.rack.last_move = Some((moving.module.clone(), from));
        }
        self.rack.sent = Some((moving.rows, now));
    }

    /// The wall's rows have come: once they show the moves sent, the
    /// wall's own are shown again.
    pub(super) fn rows_seen(&mut self) {
        let shown = self
            .rack
            .sent
            .as_ref()
            .is_some_and(|(sent, _)| self.wall_rows() == Some(sent.as_slice()));
        if shown {
            self.rack.sent = None;
        }
    }
}
