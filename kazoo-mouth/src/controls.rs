use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, TrySendError};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use kazoo_core::synthesis::OscillatorShape;
use kazoo_core::voice::quantise::{NOTE_NAMES, QuantiseSettings, SCALE_NAMES};

use crate::engine::Command;
use crate::synth::SynthSettings;

#[cfg(test)]
mod tests;

const SHAPES: [OscillatorShape; 4] = [
    OscillatorShape::Sine,
    OscillatorShape::Triangle,
    OscillatorShape::Saw,
    OscillatorShape::Square,
];

pub(crate) trait CommandSender {
    fn try_send(&self, command: Command) -> Result<(), TrySendError<Command>>;
}

impl CommandSender for Sender<Command> {
    fn try_send(&self, command: Command) -> Result<(), TrySendError<Command>> {
        Self::try_send(self, command)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    Quantise,
    Scale,
    Root,
    Snap,
    Vibrato,
    Hysteresis,
    Blend,
    Shape,
    Cutoff,
    Air,
    Volume,
    Silent,
}

impl Control {
    pub(crate) const ALL: [Self; 12] = [
        Self::Quantise,
        Self::Scale,
        Self::Root,
        Self::Snap,
        Self::Vibrato,
        Self::Hysteresis,
        Self::Blend,
        Self::Shape,
        Self::Cutoff,
        Self::Air,
        Self::Volume,
        Self::Silent,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Quantise => "quantise",
            Self::Scale => "scale",
            Self::Root => "root",
            Self::Snap => "snap",
            Self::Vibrato => "keep vibrato",
            Self::Hysteresis => "hysteresis",
            Self::Blend => "blend",
            Self::Shape => "shape",
            Self::Cutoff => "cutoff",
            Self::Air => "air",
            Self::Volume => "volume",
            Self::Silent => "silent",
        }
    }

    pub(crate) const fn help(self) -> &'static str {
        match self {
            Self::Quantise => "Pull to the scale, or sing hertz for hertz.",
            Self::Scale => "Choose which notes the voice is pulled to.",
            Self::Root => "Choose the scale's starting note.",
            Self::Snap => "Set the correction speed, from 0 to 400 ms.",
            Self::Vibrato => "Keep pitch wavering around the corrected note.",
            Self::Hysteresis => "Hold a target until another note wins by 0 to 50 cents.",
            Self::Blend => "Mix direct pitch with corrected pitch, from 0 to 1.",
            Self::Shape => "Choose sine, triangle, saw or square tone.",
            Self::Cutoff => "Move the filter from 100 to 12,000 Hz.",
            Self::Air => "Mix breath noise into the tone, from 0 to 1.",
            Self::Volume => "Set the synth's output level, from 0 to 1.",
            Self::Silent => "Publish the voice while muting the synth.",
        }
    }

    const fn next(self, forward: bool) -> Self {
        let index = self as usize;
        let index = if forward {
            (index + 1) % Self::ALL.len()
        } else {
            (index + Self::ALL.len() - 1) % Self::ALL.len()
        };
        Self::ALL[index]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Settings {
    pub(crate) quantise: QuantiseSettings,
    pub(crate) synth: SynthSettings,
    pub(crate) silent: bool,
    last_blend: f32,
}

impl Settings {
    const fn toggle_quantise(&mut self) {
        if self.quantise.blend > 0.0 {
            self.last_blend = self.quantise.blend;
            self.quantise.blend = 0.0;
        } else {
            self.quantise.blend = self.last_blend;
        }
    }

    fn adjust(&mut self, control: Control, right: bool, shift: bool) -> Command {
        let direction = if right { 1.0_f32 } else { -1.0_f32 };
        match control {
            Control::Quantise => self.toggle_quantise(),
            Control::Scale => {
                self.quantise.scale = index_step(self.quantise.scale, SCALE_NAMES.len(), right)
            }
            Control::Root => {
                self.quantise.root =
                    index_step(usize::from(self.quantise.root), NOTE_NAMES.len(), right) as u8
            }
            Control::Snap => {
                self.quantise.snap_ms = step(
                    self.quantise.snap_ms,
                    direction,
                    if shift { 50.0 } else { 5.0 },
                    400.0,
                )
            }
            Control::Vibrato => self.quantise.keep_vibrato = !self.quantise.keep_vibrato,
            Control::Hysteresis => {
                self.quantise.hysteresis_cents = step(
                    self.quantise.hysteresis_cents,
                    direction,
                    if shift { 25.0 } else { 5.0 },
                    50.0,
                )
            }
            Control::Blend => {
                self.quantise.blend = step(
                    self.quantise.blend,
                    direction,
                    if shift { 0.25 } else { 0.05 },
                    1.0,
                );
                if self.quantise.blend > 0.0 {
                    self.last_blend = self.quantise.blend;
                }
            }
            Control::Shape => {
                let index = match self.synth.shape {
                    OscillatorShape::Sine => 0,
                    OscillatorShape::Triangle => 1,
                    OscillatorShape::Saw => 2,
                    OscillatorShape::Square => 3,
                };
                self.synth.shape = SHAPES[index_step(index, SHAPES.len(), right)];
            }
            Control::Cutoff => {
                self.synth.cutoff_hz = (self.synth.cutoff_hz
                    * (direction / if shift { 1.0 } else { 12.0 }).exp2())
                .clamp(100.0, 12_000.0)
            }
            Control::Air => {
                self.synth.air = step(
                    self.synth.air,
                    direction,
                    if shift { 0.25 } else { 0.05 },
                    1.0,
                )
            }
            Control::Volume => {
                self.synth.volume = step(
                    self.synth.volume,
                    direction,
                    if shift { 0.25 } else { 0.05 },
                    1.0,
                )
            }
            Control::Silent => self.silent = !self.silent,
        }
        match control {
            Control::Shape | Control::Cutoff | Control::Air | Control::Volume => {
                Command::SetSynth(self.synth)
            }
            Control::Silent => Command::SetSilent(self.silent),
            _ => Command::SetQuantise(self.quantise),
        }
    }

    pub(crate) fn value(self, control: Control) -> String {
        match control {
            Control::Quantise => on_off(self.quantise.blend > 0.0).into(),
            Control::Scale => SCALE_NAMES[self.quantise.scale].into(),
            Control::Root => NOTE_NAMES[usize::from(self.quantise.root)].into(),
            Control::Snap => format!("{:.0} ms", self.quantise.snap_ms),
            Control::Vibrato => on_off(self.quantise.keep_vibrato).into(),
            Control::Hysteresis => format!("{:.0} cents", self.quantise.hysteresis_cents),
            Control::Blend => format!("{:.2}", self.quantise.blend),
            Control::Shape => match self.synth.shape {
                OscillatorShape::Sine => "sine",
                OscillatorShape::Triangle => "triangle",
                OscillatorShape::Saw => "saw",
                OscillatorShape::Square => "square",
            }
            .into(),
            Control::Cutoff => format!("{:.0} Hz", self.synth.cutoff_hz),
            Control::Air => format!("{:.2}", self.synth.air),
            Control::Volume => format!("{:.2}", self.synth.volume),
            Control::Silent => on_off(self.silent).into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Continue,
    Quit,
}

#[derive(Debug)]
pub(crate) struct App {
    pub(crate) accepted: Settings,
    pub(crate) selected: Control,
    pub(crate) help: bool,
    busy_until: Option<Instant>,
}

impl App {
    pub(crate) fn new(quantise: QuantiseSettings, silent: bool) -> Self {
        let quantise = quantise.held();
        Self {
            accepted: Settings {
                quantise,
                synth: SynthSettings::default(),
                silent,
                last_blend: if quantise.blend > 0.0 {
                    quantise.blend
                } else {
                    1.0
                },
            },
            selected: Control::Quantise,
            help: false,
            busy_until: None,
        }
    }

    pub(crate) fn busy(&self, now: Instant) -> bool {
        self.busy_until.is_some_and(|until| now < until)
    }

    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        sender: &impl CommandSender,
        now: Instant,
    ) -> Result<Action, TrySendError<Command>> {
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return Ok(Action::Continue);
        }
        if matches!(key.code, KeyCode::Char('c' | 'C'))
            && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return Ok(Action::Quit);
        }
        if key.code == KeyCode::Esc {
            if self.help {
                self.help = false;
                return Ok(Action::Continue);
            }
            return Ok(Action::Quit);
        }
        if key.code == KeyCode::Char('?') {
            self.help = !self.help;
            return Ok(Action::Continue);
        }
        if self.help {
            return Ok(Action::Continue);
        }
        let control = match key.code {
            KeyCode::Up | KeyCode::Down => {
                self.selected = self.selected.next(key.code == KeyCode::Down);
                return Ok(Action::Continue);
            }
            KeyCode::Left | KeyCode::Right => self.selected,
            KeyCode::Char(' ') => Control::Quantise,
            KeyCode::Char('v' | 'V') => Control::Vibrato,
            KeyCode::Char('s' | 'S') => Control::Silent,
            _ => return Ok(Action::Continue),
        };
        // Queue acceptance is the only point at which displayed values change.
        let mut proposed = self.accepted;
        let command = proposed.adjust(
            control,
            key.code != KeyCode::Left,
            key.modifiers.contains(KeyModifiers::SHIFT),
        );
        match sender.try_send(command) {
            Ok(()) => self.accepted = proposed,
            Err(TrySendError::Full(_)) => self.busy_until = Some(now + Duration::from_secs(1)),
            Err(error @ TrySendError::Disconnected(_)) => return Err(error),
        }
        Ok(Action::Continue)
    }
}

const fn index_step(value: usize, count: usize, right: bool) -> usize {
    if right {
        let next = value.saturating_add(1);
        if next < count { next } else { count - 1 }
    } else {
        value.saturating_sub(1)
    }
}

fn step(value: f32, direction: f32, amount: f32, max: f32) -> f32 {
    direction.mul_add(amount, value).clamp(0.0, max)
}

const fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}
