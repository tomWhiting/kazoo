//! Application state, event loop, and TUI coordination.
//!
//! The [`App`] struct is the central state container for the terminal UI.
//! It owns the [`EngineHandle`], maintains local track metadata, and drives
//! the main event loop that bridges keyboard input, engine display updates,
//! and frame rendering.

use std::io;
use std::time::Duration;

use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::widgets::ListState;

use kazoo_core::engine::{DisplayState, EngineCommand, EngineHandle};
use kazoo_core::mixer::TrackId;
use kazoo_core::synthesis::SynthesisMode;
use kazoo_core::{Db, Pan};

// Re-export state types so existing `use crate::app::*` imports keep working.
pub use crate::state::{
    ActiveView, AudioIOViewState, InputMode, MixerViewState, ProjectViewState, SynthViewState,
    TrackingViewState,
};

/// Target frames per second for UI rendering.
const TARGET_FPS: u64 = 60;

// ---------------------------------------------------------------------------
// Panel focus
// ---------------------------------------------------------------------------

/// Panels that can receive keyboard focus.
///
/// `Tab` cycles forward, `BackTab` (Shift+Tab) cycles backward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusedPanel {
    Transport,
    Tracks,
    Timeline,
    Waveform,
    Spectrum,
    Effects,
    Mixer,
}

impl FocusedPanel {
    /// Cycle to the next panel in the full (all-views) tab order.
    ///
    /// Note: the TUI now uses [`panels_for_view`] for view-aware cycling.
    /// These methods are retained for tests and potential programmatic use.
    #[must_use]
    #[allow(dead_code)]
    pub const fn next(self) -> Self {
        match self {
            Self::Transport => Self::Tracks,
            Self::Tracks => Self::Timeline,
            Self::Timeline => Self::Waveform,
            Self::Waveform => Self::Spectrum,
            Self::Spectrum => Self::Effects,
            Self::Effects => Self::Mixer,
            Self::Mixer => Self::Transport,
        }
    }

    /// Cycle to the previous panel in the full (all-views) tab order.
    #[must_use]
    #[allow(dead_code)]
    pub const fn prev(self) -> Self {
        match self {
            Self::Transport => Self::Mixer,
            Self::Tracks => Self::Transport,
            Self::Timeline => Self::Tracks,
            Self::Waveform => Self::Timeline,
            Self::Spectrum => Self::Waveform,
            Self::Effects => Self::Spectrum,
            Self::Mixer => Self::Effects,
        }
    }
}

/// Return the focusable panels that belong to a given view.
///
/// Tab/Shift-Tab cycle only within this set so the user never lands on a
/// panel that is invisible in the current view.
#[must_use]
pub const fn panels_for_view(view: ActiveView) -> &'static [FocusedPanel] {
    match view {
        ActiveView::Mixer => &[FocusedPanel::Mixer],
        ActiveView::Tracking => &[
            FocusedPanel::Tracks,
            FocusedPanel::Timeline,
            FocusedPanel::Waveform,
            FocusedPanel::Effects,
        ],
        ActiveView::Project | ActiveView::AudioIO => &[FocusedPanel::Transport],
    }
}

// ---------------------------------------------------------------------------
// App mode / input mode
// ---------------------------------------------------------------------------

/// Top-level application mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    /// Normal operating mode — all panels active.
    Normal,
    /// Help overlay displayed on top of the normal view.
    Help,
    /// File browser modal overlay for loading audio files.
    FileBrowser {
        /// Current directory being browsed.
        directory: std::path::PathBuf,
        /// Entries in the current directory (directories first, then audio files).
        entries: Vec<FileBrowserEntry>,
        /// Index of the selected entry.
        selected: usize,
    },
}

/// A single entry in the file browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBrowserEntry {
    /// Display name.
    pub name: String,
    /// Full path.
    pub path: std::path::PathBuf,
    /// Whether this entry is a directory.
    pub is_dir: bool,
}

// ---------------------------------------------------------------------------
// Track metadata
// ---------------------------------------------------------------------------

/// Metadata for a single synth layer within a track.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LayerInfo {
    /// Synthesis mode for this layer.
    pub mode: SynthesisMode,
    /// Human-readable label for this layer.
    pub label: String,
    /// Layer gain in dB.
    pub gain: Db,
    /// Whether this layer is enabled.
    pub enabled: bool,
    /// Parameter metadata for this layer's synth.
    pub param_infos: Vec<kazoo_core::ParamInfo>,
    /// Current parameter values (parallel to `param_infos`).
    pub param_values: Vec<f32>,
}

/// Local track metadata maintained by the TUI.
///
/// Real-time meter data (peak/RMS levels) comes from [`DisplayState`] via
/// the engine's display ring buffer. Everything else — name, mute/solo
/// state, effects — is tracked here since the display snapshot only carries
/// audio metrics.
#[derive(Debug, Clone)]
pub struct TrackInfo {
    /// Stable track identifier matching the engine's internal `TrackId`.
    pub id: TrackId,
    /// Human-readable track name.
    pub name: String,
    /// Active synthesis mode (shortcut to layer 0).
    pub synthesis_mode: SynthesisMode,
    /// Whether this track is muted.
    pub muted: bool,
    /// Whether this track is soloed.
    pub soloed: bool,
    /// Whether this track is armed for recording.
    pub armed: bool,
    /// Track volume in dB.
    pub volume: Db,
    /// Track stereo pan position.
    pub pan: Pan,
    /// Names of effects in the chain, in order.
    pub effect_names: Vec<String>,
    /// Bypass state of each effect (parallel to `effect_names`).
    pub effect_bypassed: Vec<bool>,
    /// Number of audio clips on this track.
    pub clip_count: usize,
    /// Synth parameter metadata for the primary layer (layer 0 shortcut).
    pub synth_param_infos: Vec<kazoo_core::ParamInfo>,
    /// Current synth parameter values for the primary layer (layer 0 shortcut).
    pub synth_param_values: Vec<f32>,
    /// Synth layers on this track. Always has at least one entry.
    pub layers: Vec<LayerInfo>,
    /// Currently selected layer index.
    #[allow(dead_code)]
    pub selected_layer: usize,
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// Central application state for the terminal UI.
///
/// Owns the engine handle and all UI-specific state. The main event loop
/// lives in [`App::run`].
pub struct App {
    // -- Engine interface --------------------------------------------------
    /// Handle to the audio engine (commands + display polling).
    pub engine: EngineHandle,

    /// Latest display state snapshot from the engine.
    pub display: DisplayState,

    // -- Local track metadata ----------------------------------------------
    /// Track metadata maintained locally. Index corresponds to position in
    /// the mixer's track list. Updated via helper methods that also send
    /// engine commands.
    pub tracks: Vec<TrackInfo>,

    /// Counter for generating stable track IDs. Kept in sync with the
    /// engine's mixer by starting at 0 and incrementing on each `add_track`.
    next_track_id: usize,

    // -- UI state ----------------------------------------------------------
    /// Current application mode.
    pub mode: AppMode,

    /// Which panel currently has keyboard focus.
    pub focused_panel: FocusedPanel,

    /// Input sub-mode (normal navigation vs parameter editing).
    pub input_mode: InputMode,

    /// Index of the selected track in the track list.
    pub selected_track: usize,

    /// Ratatui list selection state for the track list widget.
    pub track_list_state: ListState,

    /// Frame counter for animations (recording blink at ~2 Hz, etc.).
    pub frame_count: u64,

    /// Current master bus volume. Tracked locally since [`DisplayState`]
    /// only carries meter readings, not the volume knob position.
    pub master_volume: Db,

    /// Text buffer for numeric input in `ParameterEdit` mode.
    pub param_edit_buffer: String,

    // -- View state ------------------------------------------------------------
    /// Which view is currently displayed in the main content area.
    pub active_view: ActiveView,

    /// Per-view state for the Synth/Effects view.
    pub synth_state: SynthViewState,

    /// Per-view state for the Mixing Desk view.
    pub mixer_view_state: MixerViewState,

    /// Per-view state for the Tracking / arrangement view.
    pub tracking_state: TrackingViewState,

    /// Per-view state for the Project Setup view.
    pub project_state: ProjectViewState,

    /// Per-view state for the Audio I/O view.
    pub audio_io_state: AudioIOViewState,

    // -- Recording workflow state --------------------------------------------
    /// The configured recording workflow (count-in, fixed-length, etc.).
    pub recording_workflow: kazoo_core::transport::RecordingWorkflow,

    /// Number of count-in bars before recording starts.
    pub count_in_bars: u8,

    /// Number of bars to record (0 = unlimited / until manual stop).
    pub record_bars: u8,

    /// Set to `true` to exit the main event loop.
    pub should_quit: bool,
}

impl App {
    /// Create a new application with the given engine handle.
    #[must_use]
    pub fn new(engine: EngineHandle) -> Self {
        let display = DisplayState::initial(engine.sample_rate());
        let mut track_list_state = ListState::default();
        track_list_state.select(Some(0));

        let mut audio_io_state = AudioIOViewState::default();
        let (input_devices, output_devices) = enumerate_devices();
        audio_io_state.input_devices = input_devices;
        audio_io_state.output_devices = output_devices;

        let mut app = Self {
            engine,
            display,
            tracks: Vec::new(),
            next_track_id: 0,
            mode: AppMode::Normal,
            focused_panel: FocusedPanel::Tracks,
            input_mode: InputMode::Normal,
            selected_track: 0,
            track_list_state,
            frame_count: 0,
            master_volume: Db::UNITY,
            param_edit_buffer: String::new(),
            active_view: ActiveView::Tracking,
            synth_state: SynthViewState::default(),
            mixer_view_state: MixerViewState::default(),
            tracking_state: TrackingViewState::default(),
            project_state: ProjectViewState::default(),
            audio_io_state,
            recording_workflow: kazoo_core::transport::RecordingWorkflow::CountIn {
                count_in_bars: 1,
                record_bars: 4,
            },
            count_in_bars: 1,
            record_bars: 4,
            should_quit: false,
        };

        // Create a default armed PitchTracked track so the voice-driven
        // synthesizer works immediately on launch — no manual setup needed.
        // `add_track` auto-arms the first track.
        app.add_track("1".into(), SynthesisMode::PitchTracked);

        app
    }

    /// Create an application with no default track. Used exclusively by tests
    /// that need to control track state from scratch.
    #[cfg(test)]
    #[must_use]
    pub fn new_empty(engine: EngineHandle) -> Self {
        let display = DisplayState::initial(engine.sample_rate());
        let mut track_list_state = ListState::default();
        track_list_state.select(Some(0));

        let mut audio_io_state = AudioIOViewState::default();
        let (input_devices, output_devices) = enumerate_devices();
        audio_io_state.input_devices = input_devices;
        audio_io_state.output_devices = output_devices;

        Self {
            engine,
            display,
            tracks: Vec::new(),
            next_track_id: 0,
            mode: AppMode::Normal,
            focused_panel: FocusedPanel::Transport,
            input_mode: InputMode::Normal,
            selected_track: 0,
            track_list_state,
            frame_count: 0,
            master_volume: Db::UNITY,
            param_edit_buffer: String::new(),
            active_view: ActiveView::Tracking,
            synth_state: SynthViewState::default(),
            mixer_view_state: MixerViewState::default(),
            tracking_state: TrackingViewState::default(),
            project_state: ProjectViewState::default(),
            audio_io_state,
            recording_workflow: kazoo_core::transport::RecordingWorkflow::CountIn {
                count_in_bars: 1,
                record_bars: 4,
            },
            count_in_bars: 1,
            record_bars: 4,
            should_quit: false,
        }
    }

    // -----------------------------------------------------------------------
    // Main event loop
    // -----------------------------------------------------------------------

    /// Run the main event loop until the user quits.
    ///
    /// Drives the entire TUI lifecycle:
    /// 1. Polls keyboard events via crossterm's async [`EventStream`].
    /// 2. Ticks at [`TARGET_FPS`] to poll display state and re-render.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] if terminal rendering fails.
    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let tick_rate = Duration::from_millis(1000 / TARGET_FPS);
        let mut tick_interval = tokio::time::interval(tick_rate);
        let mut event_stream = EventStream::new();

        // Initial render.
        terminal.draw(|frame| crate::ui::draw(frame, self))?;

        while !self.should_quit {
            tokio::select! {
                maybe_event = event_stream.next() => {
                    match maybe_event {
                        Some(Ok(event)) => self.handle_event(&event),
                        Some(Err(_)) => self.should_quit = true,
                        None => {}
                    }
                }
                _ = tick_interval.tick() => {
                    self.tick();
                    terminal.draw(|frame| crate::ui::draw(frame, self))?;
                }
            }
        }

        Ok(())
    }

    /// Process one tick: poll engine display state and advance animations.
    fn tick(&mut self) {
        self.display = self.engine.poll_display().clone();
        self.frame_count = self.frame_count.wrapping_add(1);

        // Keep track selection within bounds if tracks were removed.
        if !self.tracks.is_empty() && self.selected_track >= self.tracks.len() {
            self.selected_track = self.tracks.len().saturating_sub(1);
            self.track_list_state.select(Some(self.selected_track));
            self.mixer_view_state.selected_channel = self.selected_track;
        }

        // Sync clip counts from the timeline snapshot.
        for track_snap in &self.display.timeline.tracks {
            if let Some(track_info) = self
                .tracks
                .iter_mut()
                .find(|t| t.id.0 == track_snap.track_id)
            {
                track_info.clip_count = track_snap.clips.len();
            }
        }
    }

    /// Dispatch a crossterm event to the input handler.
    fn handle_event(&mut self, event: &Event) {
        if let Event::Key(key) = *event {
            crate::input::handle_key_event(self, key);
        }
    }

    // -----------------------------------------------------------------------
    // Track management
    //
    // These methods update local metadata AND send the corresponding engine
    // command, keeping the TUI's view in sync with the engine.
    // -----------------------------------------------------------------------

    /// Add a new track with the given name and synthesis mode.
    pub fn add_track(&mut self, name: String, synthesis_mode: SynthesisMode) {
        let id = TrackId(self.next_track_id);
        self.next_track_id += 1;

        let sample_rate = self.engine.sample_rate() as f32;
        let param_infos = synthesis_mode.param_infos(sample_rate);
        let param_values = synthesis_mode.default_param_values(sample_rate);
        let layer0 = LayerInfo {
            mode: synthesis_mode,
            label: synthesis_mode.display_name().into(),
            gain: Db::UNITY,
            enabled: true,
            param_infos: param_infos.clone(),
            param_values: param_values.clone(),
        };
        // Auto-arm the first track so voice-driven synthesis works
        // immediately without manual setup.
        let auto_arm = self.tracks.is_empty();
        let info = TrackInfo {
            id,
            name: name.clone(),
            synthesis_mode,
            muted: false,
            soloed: false,
            armed: auto_arm,
            volume: Db::UNITY,
            pan: Pan::CENTER,
            effect_names: Vec::new(),
            effect_bypassed: Vec::new(),
            clip_count: 0,
            synth_param_infos: param_infos,
            synth_param_values: param_values,
            layers: vec![layer0],
            selected_layer: 0,
        };
        self.tracks.push(info);

        // Select the new track if it's the first one.
        if self.tracks.len() == 1 {
            self.selected_track = 0;
            self.track_list_state.select(Some(0));
        }

        let _ = self.engine.add_track(name, synthesis_mode);

        // Send the arm command to the engine so it matches TUI state.
        if auto_arm {
            let _ = self
                .engine
                .send_command(EngineCommand::SetTrackArm(id, true));
        }
    }

    /// Remove the track at the given list index.
    pub fn remove_track(&mut self, index: usize) {
        if index >= self.tracks.len() {
            return;
        }

        let id = self.tracks[index].id;
        self.tracks.remove(index);
        let _ = self.engine.send_command(EngineCommand::RemoveTrack(id));

        // Adjust selection (keep all selection state in sync).
        if self.tracks.is_empty() {
            self.selected_track = 0;
            self.mixer_view_state.selected_channel = 0;
            self.track_list_state.select(None);
        } else if self.selected_track >= self.tracks.len() {
            self.selected_track = self.tracks.len().saturating_sub(1);
            self.mixer_view_state.selected_channel = self.selected_track;
            self.track_list_state.select(Some(self.selected_track));
        }
    }

    /// Toggle mute on the track at the given index.
    pub fn toggle_mute(&mut self, index: usize) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.muted = !track.muted;
            let _ = self.engine.set_track_mute(track.id, track.muted);
        }
    }

    /// Toggle solo on the track at the given index.
    pub fn toggle_solo(&mut self, index: usize) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.soloed = !track.soloed;
            let _ = self.engine.set_track_solo(track.id, track.soloed);
        }
    }

    /// Toggle arm (record enable) on the track at the given index.
    pub fn toggle_arm(&mut self, index: usize) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.armed = !track.armed;
            let _ = self
                .engine
                .send_command(EngineCommand::SetTrackArm(track.id, track.armed));
        }
    }

    /// Cycle the synthesis mode on the track at the given index.
    ///
    /// Updates both the primary synth shortcut fields and layer 0.
    pub fn cycle_synth_mode(&mut self, index: usize) {
        if let Some(track) = self.tracks.get_mut(index) {
            let next = match track.synthesis_mode {
                SynthesisMode::Passthrough => SynthesisMode::PitchTracked,
                SynthesisMode::PitchTracked => SynthesisMode::Wavetable,
                SynthesisMode::Wavetable => SynthesisMode::Granular,
                SynthesisMode::Granular => SynthesisMode::Vocoder,
                SynthesisMode::Vocoder => SynthesisMode::PhaseVocoder,
                SynthesisMode::PhaseVocoder => SynthesisMode::Passthrough,
            };
            track.synthesis_mode = next;
            let sample_rate = self.engine.sample_rate() as f32;
            let infos = next.param_infos(sample_rate);
            let values = next.default_param_values(sample_rate);
            track.synth_param_infos.clone_from(&infos);
            track.synth_param_values.clone_from(&values);
            // Update layer 0 to match.
            if let Some(layer) = track.layers.first_mut() {
                layer.mode = next;
                layer.label = next.display_name().into();
                layer.param_infos = infos;
                layer.param_values = values;
            }
            let _ = self
                .engine
                .send_command(EngineCommand::SetTrackSynthesisMode(track.id, next));
        }
        self.synth_state.selected_synth_param = 0;
    }

    /// Set the volume for the track at the given index.
    pub fn set_track_volume(&mut self, index: usize, db: Db) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.volume = db;
            let _ = self.engine.set_track_volume(track.id, db);
        }
    }

    /// Set the pan for the track at the given index.
    pub fn set_track_pan(&mut self, index: usize, pan: Pan) {
        if let Some(track) = self.tracks.get_mut(index) {
            track.pan = pan;
            let _ = self.engine.set_track_pan(track.id, pan);
        }
    }

    /// Add an effect to the selected track's chain.
    pub fn add_effect_to_track(
        &mut self,
        track_index: usize,
        name: String,
        effect: Box<dyn kazoo_core::Processor>,
    ) {
        if let Some(track) = self.tracks.get_mut(track_index) {
            track.effect_names.push(name);
            track.effect_bypassed.push(false);
            let _ = self.engine.add_effect(track.id, effect);
        }
    }

    /// Toggle bypass on an effect in the selected track's chain.
    pub fn toggle_effect_bypass(&mut self, track_index: usize, effect_index: usize) {
        if let Some(track) = self.tracks.get_mut(track_index) {
            if let Some(bypassed) = track.effect_bypassed.get_mut(effect_index) {
                *bypassed = !*bypassed;
                let _ = self.engine.send_command(EngineCommand::SetEffectBypass {
                    track_id: track.id,
                    effect_index,
                    bypassed: *bypassed,
                });
            }
        }
    }

    /// Remove an effect from a track's chain by index.
    pub fn remove_effect(&mut self, track_index: usize, effect_index: usize) {
        if let Some(track) = self.tracks.get_mut(track_index) {
            if effect_index < track.effect_names.len() {
                track.effect_names.remove(effect_index);
                track.effect_bypassed.remove(effect_index);
                let _ = self.engine.send_command(EngineCommand::RemoveEffect {
                    track_id: track.id,
                    effect_index,
                });

                // Adjust selection indices if the removed effect was on the
                // currently selected track.
                if track_index == self.selected_track {
                    if track.effect_names.is_empty() {
                        self.synth_state.selected_effect = 0;
                    } else if self.synth_state.selected_effect >= track.effect_names.len() {
                        self.synth_state.selected_effect = track.effect_names.len() - 1;
                    }
                    self.synth_state.selected_param = 0;
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Layer management
    // -----------------------------------------------------------------------

    /// Add a synth layer to the selected track.
    ///
    /// Returns `true` if the layer was added, `false` if the track doesn't
    /// exist or the maximum number of layers has been reached.
    #[allow(dead_code)]
    pub fn add_synth_layer(&mut self, mode: SynthesisMode) -> bool {
        let Some(track) = self.tracks.get_mut(self.selected_track) else {
            return false;
        };
        if track.layers.len() >= kazoo_core::MAX_SYNTH_LAYERS {
            return false;
        }

        let sample_rate = self.engine.sample_rate() as f32;
        let label: String = mode.display_name().into();
        let layer = LayerInfo {
            mode,
            label: label.clone(),
            gain: Db::UNITY,
            enabled: true,
            param_infos: mode.param_infos(sample_rate),
            param_values: mode.default_param_values(sample_rate),
        };
        track.layers.push(layer);

        let _ = self.engine.send_command(EngineCommand::AddSynthLayer {
            track_id: track.id,
            synthesis_mode: mode,
            label,
        });
        true
    }

    /// Remove a synth layer from the selected track by index.
    ///
    /// Layer 0 cannot be removed. Returns `true` if the layer was removed.
    #[allow(dead_code)]
    pub fn remove_synth_layer(&mut self, layer_index: usize) -> bool {
        let Some(track) = self.tracks.get_mut(self.selected_track) else {
            return false;
        };
        if layer_index == 0 || layer_index >= track.layers.len() {
            return false;
        }

        let id = track.id;
        track.layers.remove(layer_index);

        // Adjust selected layer.
        if track.selected_layer >= track.layers.len() {
            track.selected_layer = track.layers.len().saturating_sub(1);
        }

        let _ = self.engine.send_command(EngineCommand::RemoveSynthLayer {
            track_id: id,
            layer_index,
        });
        true
    }

    /// Toggle the enabled state of a layer on the selected track.
    #[allow(dead_code)]
    pub fn toggle_layer_enabled(&mut self, layer_index: usize) {
        let Some(track) = self.tracks.get_mut(self.selected_track) else {
            return;
        };
        let Some(layer) = track.layers.get_mut(layer_index) else {
            return;
        };

        layer.enabled = !layer.enabled;
        let _ = self
            .engine
            .send_command(EngineCommand::SetSynthLayerEnabled {
                track_id: track.id,
                layer_index,
                enabled: layer.enabled,
            });
    }

    /// Set the gain of a layer on the selected track.
    #[allow(dead_code)]
    pub fn set_layer_gain(&mut self, layer_index: usize, gain: Db) {
        let Some(track) = self.tracks.get_mut(self.selected_track) else {
            return;
        };
        let Some(layer) = track.layers.get_mut(layer_index) else {
            return;
        };

        layer.gain = gain;
        let _ = self.engine.send_command(EngineCommand::SetSynthLayerGain {
            track_id: track.id,
            layer_index,
            gain,
        });
    }

    // -----------------------------------------------------------------------
    // UI helpers
    // -----------------------------------------------------------------------

    /// Get the `TrackId` for the currently selected track, if any.
    #[must_use]
    pub fn selected_track_id(&self) -> Option<TrackId> {
        self.tracks.get(self.selected_track).map(|t| t.id)
    }

    /// Get the selected track info, if any.
    #[must_use]
    pub fn selected_track_info(&self) -> Option<&TrackInfo> {
        self.tracks.get(self.selected_track)
    }

    /// Whether the recording blink animation should show the indicator.
    ///
    /// Blinks at approximately 2 Hz (toggles every 30 frames at 60 fps).
    #[must_use]
    pub const fn recording_blink_visible(&self) -> bool {
        (self.frame_count / 30) % 2 == 0
    }

    /// The number of tracks.
    #[must_use]
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Whether any track has clips (used to decide timeline vs waveform).
    #[must_use]
    pub fn has_clips(&self) -> bool {
        !self.display.timeline.tracks.is_empty()
            && self
                .display
                .timeline
                .tracks
                .iter()
                .any(|t| !t.clips.is_empty() || t.is_recording_clip)
    }

    /// Open the file browser starting in the current working directory.
    pub fn open_file_browser(&mut self) {
        let dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
        let entries = Self::scan_directory(&dir);
        self.mode = AppMode::FileBrowser {
            directory: dir,
            entries,
            selected: 0,
        };
    }

    /// Scan a directory for subdirectories and audio files.
    ///
    /// Returns entries sorted: directories first (alphabetical), then audio
    /// files (alphabetical). Non-readable entries are silently skipped.
    #[must_use]
    pub fn scan_directory(dir: &std::path::Path) -> Vec<FileBrowserEntry> {
        let mut dirs = Vec::new();
        let mut files = Vec::new();

        let Ok(read_dir) = std::fs::read_dir(dir) else {
            return Vec::new();
        };

        for entry in read_dir.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();

            // Skip hidden entries.
            if name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                dirs.push(FileBrowserEntry {
                    name,
                    path,
                    is_dir: true,
                });
            } else if is_audio_file(&name) {
                files.push(FileBrowserEntry {
                    name,
                    path,
                    is_dir: false,
                });
            }
        }

        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        dirs.extend(files);
        dirs
    }

    /// Check whether a specific panel has focus.
    #[must_use]
    pub const fn is_focused(&self, panel: FocusedPanel) -> bool {
        matches!(
            (&self.focused_panel, &panel),
            (FocusedPanel::Transport, FocusedPanel::Transport)
                | (FocusedPanel::Tracks, FocusedPanel::Tracks)
                | (FocusedPanel::Timeline, FocusedPanel::Timeline)
                | (FocusedPanel::Waveform, FocusedPanel::Waveform)
                | (FocusedPanel::Spectrum, FocusedPanel::Spectrum)
                | (FocusedPanel::Effects, FocusedPanel::Effects)
                | (FocusedPanel::Mixer, FocusedPanel::Mixer)
        )
    }
}

/// Enumerate available audio input and output devices.
///
/// Returns `(input_device_names, output_device_names)`. On error, returns
/// empty vectors so the UI gracefully falls back to "no devices found".
fn enumerate_devices() -> (Vec<String>, Vec<String>) {
    let inputs = kazoo_core::io::enumerate_input_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| d.name)
        .collect();
    let outputs = kazoo_core::io::enumerate_output_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| d.name)
        .collect();
    (inputs, outputs)
}

/// Check whether a filename has a recognised audio extension.
fn is_audio_file(name: &str) -> bool {
    std::path::Path::new(name).extension().is_some_and(|ext| {
        ext.eq_ignore_ascii_case("wav")
            || ext.eq_ignore_ascii_case("mp3")
            || ext.eq_ignore_ascii_case("flac")
            || ext.eq_ignore_ascii_case("ogg")
            || ext.eq_ignore_ascii_case("aiff")
            || ext.eq_ignore_ascii_case("aif")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_panel_next_cycles() {
        let start = FocusedPanel::Transport;
        let mut panel = start;
        let panels = [
            FocusedPanel::Tracks,
            FocusedPanel::Timeline,
            FocusedPanel::Waveform,
            FocusedPanel::Spectrum,
            FocusedPanel::Effects,
            FocusedPanel::Mixer,
            FocusedPanel::Transport,
        ];
        for expected in panels {
            panel = panel.next();
            assert_eq!(panel, expected);
        }
    }

    #[test]
    fn focused_panel_prev_cycles() {
        let start = FocusedPanel::Transport;
        let mut panel = start;
        let panels = [
            FocusedPanel::Mixer,
            FocusedPanel::Effects,
            FocusedPanel::Spectrum,
            FocusedPanel::Waveform,
            FocusedPanel::Timeline,
            FocusedPanel::Tracks,
            FocusedPanel::Transport,
        ];
        for expected in panels {
            panel = panel.prev();
            assert_eq!(panel, expected);
        }
    }

    #[test]
    fn focused_panel_next_prev_inverse() {
        for panel in [
            FocusedPanel::Transport,
            FocusedPanel::Tracks,
            FocusedPanel::Timeline,
            FocusedPanel::Waveform,
            FocusedPanel::Spectrum,
            FocusedPanel::Effects,
            FocusedPanel::Mixer,
        ] {
            assert_eq!(panel.next().prev(), panel);
            assert_eq!(panel.prev().next(), panel);
        }
    }

    #[test]
    fn recording_blink_visible_alternates() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        // Frames 0-29: visible (frame_count/30 == 0, 0%2 == 0)
        app.frame_count = 0;
        assert!(app.recording_blink_visible());

        app.frame_count = 29;
        assert!(app.recording_blink_visible());

        // Frames 30-59: hidden (frame_count/30 == 1, 1%2 == 1)
        app.frame_count = 30;
        assert!(!app.recording_blink_visible());

        app.frame_count = 59;
        assert!(!app.recording_blink_visible());

        // Frames 60-89: visible again
        app.frame_count = 60;
        assert!(app.recording_blink_visible());
    }

    #[test]
    fn is_focused_checks_correctly() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.focused_panel = FocusedPanel::Tracks;
        assert!(app.is_focused(FocusedPanel::Tracks));
        assert!(!app.is_focused(FocusedPanel::Transport));
        assert!(!app.is_focused(FocusedPanel::Mixer));
    }

    #[test]
    fn new_creates_default_armed_track() {
        let app = App::new(test_engine_handle());
        assert_eq!(app.tracks.len(), 1);
        assert_eq!(app.tracks[0].name, "1");
        assert!(app.tracks[0].armed, "default track must be armed");
        assert_eq!(app.tracks[0].synthesis_mode, SynthesisMode::PitchTracked);
    }

    #[test]
    fn first_track_auto_arms() {
        let mut app = App::new_empty(test_engine_handle());
        app.add_track("A".into(), SynthesisMode::PitchTracked);
        assert!(app.tracks[0].armed, "first track should auto-arm");

        // Second track should NOT auto-arm.
        app.add_track("B".into(), SynthesisMode::Granular);
        assert!(!app.tracks[1].armed, "second track should not auto-arm");
    }

    #[test]
    fn add_track_increments_id() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.add_track("Lead".into(), SynthesisMode::PitchTracked);
        app.add_track("Bass".into(), SynthesisMode::Granular);

        assert_eq!(app.tracks.len(), 2);
        assert_eq!(app.tracks[0].id, TrackId(0));
        assert_eq!(app.tracks[0].name, "Lead");
        assert_eq!(app.tracks[1].id, TrackId(1));
        assert_eq!(app.tracks[1].name, "Bass");
    }

    #[test]
    fn remove_track_adjusts_selection() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.add_track("A".into(), SynthesisMode::PitchTracked);
        app.add_track("B".into(), SynthesisMode::Granular);
        app.add_track("C".into(), SynthesisMode::Vocoder);
        app.selected_track = 2;

        // Remove last track: selection moves to new last.
        app.remove_track(2);
        assert_eq!(app.selected_track, 1);
        assert_eq!(app.tracks.len(), 2);
    }

    #[test]
    fn remove_all_tracks_clears_selection() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.add_track("Solo".into(), SynthesisMode::Wavetable);
        app.remove_track(0);

        assert!(app.tracks.is_empty());
        assert_eq!(app.selected_track, 0);
        assert_eq!(app.track_list_state.selected(), None);
    }

    #[test]
    fn toggle_mute_flips_state() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        assert!(!app.tracks[0].muted);

        app.toggle_mute(0);
        assert!(app.tracks[0].muted);

        app.toggle_mute(0);
        assert!(!app.tracks[0].muted);
    }

    #[test]
    fn toggle_solo_flips_state() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.toggle_solo(0);
        assert!(app.tracks[0].soloed);
    }

    #[test]
    fn toggle_arm_flips_state() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        // First track is auto-armed; toggling should disarm it.
        assert!(app.tracks[0].armed);
        app.toggle_arm(0);
        assert!(!app.tracks[0].armed);
        // Toggle again to re-arm.
        app.toggle_arm(0);
        assert!(app.tracks[0].armed);
    }

    #[test]
    fn selected_track_id_returns_none_when_empty() {
        let engine_handle = test_engine_handle();
        let app = App::new_empty(engine_handle);
        assert!(app.selected_track_id().is_none());
    }

    #[test]
    fn selected_track_id_returns_correct_id() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.selected_track = 0;
        assert_eq!(app.selected_track_id(), Some(TrackId(0)));
    }

    #[test]
    fn toggle_effect_bypass_out_of_bounds_is_noop() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);
        app.add_track("T".into(), SynthesisMode::PitchTracked);

        // No effects added — should not panic.
        app.toggle_effect_bypass(0, 0);
        assert!(app.tracks[0].effect_bypassed.is_empty());
    }

    #[test]
    fn set_track_volume_updates_local_state() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);
        app.add_track("T".into(), SynthesisMode::PitchTracked);

        app.set_track_volume(0, Db::new(-6.0));
        assert!((app.tracks[0].volume.value() - (-6.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn set_track_pan_updates_local_state() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);
        app.add_track("T".into(), SynthesisMode::PitchTracked);

        app.set_track_pan(0, Pan::new(0.5));
        assert!((app.tracks[0].pan.value() - 0.5).abs() < f32::EPSILON);
    }

    // -- M8: remove_effect adjusts selected_effect --------------------------

    #[test]
    fn remove_effect_clamps_selected_effect() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        // Manually add effect metadata (we can't add real Processor objects
        // in unit tests, but we can simulate the metadata).
        app.tracks[0].effect_names = vec!["FX1".into(), "FX2".into(), "FX3".into()];
        app.tracks[0].effect_bypassed = vec![false, false, false];
        app.selected_track = 0;
        app.synth_state.selected_effect = 2; // pointing at FX3
        app.synth_state.selected_param = 3;

        app.remove_effect(0, 2); // remove FX3

        // selected_effect should clamp to the new last index (1).
        assert_eq!(app.synth_state.selected_effect, 1);
        // selected_param should reset to 0.
        assert_eq!(app.synth_state.selected_param, 0);
        assert_eq!(app.tracks[0].effect_names.len(), 2);
    }

    #[test]
    fn remove_all_effects_resets_selected_effect() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.tracks[0].effect_names = vec!["FX1".into()];
        app.tracks[0].effect_bypassed = vec![false];
        app.selected_track = 0;
        app.synth_state.selected_effect = 0;

        app.remove_effect(0, 0);

        assert_eq!(app.synth_state.selected_effect, 0);
        assert_eq!(app.synth_state.selected_param, 0);
        assert!(app.tracks[0].effect_names.is_empty());
    }

    #[test]
    fn remove_effect_on_other_track_does_not_change_selection() {
        let engine_handle = test_engine_handle();
        let mut app = App::new_empty(engine_handle);

        app.add_track("T1".into(), SynthesisMode::PitchTracked);
        app.add_track("T2".into(), SynthesisMode::Granular);
        app.tracks[0].effect_names = vec!["FX1".into(), "FX2".into()];
        app.tracks[0].effect_bypassed = vec![false, false];
        app.tracks[1].effect_names = vec!["FX3".into()];
        app.tracks[1].effect_bypassed = vec![false];
        app.selected_track = 0;
        app.synth_state.selected_effect = 1;
        app.synth_state.selected_param = 2;

        // Remove from track 1 (not the selected track).
        app.remove_effect(1, 0);

        // Selection on the selected track should be untouched.
        assert_eq!(app.synth_state.selected_effect, 1);
        assert_eq!(app.synth_state.selected_param, 2);
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    // -- Timeline state -------------------------------------------------------

    #[test]
    fn initial_timeline_state() {
        let app = App::new_empty(test_engine_handle());
        assert!((app.tracking_state.timeline_zoom - 256.0).abs() < f64::EPSILON);
        assert!((app.tracking_state.timeline_scroll - 0.0).abs() < f64::EPSILON);
        assert!(app.tracking_state.selected_clip.is_none());
    }

    #[test]
    fn has_clips_returns_false_with_no_clips() {
        let app = App::new_empty(test_engine_handle());
        assert!(!app.has_clips());
    }

    #[test]
    fn timeline_panel_in_focus_cycle() {
        let mut app = App::new_empty(test_engine_handle());
        app.focused_panel = FocusedPanel::Tracks;
        assert_eq!(app.focused_panel.next(), FocusedPanel::Timeline);
        assert_eq!(FocusedPanel::Timeline.next(), FocusedPanel::Waveform);
        assert_eq!(FocusedPanel::Waveform.prev(), FocusedPanel::Timeline);
    }

    #[test]
    fn is_audio_file_recognises_extensions() {
        assert!(super::is_audio_file("song.wav"));
        assert!(super::is_audio_file("song.WAV"));
        assert!(super::is_audio_file("beat.mp3"));
        assert!(super::is_audio_file("track.flac"));
        assert!(super::is_audio_file("sound.ogg"));
        assert!(super::is_audio_file("clip.aiff"));
        assert!(super::is_audio_file("clip.aif"));
        assert!(!super::is_audio_file("readme.txt"));
        assert!(!super::is_audio_file("image.png"));
        assert!(!super::is_audio_file("code.rs"));
    }

    #[test]
    fn add_track_has_zero_clip_count() {
        let mut app = App::new_empty(test_engine_handle());
        app.add_track("T".into(), SynthesisMode::PitchTracked);
        assert_eq!(app.tracks[0].clip_count, 0);
    }

    #[test]
    fn file_browser_entry_debug() {
        let entry = FileBrowserEntry {
            name: "test.wav".into(),
            path: std::path::PathBuf::from("/tmp/test.wav"),
            is_dir: false,
        };
        let dbg = format!("{entry:?}");
        assert!(dbg.contains("test.wav"));
    }

    // -- Layer management tests -----------------------------------------------

    #[test]
    fn add_track_creates_single_layer() {
        let mut app = App::new_empty(test_engine_handle());
        app.add_track("T".into(), SynthesisMode::PitchTracked);

        assert_eq!(app.tracks[0].layers.len(), 1);
        assert_eq!(app.tracks[0].layers[0].mode, SynthesisMode::PitchTracked);
        assert!(app.tracks[0].layers[0].enabled);
        assert_eq!(app.tracks[0].selected_layer, 0);
    }

    #[test]
    fn add_synth_layer_adds_to_selected_track() {
        let mut app = App::new_empty(test_engine_handle());
        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.selected_track = 0;

        assert!(app.add_synth_layer(SynthesisMode::Wavetable));
        assert_eq!(app.tracks[0].layers.len(), 2);
        assert_eq!(app.tracks[0].layers[1].mode, SynthesisMode::Wavetable);
    }

    #[test]
    fn add_synth_layer_respects_max() {
        let mut app = App::new_empty(test_engine_handle());
        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.selected_track = 0;

        for _ in 1..kazoo_core::MAX_SYNTH_LAYERS {
            assert!(app.add_synth_layer(SynthesisMode::Wavetable));
        }
        assert!(!app.add_synth_layer(SynthesisMode::Granular));
        assert_eq!(app.tracks[0].layers.len(), kazoo_core::MAX_SYNTH_LAYERS);
    }

    #[test]
    fn remove_synth_layer_cannot_remove_zero() {
        let mut app = App::new_empty(test_engine_handle());
        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.selected_track = 0;

        assert!(!app.remove_synth_layer(0));
        assert_eq!(app.tracks[0].layers.len(), 1);
    }

    #[test]
    fn remove_synth_layer_adjusts_selection() {
        let mut app = App::new_empty(test_engine_handle());
        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.selected_track = 0;
        app.add_synth_layer(SynthesisMode::Wavetable);
        app.add_synth_layer(SynthesisMode::Granular);
        app.tracks[0].selected_layer = 2;

        assert!(app.remove_synth_layer(2));
        assert_eq!(app.tracks[0].layers.len(), 2);
        assert_eq!(app.tracks[0].selected_layer, 1);
    }

    #[test]
    fn toggle_layer_enabled_flips() {
        let mut app = App::new_empty(test_engine_handle());
        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.selected_track = 0;
        assert!(app.tracks[0].layers[0].enabled);

        app.toggle_layer_enabled(0);
        assert!(!app.tracks[0].layers[0].enabled);

        app.toggle_layer_enabled(0);
        assert!(app.tracks[0].layers[0].enabled);
    }

    #[test]
    fn cycle_synth_mode_resets_param_and_layer() {
        let mut app = App::new_empty(test_engine_handle());
        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.synth_state.selected_synth_param = 3;

        app.cycle_synth_mode(0);

        assert_eq!(app.synth_state.selected_synth_param, 0);
        assert_eq!(app.tracks[0].synthesis_mode, SynthesisMode::Wavetable);
        assert_eq!(app.tracks[0].layers[0].mode, SynthesisMode::Wavetable);
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Create an `EngineHandle` backed by real channels but no audio threads.
    fn test_engine_handle() -> EngineHandle {
        use crossbeam_channel::unbounded;
        use ringbuf::HeapRb;
        use ringbuf::traits::Split;

        let (cmd_tx, _cmd_rx) = unbounded();
        let rb = HeapRb::<DisplayState>::new(4);
        let (_prod, cons) = rb.split();
        EngineHandle::new(cmd_tx, cons, 44_100, 256)
    }
}
