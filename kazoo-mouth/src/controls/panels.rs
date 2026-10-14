use std::time::Instant;

use crossbeam_channel::TrySendError;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use kazoo_fx::Curve;
use ratatui::layout::Rect;

use super::{Action, App, CommandSender, Control, Focus, SHAPES, Settings};
use crate::engine::{Command, fx::Slot};

pub const SOURCE: [Control; 5] = [
    Control::Shape,
    Control::Cutoff,
    Control::Air,
    Control::Volume,
    Control::Silent,
];
pub const PITCH: [Control; 7] = [
    Control::Quantise,
    Control::Scale,
    Control::Root,
    Control::Snap,
    Control::Vibrato,
    Control::Hysteresis,
    Control::Blend,
];
pub const GATE: [Control; 3] = [Control::GateOpen, Control::GateClose, Control::GateHold];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Control(Control),
    Shape(usize),
    FxOn(Slot),
    FxSlot(Slot),
    FxCell(usize),
}

#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub area: Rect,
    pub bar: Rect,
    pub target: Target,
}

impl Focus {
    pub const ALL: [Self; 4] = [Self::Synth, Self::Pitch, Self::Gate, Self::Fx];

    pub const fn controls(self) -> &'static [Control] {
        match self {
            Self::Synth => &SOURCE,
            Self::Pitch => &PITCH,
            Self::Gate => &GATE,
            Self::Fx => &[],
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::Synth => "SOURCE",
            Self::Pitch => "PITCH",
            Self::Gate => "GATE",
            Self::Fx => "FX",
        }
    }
}

impl Control {
    pub const fn panel(self) -> Focus {
        match self {
            Self::Shape | Self::Cutoff | Self::Air | Self::Volume | Self::Silent => Focus::Synth,
            Self::GateOpen | Self::GateClose | Self::GateHold => Focus::Gate,
            _ => Focus::Pitch,
        }
    }
}

impl Settings {
    pub const fn dial(&self, control: Control) -> Option<(f32, f32, f32)> {
        Some(match control {
            Control::Cutoff => (100.0, 12_000.0, self.synth.cutoff_hz),
            Control::Air => (0.0, 1.0, self.synth.air),
            Control::Volume => (0.0, 1.0, self.synth.volume),
            Control::Snap => (0.0, 400.0, self.quantise.snap_ms),
            Control::Hysteresis => (0.0, 50.0, self.quantise.hysteresis_cents),
            Control::Blend => (0.0, 1.0, self.quantise.blend),
            Control::GateOpen => (-70.0, -10.0, self.gate.open_db),
            Control::GateClose => (-80.0, -15.0, self.gate.close_db),
            Control::GateHold => (0.0, 300.0, self.gate.hold_ms),
            _ => return None,
        })
    }

    pub fn set_fraction(&mut self, control: Control, fraction: f32) -> Option<Command> {
        let (min, max, _) = self.dial(control)?;
        let value = fraction.clamp(0.0, 1.0).mul_add(max - min, min);
        Some(match control {
            Control::Cutoff => {
                self.synth.cutoff_hz = value;
                Command::SetSynth(self.synth)
            }
            Control::Air => {
                self.synth.air = value;
                Command::SetSynth(self.synth)
            }
            Control::Volume => {
                self.synth.volume = value;
                Command::SetSynth(self.synth)
            }
            Control::Snap => {
                self.quantise.snap_ms = value;
                Command::SetQuantise(self.quantise)
            }
            Control::Hysteresis => {
                self.quantise.hysteresis_cents = value;
                Command::SetQuantise(self.quantise)
            }
            Control::Blend => {
                self.quantise.blend = value;
                if value > 0.0 {
                    self.last_blend = value;
                }
                Command::SetQuantise(self.quantise)
            }
            Control::GateOpen => {
                self.gate.open_db = value;
                self.gate = self.gate.held();
                Command::SetGate(self.gate)
            }
            Control::GateClose => {
                self.gate.close_db = value;
                self.gate = self.gate.held();
                Command::SetGate(self.gate)
            }
            Control::GateHold => {
                self.gate.hold_ms = value;
                self.gate = self.gate.held();
                Command::SetGate(self.gate)
            }
            _ => return None,
        })
    }

    pub fn restore(&mut self, control: Control) -> Command {
        let defaults = Self::default();
        if let Some((min, max, value)) = defaults.dial(control)
            && let Some(command) = self.set_fraction(control, (value - min) / (max - min))
        {
            return command;
        }
        match control {
            Control::Shape => {
                self.synth.shape = defaults.synth.shape;
                Command::SetSynth(self.synth)
            }
            Control::Silent => {
                self.silent = defaults.silent;
                Command::SetSilent(self.silent)
            }
            Control::Quantise => {
                self.quantise.blend = defaults.quantise.blend;
                Command::SetQuantise(self.quantise)
            }
            Control::Scale => {
                self.quantise.scale = defaults.quantise.scale;
                Command::SetQuantise(self.quantise)
            }
            Control::Root => {
                self.quantise.root = defaults.quantise.root;
                Command::SetQuantise(self.quantise)
            }
            Control::Vibrato => {
                self.quantise.keep_vibrato = defaults.quantise.keep_vibrato;
                Command::SetQuantise(self.quantise)
            }
            _ => Command::SetGate(self.gate),
        }
    }
}

impl App {
    pub fn description(&self) -> &'static str {
        if self.focus == Focus::Fx {
            let saved = self.accepted.fx.slots[self.selected_slot as usize];
            self.selected_slot
                .kind(saved.active)
                .map_or("Effect controls", |kind| kind.description)
        } else {
            self.selected.help()
        }
    }

    fn fx_fraction(
        proposed: &mut Settings,
        slot: Slot,
        cell: usize,
        fraction: f32,
    ) -> Option<Command> {
        let saved = &mut proposed.fx.slots[slot as usize];
        let spec = slot.kind(saved.active)?.params.get(cell - 1)?;
        let fraction = fraction.clamp(0.0, 1.0);
        let value = spec.clamp(match spec.curve {
            Curve::Log => spec.min * (spec.max / spec.min).powf(fraction),
            _ => fraction.mul_add(spec.max - spec.min, spec.min),
        });
        saved.values[saved.active][cell - 1] = value;
        Some(Command::FxParam {
            slot,
            kind: saved.active as u8,
            index: (cell - 1) as u8,
            value,
        })
    }

    pub fn next_waveform(
        &mut self,
        sender: &impl CommandSender,
        now: Instant,
    ) -> Result<Action, TrySendError<Command>> {
        let mut proposed = self.accepted;
        let index = SHAPES
            .iter()
            .position(|shape| *shape == proposed.synth.shape)
            .unwrap_or(0);
        proposed.synth.shape = SHAPES[(index + 1) % SHAPES.len()];
        self.send_change(&proposed, Command::SetSynth(proposed.synth), sender, now)
    }

    pub fn next_panel(&mut self, forward: bool) {
        let index = Focus::ALL
            .iter()
            .position(|focus| *focus == self.focus)
            .unwrap_or(0);
        self.focus = Focus::ALL[(index + if forward { 1 } else { 3 }) % 4];
        if let Some(control) = self.focus.controls().first() {
            self.selected = *control;
        }
    }

    pub fn next_control(&mut self, forward: bool) {
        let controls = self.selected.panel().controls();
        let index = controls
            .iter()
            .position(|control| *control == self.selected)
            .unwrap_or(0);
        self.selected =
            controls[(index + if forward { 1 } else { controls.len() - 1 }) % controls.len()];
        self.focus = self.selected.panel();
    }

    pub fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        sender: &impl CommandSender,
        now: Instant,
    ) -> Result<Action, TrySendError<Command>> {
        if self.help {
            return Ok(Action::Continue);
        }
        let position = ratatui::layout::Position::new(mouse.column, mouse.row);
        let hit = self
            .hits
            .borrow()
            .iter()
            .find(|hit| hit.area.contains(position))
            .copied();
        let Some(hit) = hit else {
            return Ok(Action::Continue);
        };
        let click = matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
        let drag = matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left));
        let wheel = match mouse.kind {
            MouseEventKind::ScrollUp => Some(true),
            MouseEventKind::ScrollDown => Some(false),
            _ => None,
        };
        if !click && !drag && wheel.is_none() {
            return Ok(Action::Continue);
        }
        let mut proposed = self.accepted;
        let fraction = f32::from(mouse.column.saturating_sub(hit.bar.x))
            / f32::from(hit.bar.width.saturating_sub(1).max(1));
        let command = match hit.target {
            Target::Control(control) => {
                self.selected = control;
                self.focus = control.panel();
                if let Some(right) = wheel {
                    Some(proposed.adjust(control, right, false))
                } else if hit.bar.contains(position) {
                    proposed.set_fraction(control, fraction)
                } else if click && proposed.dial(control).is_none() {
                    Some(proposed.adjust(control, true, false))
                } else {
                    None
                }
            }
            Target::Shape(index) if click => {
                self.selected = Control::Shape;
                self.focus = Focus::Synth;
                proposed.synth.shape = SHAPES[index];
                Some(Command::SetSynth(proposed.synth))
            }
            Target::FxOn(slot) if click => {
                self.selected_slot = slot;
                self.focus = Focus::Fx;
                let saved = &mut proposed.fx.slots[slot as usize];
                saved.on = !saved.on;
                Some(Command::FxOn { slot, on: saved.on })
            }
            Target::FxSlot(slot) if click => {
                self.selected_slot = slot;
                self.selected_cell = 0;
                self.focus = Focus::Fx;
                None
            }
            Target::FxCell(cell) => {
                self.selected_cell = cell;
                self.focus = Focus::Fx;
                if let Some(right) = wheel {
                    return self.fx_key(
                        KeyEvent::new(
                            if right { KeyCode::Right } else { KeyCode::Left },
                            KeyModifiers::NONE,
                        ),
                        sender,
                        now,
                    );
                }
                if cell == 0 {
                    if click {
                        return self.fx_key(
                            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
                            sender,
                            now,
                        );
                    }
                    None
                } else if hit.bar.contains(position) {
                    Self::fx_fraction(&mut proposed, self.selected_slot, cell, fraction)
                } else {
                    None
                }
            }
            _ => None,
        };
        command.map_or(Ok(Action::Continue), |command| {
            self.send_change(&proposed, command, sender, now)
        })
    }

    pub fn help_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('/') => {
                self.help_filter = Some(String::new());
                self.help_scroll.set(0);
            }
            KeyCode::Char(character) if self.help_filter.is_some() => {
                if let Some(filter) = &mut self.help_filter {
                    filter.push(character);
                }
                self.help_scroll.set(0);
            }
            KeyCode::Backspace if self.help_filter.is_some() => {
                if let Some(filter) = &mut self.help_filter {
                    filter.pop();
                }
                self.help_scroll.set(0);
            }
            KeyCode::Down => self
                .help_scroll
                .set(self.help_scroll.get().saturating_add(1)),
            KeyCode::Up => self
                .help_scroll
                .set(self.help_scroll.get().saturating_sub(1)),
            KeyCode::PageDown => self
                .help_scroll
                .set(self.help_scroll.get().saturating_add(10)),
            KeyCode::PageUp => self
                .help_scroll
                .set(self.help_scroll.get().saturating_sub(10)),
            _ => {}
        }
    }
}
