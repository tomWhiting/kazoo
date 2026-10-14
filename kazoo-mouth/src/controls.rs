use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, TrySendError};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use kazoo_core::synthesis::OscillatorShape;
use kazoo_core::voice::quantise::{NOTE_NAMES, QuantiseSettings, SCALE_NAMES};

use crate::engine::fx::{FxSettings, Slot, mouth_default};
use crate::engine::looper::{
    BAR_CHOICES, LoopFeed, LoopGauges, LoopLength, Note, State, VOLUME_DEFAULT,
};
use crate::engine::{Command, Stats};
use kazoo_fx::Curve;
use ringbuf::traits::Producer;

pub(crate) mod settings;
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
    pub(crate) last_blend: f32,
    pub(crate) fx: FxSettings,
    pub(crate) own_bpm: f64,
    pub(crate) loop_length: LoopLength,
    pub(crate) loop_volume: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            quantise: QuantiseSettings::default(),
            synth: SynthSettings::default(),
            silent: false,
            last_blend: 1.0,
            fx: FxSettings::default(),
            own_bpm: 120.0,
            loop_length: LoopLength::Free,
            loop_volume: VOLUME_DEFAULT,
        }
    }
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

    pub(crate) fn value(&self, control: Control) -> String {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Synth,
    Fx,
}

#[derive(Debug)]
pub(crate) struct App {
    pub(crate) accepted: Settings,
    pub(crate) selected: Control,
    pub(crate) help: bool,
    pub(crate) help_scroll: Cell<u16>,
    pub(crate) focus: Focus,
    pub(crate) selected_slot: Slot,
    pub(crate) selected_cell: usize,
    pub(crate) desk_bpm: Option<f64>,
    pub(crate) moving: u64,
    pub(crate) fx_faults: u64,
    pub(crate) fx_refused: u64,
    pub(crate) settings_path: PathBuf,
    busy_until: Option<Instant>,
    dirty_at: Option<Instant>,
    save_error: Option<String>,
    writable: bool,
    message: Option<(String, Instant)>,
    notes: Vec<String>,
    notes_at: Instant,
    published_own: u64,
    pub(crate) loop_feed: Option<LoopFeed>,
    loop_note_seq: u32,
    save_pending: bool,
    pub(crate) loop_sample_rate: u32,
}

impl App {
    #[cfg(test)]
    pub(crate) fn new(quantise: QuantiseSettings, silent: bool) -> Self {
        let quantise = quantise.held();
        let accepted = Settings {
            quantise,
            silent,
            last_blend: if quantise.blend > 0.0 {
                quantise.blend
            } else {
                1.0
            },
            ..Settings::default()
        };
        Self::loaded(
            settings::Loaded {
                settings: accepted,
                notes: Vec::new(),
                writable: true,
            },
            PathBuf::new(),
            Instant::now(),
        )
    }

    pub(crate) fn loaded(loaded: settings::Loaded, path: PathBuf, now: Instant) -> Self {
        Self {
            accepted: loaded.settings,
            selected: Control::Quantise,
            help: false,
            help_scroll: Cell::new(0),
            focus: Focus::Synth,
            selected_slot: Slot::Megaphone,
            selected_cell: 0,
            desk_bpm: None,
            moving: 0,
            fx_faults: 0,
            fx_refused: 0,
            settings_path: path,
            busy_until: None,
            dirty_at: None,
            save_error: None,
            writable: loaded.writable,
            message: None,
            notes: loaded.notes,
            notes_at: now,
            published_own: loaded.settings.own_bpm.to_bits(),
            loop_feed: None,
            loop_note_seq: 0,
            save_pending: false,
            loop_sample_rate: 48_000,
        }
    }

    pub(crate) fn busy(&self, now: Instant) -> bool {
        self.busy_until.is_some_and(|until| now < until)
    }

    pub(crate) fn loop_gauges(&self) -> Option<&LoopGauges> {
        self.loop_feed.as_ref().map(|feed| feed.gauges.as_ref())
    }

    pub(crate) fn message(&mut self, message: String, now: Instant) {
        self.message = Some((message, now + Duration::from_secs(3)));
    }

    pub(crate) const fn save_returned(&mut self) {
        self.save_pending = false;
    }

    pub(crate) const fn loop_save_pending(&self) -> bool {
        self.save_pending
    }

    pub(crate) fn sync_loop(&mut self, rate: u32, now: Instant) {
        let Some(gauges) = self.loop_gauges() else {
            return;
        };
        let sequence = gauges.note_seq.load(Ordering::Relaxed);
        if sequence == self.loop_note_seq {
            return;
        }
        let note = gauges.note.load(Ordering::Relaxed);
        let capacity = gauges.capacity.load(Ordering::Relaxed);
        let message = if note == Note::NoTape as u8 {
            "no tape: the loop could not get its memory".into()
        } else if note == Note::TooShort as u8 {
            "too short to loop".into()
        } else if note == Note::DoesNotFit as u8 {
            let bars = self.accepted.loop_length.bars();
            let bpm = self.desk_bpm.unwrap_or(self.accepted.own_bpm);
            format!(
                "{bars} bars at {bpm:.1} BPM need {:.2} s; the loop holds {:.2} s",
                f64::from(bars) * 240.0 / bpm,
                capacity as f64 / f64::from(rate)
            )
        } else if note == Note::ClearFirst as u8 {
            "clear the loop first (C) to change its length".into()
        } else if note == Note::Merging as u8 {
            "that layer is being merged into the one below and can no longer be undone".into()
        } else if note == Note::SaveCancelled as u8 {
            "save cancelled: the loop changed".into()
        } else {
            return;
        };
        self.loop_note_seq = sequence;
        self.message(message, now);
    }

    fn loop_key(
        &mut self,
        key: char,
        sender: &impl CommandSender,
        now: Instant,
    ) -> Result<Action, TrySendError<Command>> {
        let state = self.loop_gauges().map_or(State::Blank as u8, |gauges| {
            gauges.state.load(Ordering::Relaxed)
        });
        if state == State::NoTape as u8 {
            self.message("no tape: the loop could not get its memory".into(), now);
            return Ok(Action::Continue);
        }
        let mut proposed = self.accepted;
        let command = match key {
            'r' => Command::LoopRecord,
            'u' => Command::LoopUndo,
            'C' => Command::LoopClear,
            'o' => Command::LoopPlay(state == State::Stopped as u8),
            '-' | '=' | '_' | '+' => {
                let step = if matches!(key, '_' | '+') { 0.25 } else { 0.05 };
                let direction = if matches!(key, '-' | '_') { -1.0 } else { 1.0 };
                proposed.loop_volume = (proposed.loop_volume + direction * step).clamp(0.0, 1.0);
                Command::LoopVolume(proposed.loop_volume)
            }
            'l' => {
                if state != State::Blank as u8 {
                    self.message("clear the loop first (C) to change its length".into(), now);
                    return Ok(Action::Continue);
                }
                proposed.loop_length = self.next_loop_length(now);
                Command::LoopLength(proposed.loop_length)
            }
            'w' => {
                let n = self
                    .loop_gauges()
                    .map_or(0, |gauges| gauges.n.load(Ordering::Relaxed));
                if n == 0 {
                    self.message("nothing to save".into(), now);
                } else if self.save_pending
                    || self
                        .loop_gauges()
                        .is_some_and(|gauges| gauges.saving.load(Ordering::Relaxed))
                {
                    self.message("already saving".into(), now);
                } else if let Some(feed) = &mut self.loop_feed {
                    match feed.bounce_in.try_push(vec![0.0; n].into_boxed_slice()) {
                        Ok(()) => self.save_pending = true,
                        Err(samples) => {
                            drop(samples);
                            self.message("busy: save not sent".into(), now);
                        }
                    }
                }
                return Ok(Action::Continue);
            }
            _ => return Ok(Action::Continue),
        };
        self.send_change(&proposed, command, sender, now)
    }

    fn next_loop_length(&mut self, now: Instant) -> LoopLength {
        let mut length = self.accepted.loop_length;
        let choices = [
            0,
            BAR_CHOICES[0],
            BAR_CHOICES[1],
            BAR_CHOICES[2],
            BAR_CHOICES[3],
            BAR_CHOICES[4],
        ];
        let current = choices
            .iter()
            .position(|bars| *bars == length.bars())
            .unwrap_or(0);
        let capacity = self.loop_gauges().map_or(32.0, |gauges| {
            gauges.capacity.load(Ordering::Relaxed) as f64 / f64::from(self.loop_sample_rate)
        });
        let bpm = self.desk_bpm.unwrap_or(self.accepted.own_bpm);
        let mut skipped = Vec::new();
        for offset in 1..=choices.len() {
            let bars = choices[(current + offset) % choices.len()];
            if bars == 0 || f64::from(bars) * 240.0 / bpm <= capacity {
                length = if bars == 0 {
                    LoopLength::Free
                } else {
                    LoopLength::Bars(bars)
                };
                break;
            }
            skipped.push(format!(
                "{bars} bars need {:.2} s",
                f64::from(bars) * 240.0 / bpm
            ));
        }
        if !skipped.is_empty() {
            self.message(
                format!(
                    "skipped {} at {bpm:.1} BPM; the loop holds {capacity:.2} s",
                    skipped.join(", ")
                ),
                now,
            );
        }
        length
    }

    pub(crate) fn notice(&self, now: Instant) -> Option<&str> {
        if let Some(error) = &self.save_error {
            return Some(error);
        }
        if let Some((message, until)) = &self.message
            && now < *until
        {
            return Some(message);
        }
        let elapsed = now.saturating_duration_since(self.notes_at).as_secs() / 5;
        let index = elapsed.min(self.notes.len() as u64) as usize;
        self.notes.get(index).map(String::as_str)
    }

    pub(crate) fn sync_stats(&mut self, stats: &Stats, now: Instant) {
        let bits = stats.desk_bpm.load(Ordering::Relaxed);
        self.desk_bpm = if bits == 0 {
            None
        } else {
            Some(f64::from_bits(bits))
        };
        let own = f64::from_bits(stats.own_bpm.load(Ordering::Relaxed));
        if own.is_finite() && own >= 30.0 && own.to_bits() != self.published_own {
            let waiting = self.accepted.own_bpm.to_bits() != self.published_own;
            self.published_own = own.to_bits();
            if !waiting {
                self.accepted.own_bpm = own;
                self.dirty_at = Some(now);
            }
        }
        self.moving = stats.fx_moving.load(Ordering::Relaxed);
        self.fx_faults = stats.fx_faults.load(Ordering::Relaxed);
        self.fx_refused = stats.fx_refused.load(Ordering::Relaxed);
    }

    pub(crate) fn save_if_due(&mut self, now: Instant, quitting: bool) {
        if !self.writable || self.settings_path.as_os_str().is_empty() {
            return;
        }
        if !quitting
            && !self
                .dirty_at
                .is_some_and(|at| now.saturating_duration_since(at) >= Duration::from_secs(1))
        {
            return;
        }
        match settings::save(&self.settings_path, &self.accepted) {
            Ok(()) => {
                self.save_error = None;
                self.dirty_at = None;
            }
            Err(error) => {
                self.save_error = Some(format!("settings not saved: {error}"));
                self.dirty_at = None;
            }
        }
    }

    fn send_change(
        &mut self,
        proposed: &Settings,
        command: Command,
        sender: &impl CommandSender,
        now: Instant,
    ) -> Result<Action, TrySendError<Command>> {
        match sender.try_send(command) {
            Ok(()) => {
                self.accepted = *proposed;
                if !matches!(command, Command::SetSilent(_)) {
                    self.dirty_at = Some(now);
                }
            }
            Err(TrySendError::Full(_)) => self.busy_until = Some(now + Duration::from_secs(1)),
            Err(error @ TrySendError::Disconnected(_)) => return Err(error),
        }
        Ok(Action::Continue)
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
            self.help_scroll.set(0);
            return Ok(Action::Continue);
        }
        if let KeyCode::Char(key @ ('r' | 'u' | 'C' | 'l' | 'o' | '-' | '=' | '_' | '+' | 'w')) =
            key.code
        {
            return self.loop_key(key, sender, now);
        }
        if self.help {
            match key.code {
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
            return Ok(Action::Continue);
        }
        if key.code == KeyCode::Tab {
            self.focus = if self.focus == Focus::Synth {
                Focus::Fx
            } else {
                Focus::Synth
            };
            return Ok(Action::Continue);
        }
        if matches!(key.code, KeyCode::Char(',' | '.' | '<' | '>')) {
            return self.tempo_key(key.code, sender, now);
        }
        let global = matches!(key.code, KeyCode::Char(' ' | 'v' | 'V' | 's' | 'S'));
        if self.focus == Focus::Fx && !global {
            return self.fx_key(key, sender, now);
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
        let mut proposed = self.accepted;
        let command = proposed.adjust(
            control,
            key.code != KeyCode::Left,
            key.modifiers.contains(KeyModifiers::SHIFT),
        );
        self.send_change(&proposed, command, sender, now)
    }

    fn tempo_key(
        &mut self,
        code: KeyCode,
        sender: &impl CommandSender,
        now: Instant,
    ) -> Result<Action, TrySendError<Command>> {
        if let Some(bpm) = self.desk_bpm {
            self.message = Some((
                format!("the tempo is the desk's ({bpm:.1}): change it on the desk"),
                now + Duration::from_secs(2),
            ));
            return Ok(Action::Continue);
        }
        let step = match code {
            KeyCode::Char(',') => -1.0,
            KeyCode::Char('.') => 1.0,
            KeyCode::Char('<') => -10.0,
            _ => 10.0,
        };
        let mut proposed = self.accepted;
        proposed.own_bpm = (proposed.own_bpm + step).clamp(30.0, 300.0);
        self.send_change(&proposed, Command::OwnTempo(proposed.own_bpm), sender, now)
    }

    fn fx_key(
        &mut self,
        key: KeyEvent,
        sender: &impl CommandSender,
        now: Instant,
    ) -> Result<Action, TrySendError<Command>> {
        let slot = self.selected_slot;
        let mut proposed = self.accepted;
        let Some(position) = proposed.fx.order.iter().position(|entry| *entry == slot) else {
            return Ok(Action::Continue);
        };
        let saved = &mut proposed.fx.slots[slot as usize];
        let Some(description) = slot.kind(saved.active) else {
            return Ok(Action::Continue);
        };
        let cells = description.params.len() + 1;
        match key.code {
            KeyCode::Char('[' | ']') => {
                let next = if key.code == KeyCode::Char(']') {
                    (position + 1) % 5
                } else {
                    (position + 4) % 5
                };
                self.selected_slot = proposed.fx.order[next];
                self.selected_cell = 0;
                return Ok(Action::Continue);
            }
            KeyCode::Up | KeyCode::Down => {
                self.selected_cell = if key.code == KeyCode::Down {
                    (self.selected_cell + 1) % cells
                } else {
                    (self.selected_cell + cells - 1) % cells
                };
                return Ok(Action::Continue);
            }
            _ => {}
        }
        let command = match key.code {
            KeyCode::Char('b') => {
                saved.on = !saved.on;
                Command::FxOn { slot, on: saved.on }
            }
            KeyCode::Char('{' | '}') => {
                let next = if key.code == KeyCode::Char('}') {
                    (position + 1).min(4)
                } else {
                    position.saturating_sub(1)
                };
                proposed.fx.order.swap(position, next);
                Command::FxOrder(proposed.fx.order)
            }
            KeyCode::Left | KeyCode::Right | KeyCode::Char('d') => {
                let right = key.code == KeyCode::Right;
                if self.selected_cell == 0 {
                    saved.active = if key.code == KeyCode::Char('d') {
                        0
                    } else if right {
                        (saved.active + 1) % slot.ids().len()
                    } else {
                        (saved.active + slot.ids().len() - 1) % slot.ids().len()
                    };
                    Command::FxKind {
                        slot,
                        kind: saved.active as u8,
                    }
                } else {
                    let index = self.selected_cell - 1;
                    let Some(spec) = description.params.get(index) else {
                        return Ok(Action::Continue);
                    };
                    let value = if key.code == KeyCode::Char('d') {
                        mouth_default(slot, saved.active, spec)
                    } else {
                        turn(
                            spec,
                            saved.values[saved.active][index],
                            right,
                            key.modifiers.contains(KeyModifiers::SHIFT),
                        )
                    };
                    saved.values[saved.active][index] = value;
                    Command::FxParam {
                        slot,
                        kind: saved.active as u8,
                        index: index as u8,
                        value,
                    }
                }
            }
            _ => return Ok(Action::Continue),
        };
        self.send_change(&proposed, command, sender, now)
    }
}

fn turn(spec: &kazoo_fx::ParamSpec, value: f32, right: bool, coarse: bool) -> f32 {
    let direction: f32 = if right { 1.0 } else { -1.0 };
    let divisions = if coarse { 10.0 } else { 100.0 };
    let value = match spec.curve {
        Curve::Linear => direction.mul_add((spec.max - spec.min) / divisions, value),
        Curve::Log => value * (spec.max / spec.min).powf(direction / divisions),
        Curve::Stepped { .. } => value + direction,
    };
    spec.clamp(value)
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
