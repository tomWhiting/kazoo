//! Application state for the 808 drum machine.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::audio::{AudioCommand, CommandSender};
use kazoo_808::sequencer::{MAX_PATTERNS, STEPS_PER_PATTERN, Sequencer};
use kazoo_808::synth::{MAX_PARAMS_PER_VOICE, VOICE_COUNT, VoiceIndex, VoiceParam};
use kazoo_core::ipc::link::LinkStatus;

/// BPM change per `+`/`-` press.
const BPM_STEP: f64 = 1.0;
/// Swing change per `[`/`]` press, in percent.
const SWING_STEP: f64 = 1.0;
/// Velocity of a manual audition trigger.
const AUDITION_VELOCITY: f32 = 0.8;

/// Keys that pick a pattern in pattern-select mode, in bank order.
const PATTERN_KEYS: [char; MAX_PATTERNS] = [
    '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', 'a', 'b', 'c', 'd', 'e', 'f',
];

/// A one-line notice for the status bar about something the user asked
/// for that did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// `n` pressed with every pattern slot in use.
    PatternBankFull,
    /// Pattern-select mode got a key for a pattern that does not exist.
    NoSuchPattern,
}

impl Notice {
    /// Text shown in the status bar.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::PatternBankFull => "pattern bank full",
            Self::NoSuchPattern => "no such pattern",
        }
    }
}

/// Audio stream health, read from the engine each frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamHealth {
    /// Errors cpal reported for the output stream.
    pub errors: u64,
    /// The output device is gone: no more audio until restart.
    pub lost: bool,
}

/// Which section of the UI has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Step sequencer grid.
    Grid,
    /// Voice parameter editor.
    Params,
}

/// Top-level application state (UI side).
///
/// The audio thread owns its own `DrumMachine` and receives commands via
/// channel. This struct holds UI-side state: cursor position, selected
/// voice, focus mode, and the sequencer patterns (shared with audio via
/// commands).
#[derive(Debug)]
pub struct App {
    /// Commands to the audio thread, with failed deliveries counted.
    pub commands: CommandSender,
    /// Last request that could not be carried out, until the next key.
    pub notice: Option<Notice>,
    /// Output stream health, mirrored from the audio side.
    pub stream: StreamHealth,
    /// Desk link state; `None` until the first poll.
    pub hub: Option<LinkStatus>,
    /// Transport messages lost between the 808 and the desk.
    pub desk_lost: u64,
    /// Whether the app should exit.
    pub should_quit: bool,
    /// Currently selected voice row.
    pub selected_voice: usize,
    /// Current cursor column in the grid (0..15).
    pub cursor_step: usize,
    /// Which UI section has focus.
    pub focus: Focus,
    /// Sequencer state (pattern data lives here; audio thread gets triggers).
    pub sequencer: Sequencer,
    /// Current playback step (updated from audio thread via atomic).
    pub playback_step: usize,
    /// Selected parameter index within the current voice's param list.
    pub selected_param: usize,
    /// UI-side mirror of voice parameter values.
    /// Indexed by `[voice_idx][param_idx]`, actual (denormalized) values.
    pub param_values: [[f32; MAX_PARAMS_PER_VOICE]; VOICE_COUNT],
    /// Whether the help overlay is visible.
    pub show_help: bool,
    /// Whether pattern select mode is active (waiting for number key).
    pub pattern_select_mode: bool,
}

impl App {
    #[must_use]
    pub fn new(sample_rate: f32, commands: CommandSender) -> Self {
        Self {
            commands,
            notice: None,
            stream: StreamHealth::default(),
            hub: None,
            desk_lost: 0,
            should_quit: false,
            selected_voice: 0,
            cursor_step: 0,
            focus: Focus::Grid,
            sequencer: Sequencer::new(sample_rate),
            playback_step: 0,
            selected_param: 0,
            param_values: Self::init_param_values(),
            show_help: false,
            pattern_select_mode: false,
        }
    }

    /// Initialize parameter values from synth defaults.
    fn init_param_values() -> [[f32; MAX_PARAMS_PER_VOICE]; VOICE_COUNT] {
        let mut values = [[0.0_f32; MAX_PARAMS_PER_VOICE]; VOICE_COUNT];
        for voice in VoiceIndex::ALL {
            let params = VoiceParam::for_voice(voice);
            for (idx, param) in params.iter().enumerate() {
                values[voice as usize][idx] = param.default_actual(voice);
            }
        }
        values
    }

    /// Move cursor left, or decrease parameter in Params focus.
    /// Returns `Some((voice, param, actual_value))` if a parameter was adjusted.
    pub fn cursor_left(&mut self) -> Option<(VoiceIndex, VoiceParam, f32)> {
        match self.focus {
            Focus::Grid => {
                if self.cursor_step > 0 {
                    self.cursor_step -= 1;
                } else {
                    self.cursor_step = STEPS_PER_PATTERN - 1;
                }
                None
            }
            Focus::Params => self.adjust_param(-0.05),
        }
    }

    /// Move cursor right, or increase parameter in Params focus.
    /// Returns `Some((voice, param, actual_value))` if a parameter was adjusted.
    pub fn cursor_right(&mut self) -> Option<(VoiceIndex, VoiceParam, f32)> {
        match self.focus {
            Focus::Grid => {
                self.cursor_step = (self.cursor_step + 1) % STEPS_PER_PATTERN;
                None
            }
            Focus::Params => self.adjust_param(0.05),
        }
    }

    /// Move cursor up (previous voice row or previous parameter).
    pub const fn cursor_up(&mut self) {
        match self.focus {
            Focus::Grid => {
                if self.selected_voice > 0 {
                    self.selected_voice -= 1;
                } else {
                    self.selected_voice = VOICE_COUNT - 1;
                }
                self.selected_param = 0;
            }
            Focus::Params => {
                if self.selected_param > 0 {
                    self.selected_param -= 1;
                }
            }
        }
    }

    /// Move cursor down (next voice row or next parameter).
    pub fn cursor_down(&mut self) {
        match self.focus {
            Focus::Grid => {
                self.selected_voice = (self.selected_voice + 1) % VOICE_COUNT;
                self.selected_param = 0;
            }
            Focus::Params => {
                let params = VoiceParam::for_voice(self.selected_voice_index());
                if self.selected_param + 1 < params.len() {
                    self.selected_param += 1;
                }
            }
        }
    }

    /// Toggle the step at the cursor position.
    pub fn toggle_current_step(&mut self) {
        self.sequencer
            .toggle_step(self.selected_voice, self.cursor_step);
    }

    /// Toggle accent on the step at the cursor position.
    pub fn toggle_current_accent(&mut self) {
        self.sequencer
            .toggle_accent(self.selected_voice, self.cursor_step);
    }

    /// Cycle focus between grid and params.
    pub const fn cycle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Grid => Focus::Params,
            Focus::Params => Focus::Grid,
        };
        self.selected_param = 0;
    }

    /// Select a voice by number key (1-9 = voices 0-8, 0 = voice 9).
    pub const fn select_voice_by_key(&mut self, key: char) {
        let idx = match key {
            '1' => 0,
            '2' => 1,
            '3' => 2,
            '4' => 3,
            '5' => 4,
            '6' => 5,
            '7' => 6,
            '8' => 7,
            '9' => 8,
            '0' => 9,
            _ => return,
        };
        if idx < VOICE_COUNT {
            self.selected_voice = idx;
            self.selected_param = 0;
        }
    }

    /// Get the `VoiceIndex` for the currently selected voice.
    #[must_use]
    pub fn selected_voice_index(&self) -> VoiceIndex {
        VoiceIndex::from_index(self.selected_voice).unwrap_or(VoiceIndex::Kick)
    }

    /// Adjust the currently selected parameter by a normalized delta (-1.0 to 1.0).
    /// Returns the voice, param, and new actual value for sending to the audio thread.
    fn adjust_param(&mut self, delta_normalized: f32) -> Option<(VoiceIndex, VoiceParam, f32)> {
        let voice = self.selected_voice_index();
        let params = VoiceParam::for_voice(voice);
        if self.selected_param >= params.len() {
            return None;
        }
        let param = params[self.selected_param];
        let (min, max) = param.range(voice);
        let current = self.param_values[self.selected_voice][self.selected_param];
        let delta_actual = delta_normalized * (max - min);
        let new_val = (current + delta_actual).clamp(min, max);
        self.param_values[self.selected_voice][self.selected_param] = new_val;
        Some((voice, param, new_val))
    }

    /// Get the normalized value (0.0-1.0) of a parameter for display.
    #[must_use]
    pub fn param_normalized(&self, voice_idx: usize, param_idx: usize) -> f32 {
        if let Some(voice) = VoiceIndex::from_index(voice_idx) {
            let params = VoiceParam::for_voice(voice);
            if param_idx < params.len() {
                let param = params[param_idx];
                let actual = self.param_values[voice_idx][param_idx];
                return param.normalize(voice, actual);
            }
        }
        0.5
    }

    /// Handle a key press. Every change that the audio thread must mirror
    /// is sent first and applied to the UI only once it was queued, so the
    /// screen never shows a state the engine does not have.
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        self.notice = None;

        // Help overlay: any key dismisses it.
        if self.show_help {
            self.show_help = false;
            return;
        }

        // Pattern select mode: the next key picks a pattern or cancels.
        if self.pattern_select_mode {
            self.pattern_select_mode = false;
            if let KeyCode::Char(c) = key.code {
                self.select_pattern_by_key(c);
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('p') => self.pattern_select_mode = true,
            KeyCode::Char('n') => self.add_pattern(),
            KeyCode::Left => {
                if let Some(change) = self.cursor_left() {
                    self.send_param(change);
                }
            }
            KeyCode::Right => {
                if let Some(change) = self.cursor_right() {
                    self.send_param(change);
                }
            }
            KeyCode::Up => self.cursor_up(),
            KeyCode::Down => self.cursor_down(),
            KeyCode::Char(' ') => self.toggle_step_at_cursor(),
            KeyCode::Char('a') => self.toggle_accent_at_cursor(),
            KeyCode::Enter => self.toggle_playback(),
            KeyCode::Tab => self.cycle_focus(),
            KeyCode::Char(c @ '0'..='9') => self.select_voice_by_key(c),
            KeyCode::Char('+' | '=') => self.nudge_bpm(BPM_STEP),
            KeyCode::Char('-') => self.nudge_bpm(-BPM_STEP),
            KeyCode::Char(']') => self.nudge_swing(SWING_STEP),
            KeyCode::Char('[') => self.nudge_swing(-SWING_STEP),
            KeyCode::Char('t') => {
                // An audition has no UI state to keep in step; a failed
                // send is counted by the sender and shown.
                self.commands.post(AudioCommand::TriggerVoice {
                    voice: self.selected_voice,
                    velocity: AUDITION_VELOCITY,
                });
            }
            _ => {}
        }
    }

    fn select_pattern_by_key(&mut self, key: char) {
        let Some(idx) = PATTERN_KEYS.iter().position(|&k| k == key) else {
            // Any other key cancels pattern select.
            return;
        };
        if idx >= self.sequencer.patterns.len() {
            self.notice = Some(Notice::NoSuchPattern);
        } else if self.commands.send(AudioCommand::SelectPattern(idx)) {
            self.sequencer.select_pattern(idx);
        }
    }

    fn add_pattern(&mut self) {
        if !self.sequencer.can_add_pattern() {
            self.notice = Some(Notice::PatternBankFull);
        } else if self.commands.send(AudioCommand::AddPattern) {
            if let Some(idx) = self.sequencer.add_pattern() {
                self.sequencer.select_pattern(idx);
            }
        }
    }

    fn send_param(&mut self, (voice, param, value): (VoiceIndex, VoiceParam, f32)) {
        // The UI mirror was already updated by `adjust_param`; a failed send
        // is shown, and the next adjustment re-sends the absolute value.
        self.commands.post(AudioCommand::SetVoiceParam {
            voice,
            param,
            value,
        });
    }

    fn toggle_step_at_cursor(&mut self) {
        let cmd = AudioCommand::ToggleStep {
            voice: self.selected_voice,
            step: self.cursor_step,
        };
        if self.commands.send(cmd) {
            self.toggle_current_step();
        }
    }

    fn toggle_accent_at_cursor(&mut self) {
        let cmd = AudioCommand::ToggleAccent {
            voice: self.selected_voice,
            step: self.cursor_step,
        };
        if self.commands.send(cmd) {
            self.toggle_current_accent();
        }
    }

    fn toggle_playback(&mut self) {
        let play = !self.sequencer.playing;
        let cmd = if play {
            AudioCommand::Play
        } else {
            AudioCommand::Stop
        };
        if self.commands.send(cmd) {
            self.sequencer.playing = play;
        }
    }

    fn nudge_bpm(&mut self, delta: f64) {
        let bpm = self.sequencer.clock.bpm() + delta;
        if self.commands.send(AudioCommand::SetBpm(bpm)) {
            self.sequencer.clock.set_bpm(bpm);
        }
    }

    fn nudge_swing(&mut self, delta: f64) {
        let swing = self.sequencer.clock.swing() + delta;
        if self.commands.send(AudioCommand::SetSwing(swing)) {
            self.sequencer.clock.set_swing(swing);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::SendFailure;
    use crossbeam_channel::Receiver;

    fn app_with_capacity(capacity: usize) -> (App, Receiver<AudioCommand>) {
        let (tx, rx) = crossbeam_channel::bounded(capacity);
        (App::new(48_000.0, CommandSender::new(tx)), rx)
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn step_toggle_is_sent_then_shown() {
        let (mut app, rx) = app_with_capacity(8);
        press(&mut app, KeyCode::Char(' '));
        assert!(app.sequencer.current_pattern_ref().steps[0][0].active);
        assert_eq!(
            rx.try_recv().unwrap(),
            AudioCommand::ToggleStep { voice: 0, step: 0 }
        );
    }

    #[test]
    fn undelivered_edits_do_not_change_the_screen() {
        let (mut app, _rx) = app_with_capacity(1);
        press(&mut app, KeyCode::Enter); // fills the queue
        assert!(app.sequencer.playing);

        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Char('+'));
        press(&mut app, KeyCode::Char('n'));
        assert!(!app.sequencer.current_pattern_ref().steps[0][0].active);
        assert!((app.sequencer.clock.bpm() - 120.0).abs() < f64::EPSILON);
        assert_eq!(app.sequencer.patterns.len(), 1);
        assert_eq!(app.commands.failed(), 3);
        assert_eq!(app.commands.last_failure(), Some(SendFailure::QueueFull));
    }

    #[test]
    fn swing_keys_adjust_swing() {
        let (mut app, rx) = app_with_capacity(8);
        press(&mut app, KeyCode::Char(']'));
        assert!((app.sequencer.clock.swing() - 51.0).abs() < f64::EPSILON);
        assert_eq!(rx.try_recv().unwrap(), AudioCommand::SetSwing(51.0));
        press(&mut app, KeyCode::Char('['));
        press(&mut app, KeyCode::Char('['));
        // Clamped at straight time.
        assert!((app.sequencer.clock.swing() - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn new_pattern_is_added_and_selected() {
        let (mut app, rx) = app_with_capacity(8);
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.sequencer.patterns.len(), 2);
        assert_eq!(app.sequencer.current_pattern, 1);
        assert_eq!(rx.try_recv().unwrap(), AudioCommand::AddPattern);
    }

    #[test]
    fn full_pattern_bank_is_reported() {
        let (mut app, rx) = app_with_capacity(MAX_PATTERNS * 2);
        for _ in 1..MAX_PATTERNS {
            press(&mut app, KeyCode::Char('n'));
        }
        assert_eq!(app.sequencer.patterns.len(), MAX_PATTERNS);
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.notice, Some(Notice::PatternBankFull));
        assert_eq!(rx.len(), MAX_PATTERNS - 1, "no command for a full bank");
    }

    #[test]
    fn every_pattern_is_selectable() {
        let (mut app, _rx) = app_with_capacity(MAX_PATTERNS * 4);
        for _ in 1..MAX_PATTERNS {
            press(&mut app, KeyCode::Char('n'));
        }
        for (idx, key) in PATTERN_KEYS.iter().enumerate() {
            press(&mut app, KeyCode::Char('p'));
            press(&mut app, KeyCode::Char(*key));
            assert_eq!(app.sequencer.current_pattern, idx);
        }
    }

    #[test]
    fn selecting_a_missing_pattern_is_reported() {
        let (mut app, rx) = app_with_capacity(8);
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Char('5'));
        assert_eq!(app.notice, Some(Notice::NoSuchPattern));
        assert_eq!(app.sequencer.current_pattern, 0);
        assert!(rx.is_empty());

        // The next key clears the notice.
        press(&mut app, KeyCode::Down);
        assert_eq!(app.notice, None);
    }

    #[test]
    fn quit_keys_quit() {
        let (mut app, _rx) = app_with_capacity(1);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);

        let (mut app, _rx) = app_with_capacity(1);
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }
}
