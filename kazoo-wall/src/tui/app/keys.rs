//! What every key does, in each mode.
//!
//! Keys only change the console's state and queue jobs; nothing here
//! touches the socket.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use kazoo_wall::format;
use kazoo_wall::protocol::{KnobView, ModuleView, Place, Request, valid_name};

use super::super::knob::{Step, Travel};
use super::super::rack::geometry::Density;
use super::super::worker::Job;
use super::{
    AMOUNT_FINE, AMOUNT_STEP, App, Exit, Focus, GLIDES, MAX_ENTRY, Mode, PICKER_PAGE, Patching,
    Shove, Tone, TurnKind, View, glide_words, has_jacks, has_outputs, jack_module, jack_names,
    next_focus, split_jack, step_cursor,
};

impl App {
    // -----------------------------------------------------------------------

    /// Act on a key.
    pub fn handle_key(&mut self, key: KeyEvent, now: Instant) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.exit = Some(Exit::Left);
            return;
        }
        // Keys move the selection; the rack view brings it into sight.
        self.rack.follow = true;
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        self.mode = match mode {
            Mode::Normal => {
                self.normal_key(key, now);
                return;
            }
            Mode::Help { scroll } => match key.code {
                KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => Mode::Help {
                    scroll: scroll.saturating_add(if key.code == KeyCode::PageDown {
                        10
                    } else {
                        1
                    }),
                },
                KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => Mode::Help {
                    scroll: scroll.saturating_sub(if key.code == KeyCode::PageUp { 10 } else { 1 }),
                },
                _ => Mode::Normal,
            },
            Mode::Add {
                index,
                filter,
                place,
            } => self.add_key(key, (index, filter, place), now),
            Mode::Name { kind, text, place } => self.name_key(key, (kind, text, place), now),
            Mode::Remove { module, .. } => self.confirm_remove(key, module, now),
            Mode::Stop => self.confirm_stop(key, now),
            Mode::Patch(step) => self.patch_key(key, step, now),
            Mode::Tempo { text } => self.tempo_key(key, text, now),
            Mode::Value { module, knob, text } => self.value_key(key, module, knob, text, now),
            Mode::Jump { text } => self.jump_key(key, text, now),
        };
    }

    fn normal_key(&mut self, key: KeyEvent, now: Instant) {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if self.view == View::Rack
            && key.modifiers.contains(KeyModifiers::SHIFT)
            && self.pan_key(key.code)
        {
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.exit = Some(Exit::Left),
            KeyCode::Char('Q') => self.ask_stop(now),
            KeyCode::Char('?') => self.mode = Mode::Help { scroll: 0 },
            KeyCode::Tab => self.cycle_focus(true),
            KeyCode::BackTab => self.cycle_focus(false),
            KeyCode::Char('v') => self.switch_view(now),
            KeyCode::Char('f') => self.toggle_log(now),
            KeyCode::Char('c') => self.cycle_density(now),
            KeyCode::Char('H') => self.shove(Shove::Left, now),
            KeyCode::Char('J') => self.shove(Shove::Down, now),
            KeyCode::Char('K') => self.shove(Shove::Up, now),
            KeyCode::Char('L') => self.shove(Shove::Right, now),
            KeyCode::Char('.') => self.centre_selection(now),
            KeyCode::Char('g') => {
                self.mode = Mode::Jump {
                    text: String::new(),
                }
            }
            KeyCode::Char('a') => self.open_picker(now),
            KeyCode::Char('x') => self.ask_remove(now),
            KeyCode::Char('p') => self.start_patch(now),
            KeyCode::Char('u') => self.unplug(now),
            KeyCode::Char('z') => self.undo(now),
            KeyCode::Char('t') => {
                self.mode = Mode::Tempo {
                    text: String::new(),
                }
            }
            KeyCode::Char('[') => self.change_glide(false, now),
            KeyCode::Char(']') => self.change_glide(true, now),
            KeyCode::Char('s') => self.start_wall(now),
            KeyCode::Char('m') => self.toggle_monitor(now),
            KeyCode::Char('r') => self.toggle_recording(now),
            KeyCode::Enter => self.open_value(now),
            KeyCode::Left | KeyCode::Char('h') => self.move_across(false),
            KeyCode::Right | KeyCode::Char('l') => self.move_across(true),
            KeyCode::Up | KeyCode::Char('k') => self.move_along(false),
            KeyCode::Down | KeyCode::Char('j') => self.move_along(true),
            KeyCode::PageUp => self.turn(true, Step::Coarse, now),
            KeyCode::PageDown => self.turn(false, Step::Coarse, now),
            KeyCode::Char('=') => self.turn(true, coarse_if(alt, Step::Normal), now),
            KeyCode::Char('+') => self.turn(true, coarse_if(alt, Step::Fine), now),
            KeyCode::Char('-') => self.turn(false, coarse_if(alt, Step::Normal), now),
            KeyCode::Char('_') => self.turn(false, coarse_if(alt, Step::Fine), now),
            _ => {}
        }
    }

    /// The next pane; the rack view has no cable list (its cables are on
    /// the rack), nor a log once it is put away.
    const fn cycle_focus(&mut self, forward: bool) {
        self.focus = next_focus(self.focus, forward);
        while !self.shows(self.focus) {
            self.focus = next_focus(self.focus, forward);
        }
    }

    /// Whether pane `focus` is on screen.
    const fn shows(&self, focus: Focus) -> bool {
        match focus {
            Focus::Wall => true,
            Focus::Cables => matches!(self.view, View::List),
            Focus::Log => self.log_shown(),
        }
    }

    /// Shift and an arrow, in the rack view: pan a quarter of the view that
    /// way. Whether it was one.
    fn pan_key(&mut self, code: KeyCode) -> bool {
        let across = i64::from((self.rack.area.width / 4).max(1));
        let down = i64::from((self.rack.area.height / 4).max(1));
        let (dx, dy) = match code {
            KeyCode::Left => (-across, 0),
            KeyCode::Right => (across, 0),
            KeyCode::Up => (0, -down),
            KeyCode::Down => (0, down),
            _ => return false,
        };
        self.rack.pan_by(dx, dy);
        true
    }

    /// Put the log away in the rack view, or bring it back.
    fn toggle_log(&mut self, now: Instant) {
        if self.view != View::Rack {
            self.say(
                Tone::Info,
                "f puts the log away in the rack view (v), for more rack".to_string(),
                now,
            );
            return;
        }
        self.log_hidden = !self.log_hidden;
        if self.log_hidden {
            if self.focus == Focus::Log {
                self.focus = Focus::Wall;
            }
            self.say(
                Tone::Info,
                "the log is put away, for more rack (f brings it back)".to_string(),
                now,
            );
        } else {
            self.say(
                Tone::Info,
                "the log is back (f puts it away)".to_string(),
                now,
            );
        }
    }

    /// Draw the rack at its next density: full, compact, overview.
    fn cycle_density(&mut self, now: Instant) {
        if self.view != View::Rack {
            self.say(
                Tone::Info,
                "c changes how densely the rack view (v) is drawn".to_string(),
                now,
            );
            return;
        }
        let density = self.rack.density.next();
        self.rack.set_density(density);
        let words = match density {
            Density::Full => "full faceplates (c for compact)",
            Density::Compact => "compact faceplates (c for the overview)",
            Density::Overview => {
                "the overview: click a module to go back to it (c for full faceplates)"
            }
        };
        self.say(Tone::Info, words.to_string(), now);
    }

    /// Put the selected module in the middle of the rack view.
    fn centre_selection(&mut self, now: Instant) {
        if self.view != View::Rack {
            self.say(
                Tone::Info,
                ". centres the selected module in the rack view (v)".to_string(),
                now,
            );
            return;
        }
        if self.selected_index().is_none() {
            self.say(Tone::Trouble, "no module is selected".to_string(), now);
            return;
        }
        self.rack.centre = true;
    }

    fn jump_key(&mut self, key: KeyEvent, mut text: String, now: Instant) -> Mode {
        match key.code {
            KeyCode::Esc => return Mode::Normal,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c) if !c.is_control() && text.chars().count() < MAX_ENTRY => {
                text.push(c);
            }
            KeyCode::Enter => {
                match self.jump_target(&text) {
                    Some(index) => {
                        self.focus = Focus::Wall;
                        self.select(index, Some(0));
                        self.rack.centre = true;
                        let id = self.modules()[index].id.clone();
                        self.say(Tone::Info, format!("at {id} (g jumps again)"), now);
                    }
                    None if text.trim().is_empty() => {}
                    None => self.say(
                        Tone::Trouble,
                        format!(
                            "nothing on the wall is called '{}': type a module's id, name or kind",
                            text.trim()
                        ),
                        now,
                    ),
                }
                return Mode::Normal;
            }
            _ => {}
        }
        Mode::Jump { text }
    }

    fn switch_view(&mut self, now: Instant) {
        self.rack.drag = None;
        self.view = match self.view {
            View::List => {
                self.say(
                    Tone::Info,
                    "rack view: drag a knob up or down, scroll it for fine turns, drag from a \
                     jack to a jack to patch, right-click a cable to unplug · v for the list"
                        .to_string(),
                    now,
                );
                View::Rack
            }
            View::Rack => {
                self.say(Tone::Info, "list view · v for the rack".to_string(), now);
                View::List
            }
        };
        if !self.shows(self.focus) {
            self.focus = Focus::Wall;
        }
    }

    fn change_glide(&mut self, up: bool, now: Instant) {
        self.glide = if up {
            (self.glide + 1).min(GLIDES.len() - 1)
        } else {
            self.glide.saturating_sub(1)
        };
        self.say(
            Tone::Info,
            format!(
                "glide {} for the turns from now on",
                glide_words(self.glide_beats())
            ),
            now,
        );
    }

    fn start_wall(&mut self, now: Instant) {
        if self.link.is_up() {
            self.say(Tone::Info, "the wall is already playing".to_string(), now);
        } else if self.launching {
            self.say(Tone::Info, "the wall is starting…".to_string(), now);
        } else {
            self.launching = true;
            self.jobs.push(Job::Launch);
            self.say(Tone::Info, "starting the wall…".to_string(), now);
        }
    }

    /// Make the wall heard, or silent again (it plays on either way).
    fn toggle_monitor(&mut self, now: Instant) {
        let heard = self.snapshot.as_ref().map(|snapshot| snapshot.heard);
        match heard {
            Some(heard) if self.link.is_up() => {
                self.jobs.push(Job::Call(Request::Monitor { on: !heard }));
            }
            _ => self.say(
                Tone::Trouble,
                "the wall is not answering, so there is nothing to hear".to_string(),
                now,
            ),
        }
    }

    /// Start recording what the wall plays, or stop the recording under
    /// way (whoever started it).
    fn toggle_recording(&mut self, now: Instant) {
        let recording = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.recording.is_some());
        match recording {
            Some(recording) if self.link.is_up() => {
                self.jobs
                    .push(Job::Call(Request::Record { on: !recording }));
                let words = if recording {
                    "stopping the recording…"
                } else {
                    "starting a recording…"
                };
                self.say(Tone::Info, words.to_string(), now);
            }
            _ => self.say(
                Tone::Trouble,
                "the wall is not answering, so there is nothing to record".to_string(),
                now,
            ),
        }
    }

    fn ask_stop(&mut self, now: Instant) {
        if self.link.is_up() {
            self.mode = Mode::Stop;
        } else {
            self.say(
                Tone::Trouble,
                "the wall is not answering, so there is nothing to stop".to_string(),
                now,
            );
        }
    }

    fn confirm_stop(&mut self, key: KeyEvent, now: Instant) -> Mode {
        if matches!(key.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter) {
            self.jobs.push(Job::Call(Request::Shutdown));
            self.say(Tone::Info, "asking the wall to stop…".to_string(), now);
        } else {
            self.say(Tone::Info, "the wall keeps playing".to_string(), now);
        }
        Mode::Normal
    }

    // Moving -----------------------------------------------------------------

    fn move_across(&mut self, right: bool) {
        if self.focus != Focus::Wall {
            return;
        }
        let Some(index) = self.selected_index() else {
            return;
        };
        // The rack view goes by what is beside it on screen.
        let across = (self.view == View::Rack)
            .then(|| self.rack.layout.across(index, right))
            .flatten();
        let target = across.unwrap_or_else(|| {
            if right {
                (index + 1).min(self.modules().len().saturating_sub(1))
            } else {
                index.saturating_sub(1)
            }
        });
        self.select(target, None);
    }

    fn move_along(&mut self, down: bool) {
        match self.focus {
            Focus::Cables => {
                let count = self.cables().len();
                self.cable_cursor = step_cursor(self.cable_cursor, count, down);
            }
            Focus::Log => {
                self.log_cursor = step_cursor(self.log_cursor, self.log.len(), down);
            }
            Focus::Wall => self.move_knob(down),
        }
    }

    fn move_knob(&mut self, down: bool) {
        let Some(index) = self.selected_index() else {
            return;
        };
        let knobs = self.modules()[index].knobs.len();
        if down && self.knob + 1 < knobs {
            self.knob += 1;
        } else if !down && self.knob > 0 {
            self.knob -= 1;
        } else if let Some(next) = self.module_beside(index, down) {
            let knob = if down {
                0
            } else {
                self.modules()[next].knobs.len().saturating_sub(1)
            };
            self.select(next, Some(knob));
        }
    }

    /// The module below module `index` (or above): in the rack view, the
    /// faceplate on the next row down (or up) nearest straight below it;
    /// in the list view, the panel a row of panels on.
    fn module_beside(&self, index: usize, down: bool) -> Option<usize> {
        if self.view == View::Rack {
            return self.rack.layout.beside(index, down);
        }
        let columns = self.columns.max(1);
        if down {
            Some(index + columns).filter(|&below| below < self.modules().len())
        } else {
            index.checked_sub(columns)
        }
    }

    /// Select module `index`, at knob `knob` (or the same row, held to its
    /// knobs).
    pub(super) fn select(&mut self, index: usize, knob: Option<usize>) {
        let Some(module) = self.modules().get(index) else {
            return;
        };
        let last = module.knobs.len().saturating_sub(1);
        let id = module.id.clone();
        self.knob = knob.unwrap_or(self.knob).min(last);
        self.selected = Some(id);
        self.selected_hint = index;
    }

    pub(super) fn selected_module(&self) -> Option<&ModuleView> {
        self.selected_index().map(|index| &self.modules()[index])
    }

    pub(super) fn selected_knob(&self) -> Option<(&ModuleView, &KnobView)> {
        let module = self.selected_module()?;
        Some((module, module.knobs.get(self.knob)?))
    }

    // Turning ----------------------------------------------------------------

    pub(super) fn turn(&mut self, up: bool, step: Step, now: Instant) {
        if self.focus != Focus::Wall {
            self.say(
                Tone::Info,
                "turn knobs on the wall: Tab back to it".to_string(),
                now,
            );
            return;
        }
        let Some((module, knob)) = self.selected_knob() else {
            self.say(Tone::Trouble, "no knob is selected".to_string(), now);
            return;
        };
        let travel = Travel::of(knob, self.knob_info(&module.kind, &knob.name));
        let (id, name) = (module.id.clone(), knob.name.clone());
        let from = self.ahead(&id, &name).unwrap_or(knob.target);
        let to = travel.turned(from, up, step);
        if travel.same(from, to) {
            let end = if up { "top" } else { "bottom" };
            self.say(
                Tone::Info,
                format!("{id} {name} is at the {end} of its travel"),
                now,
            );
            return;
        }
        self.hold_turn(&id, &name, to, TurnKind::Keys, None, now);
    }

    fn open_value(&mut self, now: Instant) {
        if self.focus != Focus::Wall {
            return;
        }
        match self.selected_knob() {
            Some((module, knob)) => {
                self.mode = Mode::Value {
                    module: module.id.clone(),
                    knob: knob.name.clone(),
                    text: String::new(),
                };
            }
            None => self.say(Tone::Trouble, "no knob is selected".to_string(), now),
        }
    }

    fn value_key(
        &mut self,
        key: KeyEvent,
        module: String,
        knob: String,
        mut text: String,
        now: Instant,
    ) -> Mode {
        match key.code {
            KeyCode::Esc => return Mode::Normal,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c) if !c.is_control() && text.chars().count() < MAX_ENTRY => {
                text.push(c);
            }
            KeyCode::Enter => {
                self.set_value(&module, &knob, &text, now);
                return Mode::Normal;
            }
            _ => {}
        }
        Mode::Value { module, knob, text }
    }

    /// Turn `module`'s `knob` to what was typed: a number, or one of the
    /// knob's named positions.
    fn set_value(&mut self, module: &str, knob: &str, text: &str, now: Instant) {
        let Some(view) = self
            .module(module)
            .and_then(|view| view.knobs.iter().find(|candidate| candidate.name == knob))
        else {
            self.say(Tone::Trouble, format!("{module} {knob} has gone"), now);
            return;
        };
        let kind = self
            .module(module)
            .map(|view| view.kind.clone())
            .unwrap_or_default();
        let info = self.knob_info(&kind, knob);
        let travel = Travel::of(view, info);
        let typed = text.trim();
        let labelled = info.and_then(|info| {
            info.labels
                .iter()
                .position(|label| label.eq_ignore_ascii_case(typed))
                .map(|index| travel.min + index as f64)
        });
        let value = match (labelled, typed.parse::<f64>()) {
            (Some(value), _) => value,
            (None, Ok(value)) if value.is_finite() => travel.clamp(value),
            (None, Ok(_) | Err(_)) => {
                let named = info
                    .filter(|info| !info.labels.is_empty())
                    .map_or_else(String::new, |info| {
                        format!(" or one of {}", info.labels.join(", "))
                    });
                self.say(
                    Tone::Trouble,
                    format!(
                        "'{typed}' is not a value for {module} {knob}: type a number from {} to {}{named}",
                        format::number(travel.min as f32),
                        format::number(travel.max as f32)
                    ),
                    now,
                );
                return;
            }
        };
        self.hold_turn(module, knob, value, TurnKind::Now, None, now);
        self.tick(now);
    }

    // Tempo ------------------------------------------------------------------

    fn tempo_key(&mut self, key: KeyEvent, mut text: String, now: Instant) -> Mode {
        match key.code {
            KeyCode::Esc => return Mode::Normal,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c) if (c.is_ascii_digit() || c == '.') && text.len() < MAX_ENTRY => {
                text.push(c);
            }
            KeyCode::Enter => {
                match text.parse::<f64>() {
                    Ok(bpm) if (20.0..=300.0).contains(&bpm) => {
                        self.jobs.push(Job::Call(Request::Tempo { bpm }));
                    }
                    Ok(_) | Err(_) => self.say(
                        Tone::Trouble,
                        format!("'{text}' is not a tempo: type 20 to 300 BPM"),
                        now,
                    ),
                }
                return Mode::Normal;
            }
            _ => {}
        }
        Mode::Tempo { text }
    }

    // Adding and removing ----------------------------------------------------

    /// Open the picker: in the rack view, for a module to go right after
    /// the selected one; in the list view, where the wall puts it.
    fn open_picker(&mut self, now: Instant) {
        let place = if self.view == View::Rack {
            self.after_selected()
        } else {
            None
        };
        self.open_picker_at(place, now);
    }

    /// Open the picker for a module to go at `place` (or where the wall
    /// puts it).
    pub(super) fn open_picker_at(&mut self, place: Option<Place>, now: Instant) {
        if self.catalogue.is_empty() {
            self.say(
                Tone::Trouble,
                "the catalogue has not arrived from the wall yet".to_string(),
                now,
            );
        } else {
            self.mode = Mode::Add {
                index: 0,
                filter: String::new(),
                place,
            };
        }
    }

    /// The place right after the selected module, on a wall that keeps
    /// rows.
    fn after_selected(&self) -> Option<Place> {
        let id = &self.selected_module()?.id;
        let rows = self.shown_rows()?;
        rows.iter().enumerate().find_map(|(row, ids)| {
            let at = ids.iter().position(|other| other == id)?;
            Some(Place {
                row,
                before: ids.get(at + 1).cloned(),
                own: false,
            })
        })
    }

    fn add_key(
        &mut self,
        key: KeyEvent,
        (index, mut filter, place): (usize, String, Option<Place>),
        now: Instant,
    ) -> Mode {
        let count = self.picker_for(&filter).len();
        let kind = self
            .picker_for(&filter)
            .get(index)
            .map(|(_, info)| info.kind.clone());
        let index = match key.code {
            KeyCode::Esc if filter.is_empty() => return Mode::Normal,
            KeyCode::Esc => {
                filter.clear();
                0
            }
            KeyCode::Up => step_cursor(index, count, false),
            KeyCode::Down => step_cursor(index, count, true),
            KeyCode::PageUp => index.saturating_sub(PICKER_PAGE),
            KeyCode::PageDown => (index + PICKER_PAGE).min(count.saturating_sub(1)),
            KeyCode::Backspace => {
                filter.pop();
                0
            }
            KeyCode::Enter => {
                let Some(kind) = kind else {
                    self.say(
                        Tone::Trouble,
                        format!(
                            "no kind of module matches '{filter}': Backspace or Esc to widen it"
                        ),
                        now,
                    );
                    return Mode::Add {
                        index,
                        filter,
                        place,
                    };
                };
                self.add_module(kind, None, place, now);
                return Mode::Normal;
            }
            KeyCode::Tab => {
                let Some(kind) = kind else {
                    return Mode::Add {
                        index,
                        filter,
                        place,
                    };
                };
                return Mode::Name {
                    kind,
                    text: String::new(),
                    place,
                };
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && filter.chars().count() < MAX_ENTRY =>
            {
                filter.push(c);
                0
            }
            _ => {
                self.say(
                    Tone::Info,
                    "type to find a kind · ↑↓ choose · Enter adds it · Tab names it first · Esc cancels"
                        .to_string(),
                    now,
                );
                index
            }
        };
        Mode::Add {
            index,
            filter,
            place,
        }
    }

    /// Ask the wall for a module of `kind`, named `name`, at `place`.
    fn add_module(
        &mut self,
        kind: String,
        name: Option<String>,
        place: Option<Place>,
        now: Instant,
    ) {
        let words = place
            .as_ref()
            .map_or_else(String::new, |place| format!(" {}", self.place_words(place)));
        self.say(Tone::Info, format!("adding {kind}{words}…"), now);
        self.jobs
            .push(Job::Call(Request::Add { kind, name, place }));
    }

    /// Where `place` is, in words: `to row 2 after vcf1`, `at the start of
    /// row 1`, `on a new row`.
    #[must_use]
    pub fn place_words(&self, place: &Place) -> String {
        let rows = self.shown_rows().unwrap_or_default();
        if place.own || place.row >= rows.len() {
            return "on a new row".to_string();
        }
        let row = &rows[place.row];
        let number = place.row + 1;
        let at = place
            .before
            .as_ref()
            .and_then(|before| row.iter().position(|id| id == before))
            .unwrap_or(row.len());
        at.checked_sub(1)
            .and_then(|previous| row.get(previous))
            .map_or_else(
                || format!("at the start of row {number}"),
                |previous| format!("to row {number} after {previous}"),
            )
    }

    fn name_key(
        &mut self,
        key: KeyEvent,
        (kind, mut text, place): (String, String, Option<Place>),
        now: Instant,
    ) -> Mode {
        match key.code {
            KeyCode::Esc => return Mode::Normal,
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(c) if c.is_ascii() && !c.is_ascii_control() => {
                if text.len() < kazoo_wall::protocol::MAX_NAME {
                    text.push(c);
                }
            }
            KeyCode::Enter => {
                let name = text.trim();
                if name.is_empty() {
                    self.add_module(kind, None, place, now);
                    return Mode::Normal;
                }
                if valid_name(name) {
                    let name = name.to_string();
                    self.add_module(kind, Some(name), place, now);
                    return Mode::Normal;
                }
                self.say(
                    Tone::Trouble,
                    "a name is 1 to 24 of A-Z a-z 0-9 space _ . -".to_string(),
                    now,
                );
            }
            _ => {}
        }
        Mode::Name { kind, text, place }
    }

    fn ask_remove(&mut self, now: Instant) {
        if self.focus != Focus::Wall {
            self.say(
                Tone::Info,
                "remove modules on the wall: Tab back to it".to_string(),
                now,
            );
            return;
        }
        let Some(module) = self.selected_module() else {
            self.say(Tone::Trouble, "no module is selected".to_string(), now);
            return;
        };
        let id = module.id.clone();
        let prefix = format!("{id}.");
        let cables = self
            .cables()
            .iter()
            .filter(|cable| cable.from.starts_with(&prefix) || cable.to.starts_with(&prefix))
            .count();
        self.mode = Mode::Remove { module: id, cables };
    }

    fn confirm_remove(&mut self, key: KeyEvent, module: String, now: Instant) -> Mode {
        if matches!(key.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter) {
            self.jobs.push(Job::Call(Request::Remove { module }));
        } else {
            self.say(Tone::Info, format!("{module} stays on the wall"), now);
        }
        Mode::Normal
    }

    // Cables -----------------------------------------------------------------

    fn start_patch(&mut self, now: Instant) {
        let start = self
            .selected_module()
            .filter(|module| !module.outputs.is_empty())
            .or_else(|| {
                self.modules()
                    .iter()
                    .find(|module| !module.outputs.is_empty())
            })
            .map(|module| module.id.clone());
        match start {
            Some(module) => {
                self.focus = Focus::Wall;
                self.follow(&module, None);
                self.mode = Mode::Patch(Patching::From { module, port: 0 });
            }
            None => self.say(
                Tone::Trouble,
                "nothing on the wall has an output to patch from".to_string(),
                now,
            ),
        }
    }

    /// Move the wall's selection to follow the patch cursor, so the view
    /// scrolls with it.
    fn follow(&mut self, module: &str, knob: Option<&str>) {
        let Some(index) = self.modules().iter().position(|view| view.id == module) else {
            return;
        };
        let knob = knob.and_then(|name| {
            self.modules()[index]
                .knobs
                .iter()
                .position(|view| view.name == name)
        });
        self.select(index, Some(knob.unwrap_or(0)));
    }

    /// The next module after (or before) `from` that `keep` accepts.
    fn next_module(&self, from: &str, right: bool, keep: fn(&ModuleView) -> bool) -> String {
        let modules = self.modules();
        let Some(start) = modules.iter().position(|module| module.id == from) else {
            return from.to_string();
        };
        let found = if right {
            modules[start + 1..].iter().find(|module| keep(module))
        } else {
            modules[..start].iter().rev().find(|module| keep(module))
        };
        found.map_or_else(|| from.to_string(), |module| module.id.clone())
    }

    fn patch_key(&mut self, key: KeyEvent, step: Patching, now: Instant) -> Mode {
        match step {
            Patching::From { module, port } => self.patch_from_key(key, module, port),
            Patching::To { from, module, jack } => self.patch_to_key(key, from, module, jack),
            Patching::Amount {
                from,
                to,
                amount,
                replaces,
            } => self.patch_amount_key(key, from, to, amount, replaces, now),
        }
    }

    fn patch_from_key(&mut self, key: KeyEvent, module: String, port: usize) -> Mode {
        let outputs = self.module(&module).map_or(0, |view| view.outputs.len());
        let (module, port) = match key.code {
            KeyCode::Esc => {
                self.status = None;
                return Mode::Normal;
            }
            KeyCode::Left | KeyCode::Char('h') => {
                (self.next_module(&module, false, has_outputs), 0)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                (self.next_module(&module, true, has_outputs), 0)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let port = step_cursor(port, outputs, false);
                (module, port)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let port = step_cursor(port, outputs, true);
                (module, port)
            }
            KeyCode::Enter => {
                let Some(name) = self
                    .module(&module)
                    .and_then(|view| view.outputs.get(port))
                    .cloned()
                else {
                    return Mode::Patch(Patching::From { module, port });
                };
                return Mode::Patch(Patching::To {
                    from: format!("{module}.{name}"),
                    module,
                    jack: 0,
                });
            }
            _ => (module, port),
        };
        self.follow(&module, None);
        Mode::Patch(Patching::From { module, port })
    }

    fn patch_to_key(&mut self, key: KeyEvent, from: String, module: String, jack: usize) -> Mode {
        let names: Vec<String> = self
            .module(&module)
            .map(|view| jack_names(view).into_iter().map(str::to_string).collect())
            .unwrap_or_default();
        let (module, jack) = match key.code {
            KeyCode::Esc => {
                let back = jack_module(&from).to_string();
                let port = split_jack(&from, true)
                    .and_then(|jack| {
                        self.module(&back).and_then(|view| {
                            view.outputs.iter().position(|name| *name == jack.name)
                        })
                    })
                    .unwrap_or(0);
                self.follow(&back, None);
                return Mode::Patch(Patching::From { module: back, port });
            }
            KeyCode::Left | KeyCode::Char('h') => (self.next_module(&module, false, has_jacks), 0),
            KeyCode::Right | KeyCode::Char('l') => (self.next_module(&module, true, has_jacks), 0),
            KeyCode::Up | KeyCode::Char('k') => {
                let jack = step_cursor(jack, names.len(), false);
                (module, jack)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let jack = step_cursor(jack, names.len(), true);
                (module, jack)
            }
            KeyCode::Enter => {
                let Some(name) = names.get(jack) else {
                    return Mode::Patch(Patching::To { from, module, jack });
                };
                let to = format!("{module}.{name}");
                let replaces = self
                    .cables()
                    .iter()
                    .find(|cable| cable.to == to)
                    .map(|cable| cable.id);
                return Mode::Patch(Patching::Amount {
                    from,
                    to,
                    amount: 1.0,
                    replaces,
                });
            }
            _ => (module, jack),
        };
        let knob = self
            .module(&module)
            .and_then(|view| jack_names(view).get(jack).map(|name| (*name).to_string()));
        self.follow(&module, knob.as_deref());
        Mode::Patch(Patching::To { from, module, jack })
    }

    fn patch_amount_key(
        &mut self,
        key: KeyEvent,
        from: String,
        to: String,
        amount: f64,
        replaces: Option<u32>,
        now: Instant,
    ) -> Mode {
        let nudge = |by: f64| ((amount + by).clamp(-1.0, 1.0) * 100.0).round() / 100.0;
        let amount = match key.code {
            KeyCode::Esc => {
                let module = jack_module(&to).to_string();
                let jack = self
                    .module(&module)
                    .and_then(|view| {
                        let name = split_jack(&to, false)?.name;
                        jack_names(view)
                            .iter()
                            .position(|candidate| *candidate == name)
                    })
                    .unwrap_or(0);
                return Mode::Patch(Patching::To { from, module, jack });
            }
            KeyCode::Char('=' | 'l') | KeyCode::Right | KeyCode::Up => nudge(AMOUNT_STEP),
            KeyCode::Char('-' | 'h') | KeyCode::Left | KeyCode::Down => nudge(-AMOUNT_STEP),
            KeyCode::Char('+') => nudge(AMOUNT_FINE),
            KeyCode::Char('_') => nudge(-AMOUNT_FINE),
            KeyCode::Char('i') => -amount,
            KeyCode::Enter => {
                self.jobs.push(Job::Call(Request::Patch {
                    from,
                    to,
                    amount: Some(amount),
                }));
                self.say(Tone::Info, "plugging in…".to_string(), now);
                return Mode::Normal;
            }
            _ => amount,
        };
        Mode::Patch(Patching::Amount {
            from,
            to,
            amount,
            replaces,
        })
    }

    fn unplug(&mut self, now: Instant) {
        match self.focus {
            Focus::Cables => {
                let cable = self.cables().get(self.cable_cursor).map(|cable| cable.id);
                match cable {
                    Some(cable) => self.jobs.push(Job::Call(Request::Unpatch {
                        cable: Some(cable),
                        to: None,
                    })),
                    None => self.say(Tone::Trouble, "there are no cables".to_string(), now),
                }
            }
            Focus::Wall => {
                let Some((module, knob)) = self.selected_knob() else {
                    self.say(Tone::Trouble, "no knob is selected".to_string(), now);
                    return;
                };
                let to = format!("{}.{}", module.id, knob.name);
                let cable = self
                    .cables()
                    .iter()
                    .find(|cable| cable.to == to)
                    .map(|cable| cable.id);
                match cable {
                    Some(cable) => self.jobs.push(Job::Call(Request::Unpatch {
                        cable: Some(cable),
                        to: None,
                    })),
                    None => self.say(
                        Tone::Trouble,
                        format!(
                            "nothing is plugged into {to}: Tab to the cable list to pick any cable"
                        ),
                        now,
                    ),
                }
            }
            Focus::Log => self.say(
                Tone::Info,
                "pick a cable in the cable list (Tab) to unplug it".to_string(),
                now,
            ),
        }
    }

    // Undo -------------------------------------------------------------------

    fn undo(&mut self, now: Instant) {
        // A move made here is put back first, while it was the last thing
        // done: it is not in the log.
        if self.focus != Focus::Log && self.undo_move(now) {
            return;
        }
        let change = if self.focus == Focus::Log {
            self.log.get(self.log_cursor).and_then(|line| line.seq)
        } else {
            self.log.last_undoable()
        };
        match change {
            Some(change) => self.jobs.push(Job::Call(Request::Undo { change })),
            None if self.focus == Focus::Log => self.say(
                Tone::Trouble,
                "that line is not a change, so it cannot be undone".to_string(),
                now,
            ),
            None => self.say(
                Tone::Trouble,
                "nothing in the log is left to undo".to_string(),
                now,
            ),
        }
    }
}

const fn coarse_if(alt: bool, step: Step) -> Step {
    if alt { Step::Coarse } else { step }
}
