//! Per-view state structs for the TUI.
//!
//! Each view (Mixer, Tracking, Project, Audio I/O) owns its own navigation
//! and selection state. This keeps the monolithic [`super::app::App`] struct
//! from growing without bound and makes view-specific concerns explicit.

use kazoo_core::mixer::clip::ClipId;

// ---------------------------------------------------------------------------
// Active view
// ---------------------------------------------------------------------------

/// Which top-level view is displayed in the content area.
///
/// Switched via number keys `1`–`4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActiveView {
    /// Mixing desk / console view (key: 1).
    Mixer,
    /// Tracking / arrangement timeline (key: 2).
    Tracking,
    /// Project setup (tempo, metronome, loops) (key: 3).
    Project,
    /// Audio device I/O configuration (key: 4).
    AudioIO,
}

impl ActiveView {
    /// All views in display order.
    pub const ALL: [Self; 4] = [Self::Mixer, Self::Tracking, Self::Project, Self::AudioIO];

    /// Short label for view tab display.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Mixer => "Mixer",
            Self::Tracking => "Track",
            Self::Project => "Setup",
            Self::AudioIO => "I/O",
        }
    }

    /// The number key (1-4) that activates this view.
    #[must_use]
    pub const fn key_number(self) -> u8 {
        match self {
            Self::Mixer => 1,
            Self::Tracking => 2,
            Self::Project => 3,
            Self::AudioIO => 4,
        }
    }
}

// ---------------------------------------------------------------------------
// Input mode (shared across views that edit parameters)
// ---------------------------------------------------------------------------

/// Input sub-mode for parameter editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Normal navigation keybindings.
    Normal,
    /// Editing a parameter value (captures numeric/text input).
    ParameterEdit,
}

// ---------------------------------------------------------------------------
// Synth view state
// ---------------------------------------------------------------------------

/// Navigation and selection state for the Synth/Effects view.
///
/// Some fields are reserved for future synth view input handling (module-level
/// focus, expanded effect card, per-view parameter editing).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SynthViewState {
    /// Which module card has focus (0=voice input, 1=synth engine, 2=effects chain, 3=layers).
    pub selected_module: usize,
    /// Which effect in the effects chain is selected.
    pub selected_effect: usize,
    /// Which parameter within the active module is selected.
    pub selected_param: usize,
    /// Which effect card is expanded for detailed editing.
    pub expanded_effect: Option<usize>,
    /// Text buffer for numeric input in `ParameterEdit` mode.
    pub param_edit_buffer: String,
    /// Current input sub-mode.
    pub input_mode: InputMode,
    /// Whether the synth entry is selected (vs an effect) in the effects list.
    pub synth_selected: bool,
    /// Index of the selected synth parameter (in the primary layer).
    pub selected_synth_param: usize,
}

impl Default for SynthViewState {
    fn default() -> Self {
        Self {
            selected_module: 0,
            selected_effect: 0,
            selected_param: 0,
            expanded_effect: None,
            param_edit_buffer: String::new(),
            input_mode: InputMode::Normal,
            synth_selected: true,
            selected_synth_param: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Mixer view state
// ---------------------------------------------------------------------------

/// Which control is focused within a mixer channel strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerControl {
    /// Volume fader.
    Fader,
    /// Pan knob.
    Pan,
    /// Solo button.
    Solo,
    /// Mute button.
    Mute,
    /// Arm (record enable) button.
    Arm,
}

impl MixerControl {
    /// Cycle to the next control (downward on the strip).
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Fader => Self::Pan,
            Self::Pan => Self::Solo,
            Self::Solo => Self::Mute,
            Self::Mute => Self::Arm,
            Self::Arm => Self::Fader,
        }
    }

    /// Cycle to the previous control (upward on the strip).
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Fader => Self::Arm,
            Self::Pan => Self::Fader,
            Self::Solo => Self::Pan,
            Self::Mute => Self::Solo,
            Self::Arm => Self::Mute,
        }
    }
}

/// Navigation and selection state for the Mixing Desk view.
///
/// `param_edit_buffer` and `input_mode` are reserved for future per-view
/// parameter editing (direct numeric input on fader/pan values).
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MixerViewState {
    /// Which channel strip has focus (index into tracks, or `tracks.len()` for master).
    pub selected_channel: usize,
    /// First visible channel when scrolling (for > ~8 tracks).
    pub channel_scroll: usize,
    /// Which control within the strip is focused.
    pub selected_control: MixerControl,
    /// Text buffer for numeric input.
    pub param_edit_buffer: String,
    /// Current input sub-mode.
    pub input_mode: InputMode,
}

impl Default for MixerViewState {
    fn default() -> Self {
        Self {
            selected_channel: 0,
            channel_scroll: 0,
            selected_control: MixerControl::Fader,
            param_edit_buffer: String::new(),
            input_mode: InputMode::Normal,
        }
    }
}

// ---------------------------------------------------------------------------
// Tracking view state
// ---------------------------------------------------------------------------

/// Navigation and selection state for the Tracking / arrangement view.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TrackingViewState {
    /// Zoom factor: samples per pixel. Higher = more zoomed out.
    pub timeline_zoom: f64,
    /// Horizontal scroll position in samples.
    pub timeline_scroll: f64,
    /// Currently selected clip ID, if any.
    pub selected_clip: Option<ClipId>,
    /// Which track lane has focus (for vertical navigation).
    pub selected_track_lane: usize,
    /// Waveform display zoom factor (1.0 = fit entire buffer).
    pub waveform_zoom: f32,
    /// Waveform display horizontal scroll position (0.0–1.0).
    pub waveform_scroll: f32,
}

impl Default for TrackingViewState {
    fn default() -> Self {
        Self {
            timeline_zoom: 256.0,
            timeline_scroll: 0.0,
            selected_clip: None,
            selected_track_lane: 0,
            waveform_zoom: 1.0,
            waveform_scroll: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Project view state
// ---------------------------------------------------------------------------

/// Navigation state for the Project Setup view.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProjectViewState {
    /// Which settings card has focus (0=tempo, 1=time sig, 2=count-in,
    /// 3=metronome, 4=loop, 5=recording).
    pub selected_card: usize,
    /// Which field within the active card is selected.
    pub selected_field: usize,
    /// Text buffer for numeric input.
    pub param_edit_buffer: String,
    /// Current input sub-mode.
    pub input_mode: InputMode,
}

impl Default for ProjectViewState {
    fn default() -> Self {
        Self {
            selected_card: 0,
            selected_field: 0,
            param_edit_buffer: String::new(),
            input_mode: InputMode::Normal,
        }
    }
}

// ---------------------------------------------------------------------------
// Audio I/O view state
// ---------------------------------------------------------------------------

/// Which device list has focus in the Audio I/O view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceListFocus {
    /// Input (capture) device list.
    Input,
    /// Output (playback) device list.
    Output,
    /// Settings panel.
    Settings,
}

/// Navigation state for the Audio I/O view.
#[derive(Debug, Clone)]
pub struct AudioIOViewState {
    /// Which device list / section has focus.
    pub focus: DeviceListFocus,
    /// Selected device index in the input list.
    pub selected_input_device: usize,
    /// Selected device index in the output list.
    pub selected_output_device: usize,
    /// Cached input device names (populated at startup).
    pub input_devices: Vec<String>,
    /// Cached output device names (populated at startup).
    pub output_devices: Vec<String>,
}

impl Default for AudioIOViewState {
    fn default() -> Self {
        Self {
            focus: DeviceListFocus::Input,
            selected_input_device: 0,
            selected_output_device: 0,
            input_devices: Vec::new(),
            output_devices: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_view_labels_are_not_empty() {
        for view in ActiveView::ALL {
            assert!(!view.label().is_empty());
        }
    }

    #[test]
    fn active_view_key_numbers_are_1_through_4() {
        for (i, view) in ActiveView::ALL.iter().enumerate() {
            assert_eq!(view.key_number(), (i + 1) as u8);
        }
    }

    #[test]
    fn mixer_control_next_prev_inverse() {
        for ctrl in [
            MixerControl::Fader,
            MixerControl::Pan,
            MixerControl::Solo,
            MixerControl::Mute,
            MixerControl::Arm,
        ] {
            assert_eq!(ctrl.next().prev(), ctrl);
            assert_eq!(ctrl.prev().next(), ctrl);
        }
    }

    #[test]
    fn default_states_are_sane() {
        let synth = SynthViewState::default();
        assert_eq!(synth.selected_module, 0);
        assert_eq!(synth.input_mode, InputMode::Normal);

        let mixer = MixerViewState::default();
        assert_eq!(mixer.selected_channel, 0);
        assert_eq!(mixer.selected_control, MixerControl::Fader);

        let tracking = TrackingViewState::default();
        assert!((tracking.timeline_zoom - 256.0).abs() < f64::EPSILON);
        assert!(tracking.selected_clip.is_none());

        let project = ProjectViewState::default();
        assert_eq!(project.selected_card, 0);

        let audio_io = AudioIOViewState::default();
        assert_eq!(audio_io.focus, DeviceListFocus::Input);
    }
}
