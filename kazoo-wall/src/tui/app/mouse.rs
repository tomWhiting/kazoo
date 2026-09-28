//! What the mouse does in the rack view.
//!
//! - Click a faceplate to select its module; click a knob to select it.
//! - Drag a knob up or down to turn it (hold shift for a fine turn), or
//!   scroll over it for fine turns. Double-click a knob to send it back to
//!   its default.
//! - Drag from a jack to another jack (or onto a knob, whose jack it is) to
//!   patch: an output at one end, an input or knob at the other.
//! - Right-click a cable, or the input or knob it is plugged into, to
//!   unplug it.
//! - Drag a faceplate by its title bar to move it: the rack reflows round
//!   it as it goes, and it hangs where it is let go (see [`super::moves`]).
//! - Click a blank panel (at the end of each row, or the spare row below),
//!   or double-click the bare rack, to add a module there.
//! - Drag a faceplate's panel or the rack between faceplates to pan, or
//!   scroll between faceplates; drag with the middle button to pan from
//!   anywhere, knobs and jacks included.
//! - Carry a cable to the rack's edge and the rack pans that way.
//!
//! Every change is a normal request. A dragged knob's turns are held and
//! sent at most every [`DRAG_INTERVAL`] with a short glide, and its last
//! value as soon as the mouse lets go, all within the flood guard.

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::super::knob::{Step, Travel};
use super::super::rack::geometry::{self, Density, Hit};
use super::super::worker::Job;
use kazoo_wall::patch::arrange_rows;
use kazoo_wall::protocol::{Place, Request};

use super::rack::{Drag, Due};
use super::{App, DRAG_GLIDE, Focus, Mode, Tone, TurnKind, View};

/// Two clicks on the same knob within this are a double-click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Share of a knob's sweep one row of drag turns it.
pub const DRAG_PER_ROW: f64 = 1.0 / 40.0;

/// Share of a knob's sweep one row of fine (shifted) drag turns it.
pub const FINE_PER_ROW: f64 = 1.0 / 200.0;

/// Rows of fine drag per step of a stepped knob.
const FINE_ROWS_PER_STEP: f64 = 3.0;

/// Rows one notch of the wheel pans.
const PAN_ROWS: u32 = 3;

/// Columns one notch of the wheel pans.
const PAN_COLUMNS: u32 = 8;

impl App {
    /// Say that the terminal would not give the console the mouse.
    pub fn mouse_refused(&mut self, why: &str, now: Instant) {
        self.say(
            Tone::Trouble,
            format!("this terminal would not share the mouse ({why}); every key still works"),
            now,
        );
    }

    /// Act on the mouse.
    pub fn handle_mouse(&mut self, event: MouseEvent, now: Instant) {
        let pressed = matches!(event.kind, MouseEventKind::Down(_));
        if self.view != View::Rack {
            if pressed {
                self.say(
                    Tone::Info,
                    "the mouse works in the rack view: press v".to_string(),
                    now,
                );
            }
            return;
        }
        match self.mode {
            Mode::Normal => {}
            Mode::Help { .. } => {
                if pressed {
                    self.mode = Mode::Normal;
                }
                return;
            }
            _ => {
                if pressed {
                    self.say(
                        Tone::Info,
                        "finish what the status line asks first (Esc cancels it)".to_string(),
                        now,
                    );
                }
                return;
            }
        }
        let cell = (event.column, event.row);
        let shift = event.modifiers.contains(KeyModifiers::SHIFT);
        match event.kind {
            MouseEventKind::Down(MouseButton::Left) => self.press(cell, now),
            MouseEventKind::Down(MouseButton::Right) => self.unplug_at(cell, now),
            MouseEventKind::Down(MouseButton::Middle) => self.grab_rack(cell),
            MouseEventKind::Drag(MouseButton::Left) => self.drag_to(cell, shift, now),
            MouseEventKind::Drag(MouseButton::Middle) => {
                if matches!(self.rack.drag, Some(Drag::Pan { .. })) {
                    self.drag_to(cell, shift, now);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.release(cell, now),
            MouseEventKind::Up(MouseButton::Middle) => {
                if matches!(self.rack.drag, Some(Drag::Pan { .. })) {
                    self.rack.drag = None;
                }
            }
            MouseEventKind::ScrollUp => self.wheel(cell, true, shift, now),
            MouseEventKind::ScrollDown => self.wheel(cell, false, shift, now),
            MouseEventKind::ScrollLeft => self.rack.pan_by(-i64::from(PAN_COLUMNS), 0),
            MouseEventKind::ScrollRight => self.rack.pan_by(i64::from(PAN_COLUMNS), 0),
            // Right-button drags and releases, and bare movement, do
            // nothing.
            MouseEventKind::Drag(_) | MouseEventKind::Up(_) | MouseEventKind::Moved => {}
        }
    }

    /// Take hold of the rack to pan it, from anywhere on it (the middle
    /// button, even over a knob or a jack).
    fn grab_rack(&mut self, cell: (u16, u16)) {
        if self.rack.to_sheet(cell).is_none() {
            return;
        }
        self.rack.drag = Some(Drag::Pan {
            anchor: cell,
            pan: self.rack.pan,
        });
    }

    fn press(&mut self, cell: (u16, u16), now: Instant) {
        let Some(hit) = self.rack.hit(cell) else {
            return;
        };
        // A wall older than the rack's rows cannot move modules: there a
        // title bar is the panel, as it always was.
        let hit = match hit {
            Hit::Title { module } if self.wall_rows().is_none() => Hit::Plate { module },
            other => other,
        };
        let double = self.rack.last_click.as_ref().is_some_and(|(last, at)| {
            *last == hit && now.saturating_duration_since(*at) <= DOUBLE_CLICK
        });
        self.rack.last_click = Some((hit.clone(), now));
        self.rack.drag = None;
        match hit {
            Hit::Knob { module, knob } => {
                self.focus = Focus::Wall;
                self.select(module, Some(knob));
                if double {
                    self.rack.last_click = None;
                    self.reset_knob(now);
                } else {
                    self.grab_knob(cell.1);
                }
            }
            Hit::Jack {
                module,
                name,
                output,
            } => {
                self.focus = Focus::Wall;
                self.select(module, None);
                let Some(id) = self.modules().get(module).map(|view| view.id.clone()) else {
                    return;
                };
                let towards = if output {
                    "an input or a knob"
                } else {
                    "an output"
                };
                self.say(
                    Tone::Info,
                    format!("{id}.{name}: drag the cable to {towards} and let go"),
                    now,
                );
                self.rack.drag = Some(Drag::Cable {
                    module: id,
                    name,
                    output,
                    pointer: cell,
                });
            }
            Hit::Title { module } => {
                // Picked up by its title bar: dragged, it moves; let go
                // where it was, it is only selected (in the overview, it
                // goes back to the faceplates, there).
                self.focus = Focus::Wall;
                self.select(module, None);
                self.pick_up(module, cell);
            }
            Hit::Plate { module } if self.rack.layout.density == Density::Overview => {
                // The overview's blocks are for finding a module: a click
                // goes back to the faceplates, there.
                self.focus = Focus::Wall;
                self.select(module, None);
                self.zoom_back(module, now);
            }
            Hit::Plate { module } => {
                // Selected, and the panel itself is something to take hold
                // of: dragging it moves the rack, as the bare rack does.
                self.focus = Focus::Wall;
                self.select(module, None);
                self.rack.drag = Some(Drag::Pan {
                    anchor: cell,
                    pan: self.rack.pan,
                });
            }
            Hit::Blank { row } => {
                // A blank panel: a new module goes there.
                self.rack.last_click = None;
                let place = self.wall_rows().map(|_| Place::end_of(row));
                self.open_picker_at(place, now);
            }
            Hit::Background if double => {
                // Double-clicked bare rack: a new module goes there.
                self.rack.last_click = None;
                let place = self
                    .wall_rows()
                    .and_then(|_| self.rack.to_sheet(cell))
                    .map(|point| self.slot_place(point));
                self.open_picker_at(place, now);
            }
            Hit::Background => {
                self.rack.drag = Some(Drag::Pan {
                    anchor: cell,
                    pan: self.rack.pan,
                });
            }
        }
    }

    /// Go back from the overview to the faceplates, at module `module`.
    fn zoom_back(&mut self, module: usize, now: Instant) {
        let back = self.rack.zoom_back;
        self.rack.set_density(back);
        let id = self.modules().get(module).map(|view| view.id.clone());
        if let Some(id) = id {
            self.say(
                Tone::Info,
                format!("{id}, {} (c for the overview again)", back.name()),
                now,
            );
        }
    }

    /// Pick module `module`'s faceplate up by its title bar, at `cell`.
    fn pick_up(&mut self, module: usize, cell: (u16, u16)) {
        let Some(id) = self.modules().get(module).map(|view| view.id.clone()) else {
            return;
        };
        let from: Vec<Vec<String>> = self
            .shown_rows()
            .map(<[Vec<String>]>::to_vec)
            .unwrap_or_default();
        let rows: Vec<Vec<String>> = from
            .iter()
            .map(|ids| ids.iter().filter(|other| **other != id).cloned().collect())
            .filter(|ids: &Vec<String>| !ids.is_empty())
            .collect();
        let area = self.rack.area;
        let base = geometry::rack(
            self.modules(),
            self.rack.density,
            (area.width, area.height),
            Some(&rows),
        );
        self.rack.drag = Some(Drag::Plate {
            module: id,
            base,
            rows,
            from,
            preview: None,
            anchor: cell,
            pointer: cell,
        });
    }

    /// Where the carried faceplate would go, with the mouse where it is:
    /// the rows it makes, shown as the rack reflows round it. Only once it
    /// has moved (`moved`), and only on a wall that keeps rows.
    pub(super) fn carry_plate(&mut self, moved: bool) {
        let Some(Drag::Plate {
            module,
            base,
            rows,
            preview,
            pointer,
            ..
        }) = &self.rack.drag
        else {
            return;
        };
        let area = self.rack.area;
        if (preview.is_none() && !moved)
            || self.wall_rows().is_none()
            || area.width == 0
            || area.height == 0
        {
            return;
        }
        // Past the rack's edge counts as at it (the rack pans that way).
        let cell = (
            pointer.0.clamp(area.x, area.x + area.width - 1),
            pointer.1.clamp(area.y, area.y + area.height - 1),
        );
        let Some(point) = self.rack.to_sheet(cell) else {
            return;
        };
        let slot = base.slot_at(point);
        let before = slot
            .before
            .and_then(|index| self.modules().get(index))
            .map(|view| view.id.clone())
            .filter(|before| before != module);
        let mut rows = rows.clone();
        let id = module.clone();
        arrange_rows(
            &mut rows,
            &id,
            &Place {
                row: slot.row,
                before,
                own: slot.own,
            },
        );
        if let Some(Drag::Plate { preview, .. }) = &mut self.rack.drag {
            *preview = Some(rows);
        }
        self.rack.follow = false;
    }

    /// The place a new module goes when the bare rack is double-clicked at
    /// `point`: in front of the faceplate to its right, or at the end of
    /// its row; between rows, a row of its own; below them, a new row.
    fn slot_place(&self, point: geometry::Point) -> Place {
        let slot = self.rack.layout.slot_at(point);
        Place {
            row: slot.row,
            before: slot
                .before
                .and_then(|index| self.modules().get(index))
                .map(|view| view.id.clone()),
            own: slot.own,
        }
    }

    /// Take hold of the selected knob, from where it is heading.
    fn grab_knob(&mut self, row: u16) {
        let Some((module, knob)) = self.selected_knob() else {
            return;
        };
        let travel = Travel::of(knob, self.knob_info(&module.kind, &knob.name));
        let (id, name) = (module.id.clone(), knob.name.clone());
        let start = self.ahead(&id, &name).unwrap_or(knob.target);
        let anchor = if travel.stepped {
            travel.clamp(start)
        } else {
            travel.position(start)
        };
        self.drag_sends
            .retain(|(module, knob, _)| !(*module == id && *knob == name));
        self.rack.drag = Some(Drag::Knob {
            module: id,
            knob: name,
            travel,
            anchor_row: row,
            anchor,
            fine: false,
            moved: false,
        });
    }

    /// Send the selected knob back to its default.
    fn reset_knob(&mut self, now: Instant) {
        let Some((module, knob)) = self.selected_knob() else {
            return;
        };
        let Some(info) = self.knob_info(&module.kind, &knob.name) else {
            self.say(
                Tone::Trouble,
                "the catalogue has not arrived, so the default is not known yet".to_string(),
                now,
            );
            return;
        };
        let travel = Travel::of(knob, Some(info));
        let (id, name, default) = (
            module.id.clone(),
            knob.name.clone(),
            travel.clamp(info.default),
        );
        self.hold_turn(&id, &name, default, TurnKind::Now, Some(DRAG_GLIDE), now);
        self.tick(now);
        self.say(Tone::Info, format!("{id} {name} back to its default"), now);
    }

    fn drag_to(&mut self, cell: (u16, u16), fine: bool, now: Instant) {
        let Some(drag) = self.rack.drag.take() else {
            return;
        };
        self.rack.drag = Some(match drag {
            Drag::Knob {
                module,
                knob,
                travel,
                anchor_row,
                anchor,
                fine: was_fine,
                moved,
            } => {
                // Changing between fine and coarse re-measures from here, so
                // the knob never jumps; shift held from the first move
                // measures from the grab.
                let (anchor_row, anchor) = if fine == was_fine || !moved {
                    (anchor_row, anchor)
                } else {
                    let here = self
                        .ahead(&module, &knob)
                        .unwrap_or_else(|| travel.value_at(anchor));
                    let anchor = if travel.stepped {
                        travel.clamp(here)
                    } else {
                        travel.position(here)
                    };
                    (cell.1, anchor)
                };
                let value = dragged_value(&travel, anchor, anchor_row, cell.1, fine);
                // A knob that has been dragged is not half of a double-click.
                if cell.1 != anchor_row {
                    self.rack.last_click = None;
                }
                let shown = self
                    .ahead(&module, &knob)
                    .or_else(|| self.knob_target(&module, &knob));
                if shown.is_none_or(|shown| !travel.same(shown, value)) {
                    self.hold_turn(&module, &knob, value, TurnKind::Drag, Some(DRAG_GLIDE), now);
                }
                Drag::Knob {
                    module,
                    knob,
                    travel,
                    anchor_row,
                    anchor,
                    fine,
                    moved: moved || cell.1 != anchor_row,
                }
            }
            Drag::Cable {
                module,
                name,
                output,
                ..
            } => Drag::Cable {
                module,
                name,
                output,
                pointer: cell,
            },
            Drag::Plate {
                module,
                base,
                rows,
                from,
                preview,
                anchor,
                ..
            } => {
                let moved = cell != anchor;
                if moved && preview.is_none() && self.wall_rows().is_none() {
                    // The faceplate stays where it hangs, and says why.
                    self.cannot_move(now);
                }
                self.rack.drag = Some(Drag::Plate {
                    module,
                    base,
                    rows,
                    from,
                    preview,
                    anchor,
                    pointer: cell,
                });
                self.carry_plate(moved);
                return;
            }
            Drag::Pan { anchor, pan } => {
                let dx = i32::from(cell.0) - i32::from(anchor.0);
                let dy = i32::from(cell.1) - i32::from(anchor.1);
                self.rack.pan = geometry::dragged(pan, dx, dy);
                self.rack.follow = false;
                Drag::Pan { anchor, pan }
            }
        });
    }

    /// The knob's target in the latest snapshot.
    fn knob_target(&self, module: &str, knob: &str) -> Option<f64> {
        self.modules()
            .iter()
            .find(|view| view.id == module)?
            .knobs
            .iter()
            .find(|view| view.name == knob)
            .map(|view| view.target)
    }

    fn release(&mut self, cell: (u16, u16), now: Instant) {
        match self.rack.drag.take() {
            Some(Drag::Knob { module, knob, .. }) => {
                // The last value goes as soon as the flood guard allows,
                // without waiting for the drag interval.
                for turn in &mut self.pending {
                    if turn.module == module && turn.knob == knob && turn.kind == TurnKind::Drag {
                        turn.kind = TurnKind::Now;
                    }
                }
                self.drag_sends
                    .retain(|(held, name, _)| !(*held == module && *name == knob));
                self.tick(now);
            }
            Some(Drag::Cable {
                module,
                name,
                output,
                ..
            }) => self.drop_cable(&module, &name, output, cell, now),
            Some(Drag::Plate {
                module,
                preview,
                from,
                ..
            }) => match preview {
                Some(rows) if rows != from => {
                    self.hold_move(&module, rows, Due::Now, now);
                    self.say(Tone::Info, format!("moving {module}…"), now);
                    self.tick(now);
                }
                // Let go where it hangs: nothing moves.
                Some(_) => {}
                None => {
                    if self.rack.layout.density == Density::Overview {
                        if let Some(index) =
                            self.modules().iter().position(|view| view.id == module)
                        {
                            self.zoom_back(index, now);
                        }
                    }
                }
            },
            Some(Drag::Pan { .. }) | None => {}
        }
    }

    /// Plug the cable carried from `module`.`name` into what is under
    /// `cell`.
    fn drop_cable(
        &mut self,
        module: &str,
        name: &str,
        output: bool,
        cell: (u16, u16),
        now: Instant,
    ) {
        let start = format!("{module}.{name}");
        let target = self.rack.hit(cell).and_then(|hit| match hit {
            Hit::Jack {
                module,
                name,
                output,
            } => Some((self.modules().get(module)?.id.clone(), name, output)),
            Hit::Knob { module, knob } => {
                let view = self.modules().get(module)?;
                Some((view.id.clone(), view.knobs.get(knob)?.name.clone(), false))
            }
            Hit::Title { .. } | Hit::Plate { .. } | Hit::Blank { .. } | Hit::Background => None,
        });
        let (from, to) = match target {
            Some((id, jack, false)) if output => (start, format!("{id}.{jack}")),
            Some((id, jack, true)) if !output => (format!("{id}.{jack}"), start),
            Some((id, jack, _)) if id == module && jack == name => return,
            _ => {
                self.say(
                    Tone::Info,
                    "a cable goes from an output to an input or a knob; nothing was plugged"
                        .to_string(),
                    now,
                );
                return;
            }
        };
        self.jobs.push(Job::Call(Request::Patch {
            from,
            to,
            amount: None,
        }));
        self.say(Tone::Info, "plugging in…".to_string(), now);
    }

    /// Unplug the cable under `cell`, or the one in the input or knob
    /// there.
    fn unplug_at(&mut self, cell: (u16, u16), now: Instant) {
        let Some(point) = self.rack.to_sheet(cell) else {
            return;
        };
        let into = match self.rack.layout.hit(point) {
            Hit::Jack {
                module,
                name,
                output: false,
            } => self
                .modules()
                .get(module)
                .map(|view| format!("{}.{name}", view.id)),
            Hit::Knob { module, knob } => self.modules().get(module).and_then(|view| {
                view.knobs
                    .get(knob)
                    .map(|knob| format!("{}.{}", view.id, knob.name))
            }),
            Hit::Jack { output: true, .. }
            | Hit::Title { .. }
            | Hit::Plate { .. }
            | Hit::Blank { .. }
            | Hit::Background => None,
        };
        let cable = self.rack.cables.get(&point).copied().or_else(|| {
            let into = into.as_deref()?;
            self.cables()
                .iter()
                .find(|cable| cable.to == into)
                .map(|cable| cable.id)
        });
        match cable {
            Some(cable) => {
                self.jobs.push(Job::Call(Request::Unpatch {
                    cable: Some(cable),
                    to: None,
                }));
                self.say(Tone::Info, format!("unplugging cable {cable}…"), now);
            }
            None => self.say(
                Tone::Info,
                "right-click a cable, or the input or knob it is plugged into, to unplug it"
                    .to_string(),
                now,
            ),
        }
    }

    /// The wheel: a fine turn over a knob, a pan anywhere else (sideways
    /// with shift, or when the rack is no taller than the screen).
    fn wheel(&mut self, cell: (u16, u16), up: bool, shift: bool, now: Instant) {
        match self.rack.hit(cell) {
            Some(Hit::Knob { module, knob }) => {
                self.focus = Focus::Wall;
                self.select(module, Some(knob));
                self.turn(up, Step::Fine, now);
            }
            Some(_) => {
                let sign = if up { -1 } else { 1 };
                let taller = self.rack.layout.height > u32::from(self.rack.area.height);
                if taller && !shift {
                    self.rack.pan_by(0, sign * i64::from(PAN_ROWS));
                } else {
                    self.rack.pan_by(sign * i64::from(PAN_COLUMNS), 0);
                }
            }
            None => {}
        }
    }
}

/// The value of a knob dragged from `anchor` (its position, or its value
/// when stepped) at row `anchor_row` to row `row`: up turns it up.
#[must_use]
pub fn dragged_value(travel: &Travel, anchor: f64, anchor_row: u16, row: u16, fine: bool) -> f64 {
    let rows = f64::from(i32::from(anchor_row) - i32::from(row));
    if travel.stepped {
        let steps = if fine {
            rows / FINE_ROWS_PER_STEP
        } else {
            rows
        };
        travel.clamp(anchor + steps.trunc())
    } else {
        let per_row = if fine { FINE_PER_ROW } else { DRAG_PER_ROW };
        travel.value_at(rows.mul_add(per_row, anchor))
    }
}
