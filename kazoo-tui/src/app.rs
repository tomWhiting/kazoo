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

/// Target frames per second for UI rendering.
const TARGET_FPS: u64 = 30;

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
    Waveform,
    Spectrum,
    Effects,
    Mixer,
}

impl FocusedPanel {
    /// Cycle to the next panel in tab order.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Transport => Self::Tracks,
            Self::Tracks => Self::Waveform,
            Self::Waveform => Self::Spectrum,
            Self::Spectrum => Self::Effects,
            Self::Effects => Self::Mixer,
            Self::Mixer => Self::Transport,
        }
    }

    /// Cycle to the previous panel in tab order.
    #[must_use]
    pub const fn prev(self) -> Self {
        match self {
            Self::Transport => Self::Mixer,
            Self::Tracks => Self::Transport,
            Self::Waveform => Self::Tracks,
            Self::Spectrum => Self::Waveform,
            Self::Effects => Self::Spectrum,
            Self::Mixer => Self::Effects,
        }
    }
}

// ---------------------------------------------------------------------------
// App mode / input mode
// ---------------------------------------------------------------------------

/// Top-level application mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    /// Normal operating mode — all panels active.
    Normal,
    /// Help overlay displayed on top of the normal view.
    Help,
}

/// Input sub-mode for parameter editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Normal navigation keybindings.
    Normal,
    /// Editing a parameter value (captures numeric/text input).
    ParameterEdit,
}

// ---------------------------------------------------------------------------
// Track metadata
// ---------------------------------------------------------------------------

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
    /// Active synthesis mode.
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

    /// Index of the selected effect in the focused track's effect chain.
    pub selected_effect: usize,

    /// Index of the selected parameter in the focused effect.
    pub selected_param: usize,

    /// Waveform display zoom factor (1.0 = fit entire buffer).
    pub waveform_zoom: f32,

    /// Waveform display horizontal scroll position (0.0–1.0).
    pub waveform_scroll: f32,

    /// Ratatui list selection state for the track list widget.
    pub track_list_state: ListState,

    /// Frame counter for animations (recording blink at ~2 Hz, etc.).
    pub frame_count: u64,

    /// Current master bus volume. Tracked locally since [`DisplayState`]
    /// only carries meter readings, not the volume knob position.
    pub master_volume: Db,

    /// Text buffer for numeric input in `ParameterEdit` mode.
    pub param_edit_buffer: String,

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

        Self {
            engine,
            display,
            tracks: Vec::new(),
            next_track_id: 0,
            mode: AppMode::Normal,
            focused_panel: FocusedPanel::Transport,
            input_mode: InputMode::Normal,
            selected_track: 0,
            selected_effect: 0,
            selected_param: 0,
            waveform_zoom: 1.0,
            waveform_scroll: 0.0,
            track_list_state,
            frame_count: 0,
            master_volume: Db::UNITY,
            param_edit_buffer: String::new(),
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
                    if let Some(Ok(event)) = maybe_event {
                        self.handle_event(&event);
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

        let info = TrackInfo {
            id,
            name: name.clone(),
            synthesis_mode,
            muted: false,
            soloed: false,
            armed: false,
            volume: Db::UNITY,
            pan: Pan::CENTER,
            effect_names: Vec::new(),
            effect_bypassed: Vec::new(),
        };
        self.tracks.push(info);

        // Select the new track if it's the first one.
        if self.tracks.len() == 1 {
            self.selected_track = 0;
            self.track_list_state.select(Some(0));
        }

        let _ = self.engine.add_track(name, synthesis_mode);
    }

    /// Remove the track at the given list index.
    pub fn remove_track(&mut self, index: usize) {
        if index >= self.tracks.len() {
            return;
        }

        let id = self.tracks[index].id;
        self.tracks.remove(index);
        let _ = self.engine.send_command(EngineCommand::RemoveTrack(id));

        // Adjust selection.
        if self.tracks.is_empty() {
            self.selected_track = 0;
            self.track_list_state.select(None);
        } else if self.selected_track >= self.tracks.len() {
            self.selected_track = self.tracks.len().saturating_sub(1);
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn remove_effect(&mut self, track_index: usize, effect_index: usize) {
        if let Some(track) = self.tracks.get_mut(track_index) {
            if effect_index < track.effect_names.len() {
                track.effect_names.remove(effect_index);
                track.effect_bypassed.remove(effect_index);
                let _ = self.engine.send_command(EngineCommand::RemoveEffect {
                    track_id: track.id,
                    effect_index,
                });
            }
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
    /// Blinks at approximately 2 Hz (toggles every 15 frames at 30 fps).
    #[must_use]
    pub const fn recording_blink_visible(&self) -> bool {
        (self.frame_count / 15) % 2 == 0
    }

    /// The number of tracks.
    #[must_use]
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Check whether a specific panel has focus.
    #[must_use]
    pub const fn is_focused(&self, panel: FocusedPanel) -> bool {
        matches!(
            (&self.focused_panel, &panel),
            (FocusedPanel::Transport, FocusedPanel::Transport)
                | (FocusedPanel::Tracks, FocusedPanel::Tracks)
                | (FocusedPanel::Waveform, FocusedPanel::Waveform)
                | (FocusedPanel::Spectrum, FocusedPanel::Spectrum)
                | (FocusedPanel::Effects, FocusedPanel::Effects)
                | (FocusedPanel::Mixer, FocusedPanel::Mixer)
        )
    }
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
        let mut app = App::new(engine_handle);

        // Frames 0-14: visible (frame_count/15 == 0, 0%2 == 0)
        app.frame_count = 0;
        assert!(app.recording_blink_visible());

        app.frame_count = 14;
        assert!(app.recording_blink_visible());

        // Frames 15-29: hidden (frame_count/15 == 1, 1%2 == 1)
        app.frame_count = 15;
        assert!(!app.recording_blink_visible());

        app.frame_count = 29;
        assert!(!app.recording_blink_visible());

        // Frames 30-44: visible again
        app.frame_count = 30;
        assert!(app.recording_blink_visible());
    }

    #[test]
    fn is_focused_checks_correctly() {
        let engine_handle = test_engine_handle();
        let mut app = App::new(engine_handle);

        app.focused_panel = FocusedPanel::Tracks;
        assert!(app.is_focused(FocusedPanel::Tracks));
        assert!(!app.is_focused(FocusedPanel::Transport));
        assert!(!app.is_focused(FocusedPanel::Mixer));
    }

    #[test]
    fn add_track_increments_id() {
        let engine_handle = test_engine_handle();
        let mut app = App::new(engine_handle);

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
        let mut app = App::new(engine_handle);

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
        let mut app = App::new(engine_handle);

        app.add_track("Solo".into(), SynthesisMode::Wavetable);
        app.remove_track(0);

        assert!(app.tracks.is_empty());
        assert_eq!(app.selected_track, 0);
        assert_eq!(app.track_list_state.selected(), None);
    }

    #[test]
    fn toggle_mute_flips_state() {
        let engine_handle = test_engine_handle();
        let mut app = App::new(engine_handle);

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
        let mut app = App::new(engine_handle);

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.toggle_solo(0);
        assert!(app.tracks[0].soloed);
    }

    #[test]
    fn toggle_arm_flips_state() {
        let engine_handle = test_engine_handle();
        let mut app = App::new(engine_handle);

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.toggle_arm(0);
        assert!(app.tracks[0].armed);
    }

    #[test]
    fn selected_track_id_returns_none_when_empty() {
        let engine_handle = test_engine_handle();
        let app = App::new(engine_handle);
        assert!(app.selected_track_id().is_none());
    }

    #[test]
    fn selected_track_id_returns_correct_id() {
        let engine_handle = test_engine_handle();
        let mut app = App::new(engine_handle);

        app.add_track("T".into(), SynthesisMode::PitchTracked);
        app.selected_track = 0;
        assert_eq!(app.selected_track_id(), Some(TrackId(0)));
    }

    #[test]
    fn toggle_effect_bypass_out_of_bounds_is_noop() {
        let engine_handle = test_engine_handle();
        let mut app = App::new(engine_handle);
        app.add_track("T".into(), SynthesisMode::PitchTracked);

        // No effects added — should not panic.
        app.toggle_effect_bypass(0, 0);
        assert!(app.tracks[0].effect_bypassed.is_empty());
    }

    #[test]
    fn set_track_volume_updates_local_state() {
        let engine_handle = test_engine_handle();
        let mut app = App::new(engine_handle);
        app.add_track("T".into(), SynthesisMode::PitchTracked);

        app.set_track_volume(0, Db::new(-6.0));
        assert!((app.tracks[0].volume.value() - (-6.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn set_track_pan_updates_local_state() {
        let engine_handle = test_engine_handle();
        let mut app = App::new(engine_handle);
        app.add_track("T".into(), SynthesisMode::PitchTracked);

        app.set_track_pan(0, Pan::new(0.5));
        assert!((app.tracks[0].pan.value() - 0.5).abs() < f32::EPSILON);
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
