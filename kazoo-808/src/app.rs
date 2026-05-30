//! Application state for the 808 drum machine.

use crate::sequencer::{STEPS_PER_PATTERN, Sequencer};
use crate::synth::{MAX_PARAMS_PER_VOICE, VOICE_COUNT, VoiceIndex, VoiceParam};

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
    pub fn new(sample_rate: f32) -> Self {
        Self {
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
}
