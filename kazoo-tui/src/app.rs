//! Application state, event loop, and TUI coordination.
//!
//! The [`App`] struct is the central state container for the terminal UI.
//! It owns the [`EngineHandle`], maintains local track metadata, and drives
//! the main event loop that bridges keyboard input, engine display updates,
//! and frame rendering.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::widgets::ListState;

use kazoo_core::engine::{DeskLink, DisplayState, EngineCommand, EngineHandle};
use kazoo_core::ipc::link::LinkStatus;
use kazoo_core::mixer::TrackId;
use kazoo_core::synthesis::SynthesisMode;
use kazoo_core::{Db, Pan};

// Re-export state types so existing `use crate::app::*` imports keep working.
pub use crate::state::{
    ActiveView, AudioIOViewState, InputMode, MixerViewState, ProjectViewState, SynthViewState,
    TrackingViewState,
};
use crate::status::{EngineHealth, StatusLine};

/// Target frames per second for UI rendering.
const TARGET_FPS: u64 = 60;

// ---------------------------------------------------------------------------
// Panel focus
// ---------------------------------------------------------------------------

/// Panels that can receive keyboard focus.
///
/// `Tab` cycles forward, `BackTab` (Shift+Tab) cycles backward, within the
/// set returned by [`panels_for_view`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusedPanel {
    Transport,
    Tracks,
    Timeline,
    Waveform,
    Effects,
    Mixer,
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
        directory: PathBuf,
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
    pub path: PathBuf,
    /// Whether this entry is a directory.
    pub is_dir: bool,
}

/// The result of scanning one directory for the file browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryListing {
    /// Visible entries: directories first (alphabetical), then audio files
    /// (alphabetical).
    pub entries: Vec<FileBrowserEntry>,
    /// Number of entries whose metadata could not be read (permission
    /// denied, broken symlink, entry vanished mid-scan). These are not
    /// listed, and the count is reported to the user.
    pub unreadable: usize,
}

// ---------------------------------------------------------------------------
// Audio devices
// ---------------------------------------------------------------------------

/// Audio devices discovered at startup.
///
/// Enumeration talks to the OS audio subsystem, so it is performed by the
/// caller (see [`AudioDevices::enumerate`]) and injected into [`App::new`].
/// Constructing an [`App`] therefore never touches audio hardware.
#[derive(Debug)]
pub struct AudioDevices {
    /// Input (capture) device names, or the enumeration error.
    pub inputs: kazoo_core::Result<Vec<String>>,
    /// Output (playback) device names, or the enumeration error.
    pub outputs: kazoo_core::Result<Vec<String>>,
}

impl AudioDevices {
    /// Query the OS for the available input and output devices.
    #[must_use]
    pub fn enumerate() -> Self {
        Self {
            inputs: kazoo_core::io::enumerate_input_devices()
                .map(|devices| devices.into_iter().map(|d| d.name).collect()),
            outputs: kazoo_core::io::enumerate_output_devices()
                .map(|devices| devices.into_iter().map(|d| d.name).collect()),
        }
    }

    /// An empty device set (no devices, no errors).
    #[cfg(test)]
    #[must_use]
    pub const fn none() -> Self {
        Self {
            inputs: Ok(Vec::new()),
            outputs: Ok(Vec::new()),
        }
    }
}

// ---------------------------------------------------------------------------
// Track metadata
// ---------------------------------------------------------------------------

/// Local metadata for one effect in a track's chain.
#[derive(Debug, Clone)]
pub struct EffectInfo {
    /// Display name.
    pub name: String,
    /// Whether the effect is bypassed.
    pub bypassed: bool,
    /// Parameter metadata, captured from the processor when it was added.
    pub param_infos: Vec<kazoo_core::ParamInfo>,
    /// Current parameter values (parallel to `param_infos`).
    pub param_values: Vec<f32>,
}

impl EffectInfo {
    /// Capture an effect's name and parameter metadata before the processor
    /// is handed to the engine.
    ///
    /// A parameter whose current value the processor does not report is
    /// shown at its declared default, which is what a freshly constructed
    /// processor holds.
    #[must_use]
    pub fn from_processor(name: String, processor: &dyn kazoo_core::Processor) -> Self {
        let param_infos: Vec<kazoo_core::ParamInfo> = (0..processor.param_count())
            .map_while(|index| processor.param_info(index))
            .collect();
        let param_values = param_infos
            .iter()
            .enumerate()
            .map(|(index, info)| {
                processor
                    .param_value(index)
                    .map_or(info.default, |value| info.clamp(value))
            })
            .collect();
        Self {
            name,
            bypassed: false,
            param_infos,
            param_values,
        }
    }
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
    /// Active synthesis mode of the track's primary synth.
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
    /// Effects in the chain, in order.
    pub effects: Vec<EffectInfo>,
    /// Number of audio clips on this track.
    pub clip_count: usize,
    /// Parameter metadata for the primary synth.
    pub synth_param_infos: Vec<kazoo_core::ParamInfo>,
    /// Current parameter values for the primary synth (parallel to
    /// `synth_param_infos`).
    pub synth_param_values: Vec<f32>,
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

/// What the header shows about this synth's plug into the kazoo-mix desk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeskView {
    /// The engine was not asked to plug into the desk.
    Off,
    /// The link is running: connected to a strip, or looking for the desk
    /// (with the reason it was last refused, if it was).
    Link(LinkStatus),
    /// The link could not start, so the synth plays standalone.
    Failed(String),
}

impl DeskView {
    /// The current state of the engine's desk link.
    #[must_use]
    pub fn of(desk: &DeskLink) -> Self {
        match desk {
            DeskLink::Disabled => Self::Off,
            DeskLink::Running(link) => Self::Link(link.status()),
            DeskLink::Failed(reason) => Self::Failed(reason.clone()),
        }
    }
}

/// Central application state for the terminal UI.
///
/// Owns the engine handle and all UI-specific state. The main event loop
/// lives in [`App::run`].
#[derive(Debug)]
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

    /// Status line: errors and confirmations shown in the header.
    pub status: StatusLine,

    /// The engine's failure counters, shown in the header while non-zero.
    pub engine_health: EngineHealth,

    /// The desk link, refreshed every frame for the header.
    pub desk: DeskView,

    // -- View state ------------------------------------------------------------
    /// Which view is currently displayed in the main content area.
    pub active_view: ActiveView,

    /// Selection state for the synth + effects sidebar.
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
    /// Create a new application with the given engine handle and the audio
    /// devices discovered at startup.
    ///
    /// A default armed `PitchTracked` track is created so the voice-driven
    /// synthesizer works immediately on launch — no manual setup needed.
    #[must_use]
    pub fn new(engine: EngineHandle, devices: AudioDevices) -> Self {
        let mut app = Self::new_empty(engine, devices);
        app.focused_panel = FocusedPanel::Tracks;
        // `add_track` auto-arms the first track.
        app.add_track("1".into(), SynthesisMode::PitchTracked);
        app
    }

    /// Create an application with no tracks.
    ///
    /// Device enumeration failures are shown in the Audio I/O view and
    /// posted to the status line.
    #[must_use]
    pub fn new_empty(engine: EngineHandle, devices: AudioDevices) -> Self {
        let display = DisplayState::initial(engine.sample_rate());
        let mut track_list_state = ListState::default();
        track_list_state.select(Some(0));

        let mut status = StatusLine::default();
        let mut audio_io_state = AudioIOViewState::default();
        match devices.inputs {
            Ok(names) => audio_io_state.input_devices = names,
            Err(err) => {
                let message = format!("Input device scan failed: {err}");
                status.error(message.clone());
                audio_io_state.input_device_error = Some(message);
            }
        }
        match devices.outputs {
            Ok(names) => audio_io_state.output_devices = names,
            Err(err) => {
                let message = format!("Output device scan failed: {err}");
                status.error(message.clone());
                audio_io_state.output_device_error = Some(message);
            }
        }

        let desk = DeskView::of(engine.desk_link());
        Self {
            engine,
            display,
            tracks: Vec::new(),
            mode: AppMode::Normal,
            focused_panel: FocusedPanel::Transport,
            input_mode: InputMode::Normal,
            selected_track: 0,
            track_list_state,
            frame_count: 0,
            master_volume: Db::UNITY,
            param_edit_buffer: String::new(),
            status,
            engine_health: EngineHealth::default(),
            desk,
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
    /// Returns [`io::Error`] if terminal rendering fails, if reading a
    /// terminal event fails, or if the terminal event stream closes (no
    /// further input could ever arrive, so the user could not quit).
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
                        Some(Err(err)) => return Err(err),
                        None => {
                            return Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "terminal event stream closed",
                            ));
                        }
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
        self.desk = DeskView::of(self.engine.desk_link());
        if let Some(message) = self
            .engine_health
            .observe(self.engine.stats(), Instant::now())
        {
            self.status.error(message);
        }

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
    // These methods send the corresponding engine command and update local
    // metadata only when the engine accepted it, keeping the TUI's view in
    // sync with the engine. Failures are posted to the status line.
    // -----------------------------------------------------------------------

    /// Add a new track with the given name and synthesis mode.
    ///
    /// Returns `true` if the engine accepted the track.
    pub fn add_track(&mut self, name: String, synthesis_mode: SynthesisMode) -> bool {
        // The engine handle assigns the id, so the TUI and the engine
        // always agree on it.
        let id = match self.engine.add_track(name.clone(), synthesis_mode) {
            Ok(id) => id,
            Err(err) => {
                // The engine never saw the track: do not show a track that
                // does not exist.
                self.status.report("Add track", Err(err));
                return false;
            }
        };

        let sample_rate = self.engine.sample_rate() as f32;
        // Auto-arm the first track so voice-driven synthesis works
        // immediately without manual setup.
        let auto_arm = self.tracks.is_empty();
        let armed = auto_arm
            && self.status.report(
                "Arm track",
                self.engine
                    .send_command(EngineCommand::SetTrackArm(id, true)),
            );

        self.tracks.push(TrackInfo {
            id,
            name,
            synthesis_mode,
            muted: false,
            soloed: false,
            armed,
            volume: Db::UNITY,
            pan: Pan::CENTER,
            effects: Vec::new(),
            clip_count: 0,
            synth_param_infos: synthesis_mode.param_infos(sample_rate),
            synth_param_values: synthesis_mode.default_param_values(sample_rate),
        });

        // Select the new track if it's the first one.
        if self.tracks.len() == 1 {
            self.selected_track = 0;
            self.track_list_state.select(Some(0));
        }
        true
    }

    /// Remove the track at the given list index.
    pub fn remove_track(&mut self, index: usize) {
        let Some(track) = self.tracks.get(index) else {
            return;
        };
        let removed = self.engine.remove_track(track.id);
        if !self.status.report("Remove track", removed) {
            return;
        }
        self.tracks.remove(index);

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
            let muted = !track.muted;
            let result = self.engine.set_track_mute(track.id, muted);
            if self.status.report("Mute", result) {
                track.muted = muted;
            }
        }
    }

    /// Toggle solo on the track at the given index.
    pub fn toggle_solo(&mut self, index: usize) {
        if let Some(track) = self.tracks.get_mut(index) {
            let soloed = !track.soloed;
            let result = self.engine.set_track_solo(track.id, soloed);
            if self.status.report("Solo", result) {
                track.soloed = soloed;
            }
        }
    }

    /// Toggle arm (record enable) on the track at the given index.
    pub fn toggle_arm(&mut self, index: usize) {
        if let Some(track) = self.tracks.get_mut(index) {
            let armed = !track.armed;
            let result = self
                .engine
                .send_command(EngineCommand::SetTrackArm(track.id, armed));
            if self.status.report("Arm", result) {
                track.armed = armed;
            }
        }
    }

    /// Cycle the synthesis mode on the track at the given index.
    ///
    /// Resets the synth parameter metadata and values to the new mode's
    /// defaults.
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
            let result = self.engine.set_track_synthesis_mode(track.id, next);
            if !self.status.report("Change synth mode", result) {
                return;
            }
            track.synthesis_mode = next;
            let sample_rate = self.engine.sample_rate() as f32;
            track.synth_param_infos = next.param_infos(sample_rate);
            track.synth_param_values = next.default_param_values(sample_rate);
        }
        self.synth_state.selected_synth_param = 0;
    }

    /// Set the volume for the track at the given index.
    pub fn set_track_volume(&mut self, index: usize, db: Db) {
        if let Some(track) = self.tracks.get_mut(index) {
            let result = self.engine.set_track_volume(track.id, db);
            if self.status.report("Set volume", result) {
                track.volume = db;
            }
        }
    }

    /// Set the pan for the track at the given index.
    pub fn set_track_pan(&mut self, index: usize, pan: Pan) {
        if let Some(track) = self.tracks.get_mut(index) {
            let result = self.engine.set_track_pan(track.id, pan);
            if self.status.report("Set pan", result) {
                track.pan = pan;
            }
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
            let info = EffectInfo::from_processor(name, effect.as_ref());
            let result = self.engine.add_effect(track.id, effect);
            if self.status.report("Add effect", result) {
                track.effects.push(info);
            }
        }
    }

    /// Toggle bypass on an effect in the selected track's chain.
    pub fn toggle_effect_bypass(&mut self, track_index: usize, effect_index: usize) {
        if let Some(track) = self.tracks.get_mut(track_index) {
            if let Some(effect) = track.effects.get_mut(effect_index) {
                let new_bypassed = !effect.bypassed;
                let result = self.engine.send_command(EngineCommand::SetEffectBypass {
                    track_id: track.id,
                    effect_index,
                    bypassed: new_bypassed,
                });
                if self.status.report("Bypass effect", result) {
                    effect.bypassed = new_bypassed;
                }
            }
        }
    }

    /// Remove an effect from a track's chain by index.
    pub fn remove_effect(&mut self, track_index: usize, effect_index: usize) {
        let Some(track) = self.tracks.get_mut(track_index) else {
            return;
        };
        if effect_index >= track.effects.len() {
            return;
        }
        let result = self.engine.send_command(EngineCommand::RemoveEffect {
            track_id: track.id,
            effect_index,
        });
        if !self.status.report("Remove effect", result) {
            return;
        }
        track.effects.remove(effect_index);

        // Adjust selection indices if the removed effect was on the
        // currently selected track.
        if track_index == self.selected_track {
            if track.effects.is_empty() {
                self.synth_state.selected_effect = 0;
            } else if self.synth_state.selected_effect >= track.effects.len() {
                self.synth_state.selected_effect = track.effects.len() - 1;
            }
            self.synth_state.selected_param = 0;
        }
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
        self.display
            .timeline
            .tracks
            .iter()
            .any(|t| !t.clips.is_empty() || t.recording.is_some())
    }

    /// Open the file browser starting in the current working directory.
    ///
    /// If the working directory cannot be determined, the browser opens at
    /// the filesystem root and the reason is shown in the status line.
    pub fn open_file_browser(&mut self) {
        let dir = match std::env::current_dir() {
            Ok(dir) => dir,
            Err(err) => {
                self.status.error(format!(
                    "Cannot determine working directory ({err}); browsing /"
                ));
                PathBuf::from("/")
            }
        };
        self.browse_to(dir);
    }

    /// Navigate the file browser to `dir`.
    ///
    /// On success the browser shows `dir` (opening the browser if it was
    /// closed) and returns `true`. If the directory cannot be read, the error
    /// is shown in the status line, the current mode is left unchanged, and
    /// `false` is returned.
    pub fn browse_to(&mut self, dir: PathBuf) -> bool {
        match Self::scan_directory(&dir) {
            Ok(listing) => {
                if listing.unreadable > 0 {
                    self.status.error(format!(
                        "{} entr{} in {} could not be read",
                        listing.unreadable,
                        if listing.unreadable == 1 { "y" } else { "ies" },
                        dir.display()
                    ));
                }
                self.mode = AppMode::FileBrowser {
                    directory: dir,
                    entries: listing.entries,
                    selected: 0,
                };
                true
            }
            Err(err) => {
                self.status
                    .error(format!("Cannot open {}: {err}", dir.display()));
                false
            }
        }
    }

    /// Scan a directory for subdirectories and audio files.
    ///
    /// Hidden entries (leading `.`) are skipped. Entries whose metadata
    /// cannot be read are not listed but are counted in
    /// [`DirectoryListing::unreadable`].
    ///
    /// # Errors
    ///
    /// Returns the I/O error if the directory itself cannot be read.
    pub fn scan_directory(dir: &Path) -> io::Result<DirectoryListing> {
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        let mut unreadable = 0_usize;

        for entry in std::fs::read_dir(dir)? {
            let Ok(entry) = entry else {
                unreadable += 1;
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();

            // Skip hidden entries.
            if name.starts_with('.') {
                continue;
            }

            let path = entry.path();
            // `fs::metadata` follows symlinks so linked directories are
            // browsable; a broken link is reported as unreadable.
            let Ok(metadata) = std::fs::metadata(&path) else {
                unreadable += 1;
                continue;
            };
            let is_dir = metadata.is_dir();

            if is_dir {
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

        dirs.sort_by_cached_key(|e| e.name.to_lowercase());
        files.sort_by_cached_key(|e| e.name.to_lowercase());
        dirs.extend(files);
        Ok(DirectoryListing {
            entries: dirs,
            unreadable,
        })
    }

    /// Check whether a specific panel has focus.
    #[must_use]
    pub fn is_focused(&self, panel: FocusedPanel) -> bool {
        self.focused_panel == panel
    }
}

/// Check whether a filename has a recognised audio extension.
fn is_audio_file(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|ext| {
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
    use crate::status::StatusLevel;
    use crate::test_support::TestApp;

    #[test]
    fn panels_for_view_are_non_empty() {
        for view in ActiveView::ALL {
            assert!(!panels_for_view(view).is_empty());
        }
    }

    #[test]
    fn recording_blink_visible_alternates() {
        let mut app = TestApp::empty();

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
        let mut app = TestApp::empty();

        app.focused_panel = FocusedPanel::Tracks;
        assert!(app.is_focused(FocusedPanel::Tracks));
        assert!(!app.is_focused(FocusedPanel::Transport));
        assert!(!app.is_focused(FocusedPanel::Mixer));
    }

    #[test]
    fn new_creates_default_armed_track() {
        let app = TestApp::with_default_track();
        assert_eq!(app.tracks.len(), 1);
        assert_eq!(app.tracks[0].name, "1");
        assert!(app.tracks[0].armed, "default track must be armed");
        assert_eq!(app.tracks[0].synthesis_mode, SynthesisMode::PitchTracked);
        assert_eq!(app.focused_panel, FocusedPanel::Tracks);
    }

    #[test]
    fn new_sends_add_and_arm_commands() {
        let app = TestApp::with_default_track();
        let commands = app.take_commands();
        assert_eq!(commands.len(), 2);
        assert!(matches!(
            &commands[0],
            EngineCommand::AddTrack { track, .. }
                if track.get().is_some_and(|track| {
                    track.name() == "1"
                        && track.layer(0).map(kazoo_core::mixer::SynthLayer::mode)
                            == Some(SynthesisMode::PitchTracked)
                })
        ));
        assert!(matches!(
            commands[1],
            EngineCommand::SetTrackArm(TrackId(0), true)
        ));
    }

    #[test]
    fn device_lists_are_injected() {
        let (engine, _commands) = crate::test_support::engine_handle();
        let devices = AudioDevices {
            inputs: Ok(vec!["Mic".into()]),
            outputs: Ok(vec!["Speakers".into(), "Headphones".into()]),
        };
        let app = App::new_empty(engine, devices);
        assert_eq!(app.audio_io_state.input_devices, vec!["Mic".to_owned()]);
        assert_eq!(app.audio_io_state.output_devices.len(), 2);
        assert!(app.audio_io_state.input_device_error.is_none());
        assert!(app.audio_io_state.output_device_error.is_none());
        assert!(app.status.visible(std::time::Instant::now()).is_none());
    }

    #[test]
    fn device_enumeration_failure_is_surfaced() {
        let (engine, _commands) = crate::test_support::engine_handle();
        let devices = AudioDevices {
            inputs: Err(kazoo_core::Error::AudioDevice("no host".into())),
            outputs: Ok(vec!["Speakers".into()]),
        };
        let app = App::new_empty(engine, devices);
        assert!(app.audio_io_state.input_devices.is_empty());
        let error = app.audio_io_state.input_device_error.as_deref().unwrap();
        assert!(error.contains("no host"), "{error}");
        let msg = app.status.visible(std::time::Instant::now()).unwrap();
        assert_eq!(msg.level, StatusLevel::Error);
        assert!(msg.text.contains("Input device scan failed"));
        assert_eq!(
            app.audio_io_state.output_devices,
            vec!["Speakers".to_owned()]
        );
    }

    #[test]
    fn first_track_auto_arms() {
        let mut app = TestApp::empty();
        assert!(app.add_track("A".into(), SynthesisMode::PitchTracked));
        assert!(app.tracks[0].armed, "first track should auto-arm");

        // Second track should NOT auto-arm.
        assert!(app.add_track("B".into(), SynthesisMode::Granular));
        assert!(!app.tracks[1].armed, "second track should not auto-arm");
    }

    #[test]
    fn add_track_increments_id() {
        let mut app = TestApp::empty();

        app.add_track("Lead".into(), SynthesisMode::PitchTracked);
        app.add_track("Bass".into(), SynthesisMode::Granular);

        assert_eq!(app.tracks.len(), 2);
        assert_eq!(app.tracks[0].id, TrackId(0));
        assert_eq!(app.tracks[0].name, "Lead");
        assert_eq!(app.tracks[1].id, TrackId(1));
        assert_eq!(app.tracks[1].name, "Bass");
    }

    #[test]
    fn add_track_with_engine_down_adds_nothing_and_reports() {
        let mut app = TestApp::empty();
        app.disconnect_engine();

        assert!(!app.add_track("A".into(), SynthesisMode::PitchTracked));
        assert!(app.tracks.is_empty());

        let msg = app.status.visible(std::time::Instant::now()).unwrap();
        assert_eq!(msg.level, StatusLevel::Error);
        assert_eq!(msg.text, "Add track failed: Engine not running");
    }

    #[test]
    fn track_ids_come_from_the_engine_handle() {
        let mut app = TestApp::empty();
        assert!(app.add_track("A".into(), SynthesisMode::PitchTracked));
        assert!(app.add_track("B".into(), SynthesisMode::PitchTracked));
        app.remove_track(0);
        assert!(app.add_track("C".into(), SynthesisMode::PitchTracked));
        let ids: Vec<TrackId> = app.tracks.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![TrackId(1), TrackId(2)]);
    }

    #[test]
    fn the_track_limit_is_reported_and_no_track_is_shown() {
        let mut app = TestApp::empty();
        for i in 0..kazoo_core::MAX_TRACKS {
            assert!(app.add_track(format!("{i}"), SynthesisMode::PitchTracked));
        }
        assert!(!app.add_track("Extra".into(), SynthesisMode::PitchTracked));
        assert_eq!(app.tracks.len(), kazoo_core::MAX_TRACKS);
        let msg = app.status.visible(std::time::Instant::now()).unwrap();
        assert_eq!(
            msg.text,
            "Add track failed: the engine already has the maximum of 16 tracks"
        );
    }

    #[test]
    fn remove_track_adjusts_selection() {
        let mut app = TestApp::empty();

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
        let mut app = TestApp::empty();

        app.add_track("Solo".into(), SynthesisMode::Wavetable);
        app.remove_track(0);

        assert!(app.tracks.is_empty());
        assert_eq!(app.selected_track, 0);
        assert_eq!(app.track_list_state.selected(), None);
    }

    #[test]
    fn remove_track_with_engine_down_keeps_track() {
        let mut app = TestApp::with_tracks(1);
        app.disconnect_engine();
        app.remove_track(0);
        assert_eq!(app.tracks.len(), 1);
        let msg = app.status.visible(std::time::Instant::now()).unwrap();
        assert_eq!(msg.text, "Remove track failed: Engine not running");
    }

    #[test]
    fn toggle_mute_flips_state() {
        let mut app = TestApp::empty();

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        assert!(!app.tracks[0].muted);

        app.toggle_mute(0);
        assert!(app.tracks[0].muted);

        app.toggle_mute(0);
        assert!(!app.tracks[0].muted);
    }

    #[test]
    fn toggle_mute_sends_command() {
        let mut app = TestApp::with_tracks(1);
        app.take_commands();
        app.toggle_mute(0);
        let commands = app.take_commands();
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            commands[0],
            EngineCommand::SetTrackMute(TrackId(0), true)
        ));
    }

    #[test]
    fn toggle_mute_with_engine_down_keeps_state_and_reports() {
        let mut app = TestApp::with_tracks(1);
        app.disconnect_engine();
        app.toggle_mute(0);
        assert!(
            !app.tracks[0].muted,
            "UI must not show a mute the engine never got"
        );
        let msg = app.status.visible(std::time::Instant::now()).unwrap();
        assert_eq!(msg.text, "Mute failed: Engine not running");
    }

    #[test]
    fn toggle_solo_flips_state() {
        let mut app = TestApp::empty();

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.toggle_solo(0);
        assert!(app.tracks[0].soloed);
    }

    #[test]
    fn toggle_arm_flips_state() {
        let mut app = TestApp::empty();

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
    fn engine_down_leaves_every_track_setting_unchanged() {
        let mut app = TestApp::with_tracks(1);
        app.tracks[0].effects = crate::test_support::effects(&["FX"]);
        app.disconnect_engine();

        app.toggle_solo(0);
        app.toggle_arm(0);
        app.set_track_volume(0, Db::new(-6.0));
        app.set_track_pan(0, Pan::new(0.5));
        app.cycle_synth_mode(0);
        app.toggle_effect_bypass(0, 0);
        app.remove_effect(0, 0);

        let track = &app.tracks[0];
        assert!(!track.soloed);
        assert!(track.armed);
        assert!((track.volume.value() - Db::UNITY.value()).abs() < f32::EPSILON);
        assert!((track.pan.value() - Pan::CENTER.value()).abs() < f32::EPSILON);
        assert_eq!(track.synthesis_mode, SynthesisMode::PitchTracked);
        assert_eq!(track.effects.len(), 1);
        assert!(!track.effects[0].bypassed);
        let msg = app.status.visible(std::time::Instant::now()).unwrap();
        assert_eq!(msg.level, StatusLevel::Error);
        assert_eq!(msg.text, "Remove effect failed: Engine not running");
    }

    #[test]
    fn selected_track_id_returns_none_when_empty() {
        let app = TestApp::empty();
        assert!(app.selected_track_id().is_none());
    }

    #[test]
    fn selected_track_id_returns_correct_id() {
        let mut app = TestApp::empty();

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.selected_track = 0;
        assert_eq!(app.selected_track_id(), Some(TrackId(0)));
    }

    #[test]
    fn toggle_effect_bypass_out_of_bounds_is_noop() {
        let mut app = TestApp::empty();
        app.add_track("T".into(), SynthesisMode::PitchTracked);

        // No effects added — should not panic.
        app.toggle_effect_bypass(0, 0);
        assert!(app.tracks[0].effects.is_empty());
    }

    #[test]
    fn set_track_volume_updates_local_state() {
        let mut app = TestApp::empty();
        app.add_track("T".into(), SynthesisMode::PitchTracked);

        app.set_track_volume(0, Db::new(-6.0));
        assert!((app.tracks[0].volume.value() - (-6.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn set_track_pan_updates_local_state() {
        let mut app = TestApp::empty();
        app.add_track("T".into(), SynthesisMode::PitchTracked);

        app.set_track_pan(0, Pan::new(0.5));
        assert!((app.tracks[0].pan.value() - 0.5).abs() < f32::EPSILON);
    }

    // -- M8: remove_effect adjusts selected_effect --------------------------

    #[test]
    fn remove_effect_clamps_selected_effect() {
        let mut app = TestApp::empty();

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        // Manually add effect metadata (we can't add real Processor objects
        // in unit tests, but we can simulate the metadata).
        app.tracks[0].effects = crate::test_support::effects(&["FX1", "FX2", "FX3"]);
        app.selected_track = 0;
        app.synth_state.selected_effect = 2; // pointing at FX3
        app.synth_state.selected_param = 3;

        app.remove_effect(0, 2); // remove FX3

        // selected_effect should clamp to the new last index (1).
        assert_eq!(app.synth_state.selected_effect, 1);
        // selected_param should reset to 0.
        assert_eq!(app.synth_state.selected_param, 0);
        assert_eq!(app.tracks[0].effects.len(), 2);
    }

    #[test]
    fn remove_all_effects_resets_selected_effect() {
        let mut app = TestApp::empty();

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.tracks[0].effects = crate::test_support::effects(&["FX1"]);
        app.selected_track = 0;
        app.synth_state.selected_effect = 0;

        app.remove_effect(0, 0);

        assert_eq!(app.synth_state.selected_effect, 0);
        assert_eq!(app.synth_state.selected_param, 0);
        assert!(app.tracks[0].effects.is_empty());
    }

    #[test]
    fn remove_effect_on_other_track_does_not_change_selection() {
        let mut app = TestApp::empty();

        app.add_track("T1".into(), SynthesisMode::PitchTracked);
        app.add_track("T2".into(), SynthesisMode::Granular);
        app.tracks[0].effects = crate::test_support::effects(&["FX1", "FX2"]);
        app.tracks[1].effects = crate::test_support::effects(&["FX3"]);
        app.selected_track = 0;
        app.synth_state.selected_effect = 1;
        app.synth_state.selected_param = 2;

        // Remove from track 1 (not the selected track).
        app.remove_effect(1, 0);

        // Selection on the selected track should be untouched.
        assert_eq!(app.synth_state.selected_effect, 1);
        assert_eq!(app.synth_state.selected_param, 2);
    }

    // -- Timeline state -------------------------------------------------------

    #[test]
    fn initial_timeline_state() {
        let app = TestApp::empty();
        assert!((app.tracking_state.timeline_zoom - 256.0).abs() < f64::EPSILON);
        assert!((app.tracking_state.timeline_scroll - 0.0).abs() < f64::EPSILON);
        assert!(app.tracking_state.selected_clip.is_none());
    }

    #[test]
    fn has_clips_returns_false_with_no_clips() {
        let app = TestApp::empty();
        assert!(!app.has_clips());
    }

    #[test]
    fn tracking_view_tab_order_includes_timeline_after_tracks() {
        let panels = panels_for_view(ActiveView::Tracking);
        let tracks = panels
            .iter()
            .position(|p| *p == FocusedPanel::Tracks)
            .unwrap();
        assert_eq!(panels[tracks + 1], FocusedPanel::Timeline);
        assert_eq!(panels[tracks + 2], FocusedPanel::Waveform);
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
        let mut app = TestApp::empty();
        app.add_track("T".into(), SynthesisMode::PitchTracked);
        assert_eq!(app.tracks[0].clip_count, 0);
    }

    #[test]
    fn file_browser_entry_debug() {
        let entry = FileBrowserEntry {
            name: "test.wav".into(),
            path: PathBuf::from("/tmp/test.wav"),
            is_dir: false,
        };
        let dbg = format!("{entry:?}");
        assert!(dbg.contains("test.wav"));
    }

    // -- Directory scanning ---------------------------------------------------

    /// A uniquely named scratch directory removed on drop.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "kazoo-tui-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            if dir.exists() {
                std::fs::remove_dir_all(&dir).unwrap();
            }
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            if let Err(err) = std::fs::remove_dir_all(&self.0) {
                // Drop cannot fail; a leaked temp dir is reported, not hidden.
                eprintln!("could not remove {}: {err}", self.0.display());
            }
        }
    }

    #[test]
    fn scan_directory_sorts_dirs_first_and_filters_files() {
        let scratch = ScratchDir::new("scan");
        std::fs::create_dir(scratch.0.join("beta")).unwrap();
        std::fs::create_dir(scratch.0.join("Alpha")).unwrap();
        std::fs::create_dir(scratch.0.join(".hidden")).unwrap();
        std::fs::write(scratch.0.join("zed.wav"), b"").unwrap();
        std::fs::write(scratch.0.join("Bee.FLAC"), b"").unwrap();
        std::fs::write(scratch.0.join("notes.txt"), b"").unwrap();

        let listing = App::scan_directory(&scratch.0).unwrap();
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "beta", "Bee.FLAC", "zed.wav"]);
        assert!(listing.entries[0].is_dir && listing.entries[1].is_dir);
        assert!(!listing.entries[2].is_dir);
        assert_eq!(listing.unreadable, 0);
    }

    #[test]
    fn scan_directory_missing_dir_is_an_error() {
        let scratch = ScratchDir::new("missing");
        let missing = scratch.0.join("does-not-exist");
        assert!(App::scan_directory(&missing).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn scan_directory_counts_broken_symlinks_as_unreadable() {
        let scratch = ScratchDir::new("symlink");
        std::os::unix::fs::symlink(scratch.0.join("nowhere"), scratch.0.join("dangling.wav"))
            .unwrap();
        let listing = App::scan_directory(&scratch.0).unwrap();
        assert!(listing.entries.is_empty());
        assert_eq!(listing.unreadable, 1);
    }

    #[test]
    fn browse_to_unreadable_dir_reports_and_keeps_mode() {
        let scratch = ScratchDir::new("browse");
        let mut app = TestApp::empty();
        assert!(!app.browse_to(scratch.0.join("absent")));
        assert_eq!(app.mode, AppMode::Normal);
        let msg = app.status.visible(std::time::Instant::now()).unwrap();
        assert_eq!(msg.level, StatusLevel::Error);
        assert!(msg.text.starts_with("Cannot open"), "{}", msg.text);
    }

    #[test]
    fn browse_to_readable_dir_opens_browser() {
        let scratch = ScratchDir::new("browse-ok");
        std::fs::write(scratch.0.join("a.wav"), b"").unwrap();
        let mut app = TestApp::empty();
        assert!(app.browse_to(scratch.0.clone()));
        let AppMode::FileBrowser {
            directory,
            entries,
            selected,
        } = &app.mode
        else {
            panic!("file browser should be open");
        };
        assert_eq!(directory, &scratch.0);
        assert_eq!(entries.len(), 1);
        assert_eq!(*selected, 0);
    }

    #[test]
    fn cycle_synth_mode_resets_param_selection_and_values() {
        let mut app = TestApp::empty();
        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.synth_state.selected_synth_param = 3;

        app.cycle_synth_mode(0);

        assert_eq!(app.synth_state.selected_synth_param, 0);
        assert_eq!(app.tracks[0].synthesis_mode, SynthesisMode::Wavetable);
        let sample_rate = crate::test_support::TEST_SAMPLE_RATE as f32;
        assert_eq!(
            app.tracks[0].synth_param_values,
            SynthesisMode::Wavetable.default_param_values(sample_rate)
        );
    }
}
