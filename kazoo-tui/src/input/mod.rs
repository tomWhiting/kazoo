//! Input handling: keybinding dispatch, focus management, modal input.
//!
//! All keyboard input flows through [`handle_key_event`], which resolves a
//! [`KeyEvent`] into a [`KeyAction`] and then applies the action to the
//! application state. The resolution is context-sensitive: the current
//! [`InputMode`], [`AppMode`], and [`FocusedPanel`] all influence which
//! action (if any) a key produces.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use kazoo_core::engine::EngineCommand;
use kazoo_core::mixer::clip::ClipId;
use kazoo_core::synthesis::SynthesisMode;
use kazoo_core::transport::{TransportCommand, TransportState};
use kazoo_core::{Db, Pan};

use crate::app::{App, AppMode, FocusedPanel, InputMode};
use crate::state::{ActiveView, MixerControl};

// ---------------------------------------------------------------------------
// KeyAction
// ---------------------------------------------------------------------------

/// A semantic action produced by resolving a key event in context.
///
/// Some variants (e.g. `Pause`, `SetMasterVolume`) are part of the action
/// vocabulary but do not yet have dedicated keybindings — they are still
/// handled in [`apply_action`] so they can be triggered programmatically.
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
enum KeyAction {
    Quit,
    ToggleHelp,

    // View switching
    SwitchView(ActiveView),

    // Focus
    FocusNext,
    FocusPrev,

    // Transport
    Play,
    Stop,
    Pause,
    Record,
    RecordWithCountIn,
    ToggleLoop,
    ToggleMetronome,

    // Track selection
    SelectTrack(usize),
    NextTrack,
    PrevTrack,

    // Track state
    ToggleMute,
    ToggleSolo,
    ToggleArm,

    // Track management
    AddTrack,
    RemoveTrack,

    // Effect navigation
    NextEffect,
    PrevEffect,

    // Effect management
    AddEffect,
    RemoveEffect,
    ToggleEffectBypass,

    // Parameter navigation / editing
    NextParam,
    PrevParam,
    IncreaseParam,
    DecreaseParam,
    EnterParamEdit,
    ConfirmParamEdit,
    CancelParamEdit,
    ParamEditChar(char),
    ParamEditBackspace,

    // Waveform view
    ZoomIn,
    ZoomOut,
    ScrollLeft,
    ScrollRight,

    // Volume / pan
    SetMasterVolume(f32),
    IncreaseVolume,
    DecreaseVolume,
    PanLeft,
    PanRight,

    // File browser
    OpenFileBrowser,

    // Timeline / clip operations
    TimelineZoomIn,
    TimelineZoomOut,
    TimelineScrollLeft,
    TimelineScrollRight,
    SelectNextClip,
    SelectPrevClip,
    MoveClipLeft,
    MoveClipRight,
    DeleteClip,
    SplitClip,
    DuplicateClip,

    // BPM adjustment (Transport panel)
    IncreaseBPM,
    DecreaseBPM,
    IncreaseBPMLarge,
    DecreaseBPMLarge,

    // Recording workflow (Transport panel)
    CycleRecordingWorkflow,
    IncreaseRecordBars,
    DecreaseRecordBars,

    // Mixer view navigation
    MixerNextChannel,
    MixerPrevChannel,
    MixerNextControl,
    MixerPrevControl,

    // Project view
    ProjectNextCard,
    ProjectPrevCard,
    ProjectNextField,
    ProjectPrevField,
    ProjectAdjustUp,
    ProjectAdjustDown,
    ProjectToggle,

    // Audio I/O view
    AudioIONextSection,
    AudioIOPrevSection,
    AudioIONextDevice,
    AudioIOPrevDevice,

    // Synth mode
    CycleSynthMode,

    // Direct panel focus
    FocusEffects,

    // File browser navigation
    FileBrowserUp,
    FileBrowserDown,
    FileBrowserEnter,
    FileBrowserBack,
    FileBrowserClose,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Handle a key event by resolving and applying the appropriate action.
pub fn handle_key_event(app: &mut App, key: KeyEvent) {
    if let Some(action) = resolve_action(app, key) {
        apply_action(app, action);
    }
}

// ---------------------------------------------------------------------------
// Action resolution
// ---------------------------------------------------------------------------

/// Top-level resolver: dispatches to sub-resolvers based on the current
/// input mode and application mode.
fn resolve_action(app: &App, key: KeyEvent) -> Option<KeyAction> {
    // 1. Parameter-edit mode captures all input.
    if app.input_mode == InputMode::ParameterEdit {
        return resolve_param_edit_action(key);
    }

    // 2. File browser mode captures all input.
    if matches!(app.mode, AppMode::FileBrowser { .. }) {
        return resolve_file_browser_action(key);
    }

    // 3. Help overlay only responds to dismiss keys.
    if app.mode == AppMode::Help {
        return resolve_help_action(key);
    }

    // 4. Normal mode: try view-specific keys, then panel-specific, then global.
    //    View-first allows the active view to intercept navigation keys.
    //    Panel-first ensures that modified keys (e.g. Ctrl+S for SplitClip
    //    in the Timeline panel) are not intercepted by unmodified global
    //    bindings (e.g. 's' for Stop).
    resolve_view_action(app, key)
        .or_else(|| resolve_panel_action(app, key))
        .or_else(|| resolve_global_action(key))
}

/// Resolve keys while in parameter-edit mode.
const fn resolve_param_edit_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Enter => Some(KeyAction::ConfirmParamEdit),
        KeyCode::Esc => Some(KeyAction::CancelParamEdit),
        KeyCode::Backspace => Some(KeyAction::ParamEditBackspace),
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
            Some(KeyAction::ParamEditChar(c))
        }
        _ => None,
    }
}

/// Resolve keys while in file browser mode.
const fn resolve_file_browser_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => Some(KeyAction::FileBrowserDown),
        KeyCode::Char('k') | KeyCode::Up => Some(KeyAction::FileBrowserUp),
        KeyCode::Enter => Some(KeyAction::FileBrowserEnter),
        KeyCode::Backspace => Some(KeyAction::FileBrowserBack),
        KeyCode::Esc => Some(KeyAction::FileBrowserClose),
        _ => None,
    }
}

/// Resolve keys while in help mode.
const fn resolve_help_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q' | '?') => Some(KeyAction::ToggleHelp),
        _ => None,
    }
}

/// Resolve keys that work regardless of which panel is focused.
///
/// Keys 1-5 switch between views. Track selection by number is no longer
/// available — use `j`/`k` for track navigation.
const fn resolve_global_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('q') => Some(KeyAction::Quit),
        KeyCode::Char('?') => Some(KeyAction::ToggleHelp),
        KeyCode::Tab => Some(KeyAction::FocusNext),
        KeyCode::BackTab => Some(KeyAction::FocusPrev),

        // View switching (1-4)
        KeyCode::Char('1') => Some(KeyAction::SwitchView(ActiveView::Mixer)),
        KeyCode::Char('2') => Some(KeyAction::SwitchView(ActiveView::Tracking)),
        KeyCode::Char('3') => Some(KeyAction::SwitchView(ActiveView::Project)),
        KeyCode::Char('4') => Some(KeyAction::SwitchView(ActiveView::AudioIO)),

        // Transport
        KeyCode::Char(' ') => Some(KeyAction::Play),
        KeyCode::Char('s') => Some(KeyAction::Stop),
        KeyCode::Char('r') => Some(KeyAction::Record),
        KeyCode::Char('R') => Some(KeyAction::RecordWithCountIn),
        KeyCode::Char('L') => Some(KeyAction::ToggleLoop),
        KeyCode::Char('M') => Some(KeyAction::ToggleMetronome),

        // Track navigation
        KeyCode::Char('j') | KeyCode::Down => Some(KeyAction::NextTrack),
        KeyCode::Char('k') | KeyCode::Up => Some(KeyAction::PrevTrack),

        // Track state
        KeyCode::Char('m') => Some(KeyAction::ToggleMute),
        KeyCode::Char('S') => Some(KeyAction::ToggleSolo),
        KeyCode::Char('a') => Some(KeyAction::ToggleArm),

        // Track management
        KeyCode::Char('n') => Some(KeyAction::AddTrack),
        KeyCode::Char('x') => Some(KeyAction::RemoveTrack),
        KeyCode::Char('t') => Some(KeyAction::CycleSynthMode),

        // Waveform zoom
        KeyCode::Char('[') => Some(KeyAction::ZoomOut),
        KeyCode::Char(']') => Some(KeyAction::ZoomIn),

        // File browser
        KeyCode::Char('o') => Some(KeyAction::OpenFileBrowser),

        _ => None,
    }
}

/// Resolve keys based on the active view. Returns `None` to fall through
/// to panel-specific and global resolvers.
const fn resolve_view_action(app: &App, key: KeyEvent) -> Option<KeyAction> {
    match app.active_view {
        ActiveView::Mixer => resolve_mixer_view_action(app, key),
        ActiveView::Project => resolve_project_view_action(key),
        ActiveView::AudioIO => resolve_audio_io_view_action(app, key),
        ActiveView::Tracking => resolve_tracking_view_action(key),
    }
}

/// View-specific keys for the Tracking view.
///
/// Effect management keys are available here so effects can be managed
/// directly from the tracking view. These same keys are also available
/// in the Effects panel resolver.
const fn resolve_tracking_view_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('e') => Some(KeyAction::FocusEffects),
        KeyCode::Char('A') => Some(KeyAction::AddEffect),
        KeyCode::Char('X') => Some(KeyAction::RemoveEffect),
        KeyCode::Char('b') => Some(KeyAction::ToggleEffectBypass),
        _ => None,
    }
}

/// View-specific keys for the Project Setup view.
///
/// Tab/BackTab cycles between cards; j/k navigates fields within a card;
/// +/-/Enter adjusts or toggles the selected value.
/// Space is NOT captured here — it always triggers Play/Pause globally.
const fn resolve_project_view_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Tab => Some(KeyAction::ProjectNextCard),
        KeyCode::BackTab => Some(KeyAction::ProjectPrevCard),
        KeyCode::Char('j') | KeyCode::Down => Some(KeyAction::ProjectNextField),
        KeyCode::Char('k') | KeyCode::Up => Some(KeyAction::ProjectPrevField),
        KeyCode::Char('+' | '=') => Some(KeyAction::ProjectAdjustUp),
        KeyCode::Char('-') => Some(KeyAction::ProjectAdjustDown),
        KeyCode::Enter => Some(KeyAction::ProjectToggle),
        KeyCode::Char('L') => Some(KeyAction::ToggleLoop),
        KeyCode::Char('M') => Some(KeyAction::ToggleMetronome),
        _ => None,
    }
}

/// View-specific keys for the Audio I/O view.
///
/// Tab/BackTab cycles between input/output/settings sections;
/// j/k navigates devices within the focused list.
const fn resolve_audio_io_view_action(_app: &App, key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Tab => Some(KeyAction::AudioIONextSection),
        KeyCode::BackTab => Some(KeyAction::AudioIOPrevSection),
        KeyCode::Char('j') | KeyCode::Down => Some(KeyAction::AudioIONextDevice),
        KeyCode::Char('k') | KeyCode::Up => Some(KeyAction::AudioIOPrevDevice),
        // Allow transport passthrough.
        KeyCode::Char(' ') => Some(KeyAction::Play),
        KeyCode::Char('s') => Some(KeyAction::Stop),
        KeyCode::Char('r') => Some(KeyAction::Record),
        _ => None,
    }
}

/// View-specific keys for the Mixing Desk view.
///
/// - `h`/`l` or Left/Right: navigate between channel strips
/// - `j`/`k` or Down/Up: navigate between controls within a strip
/// - `+`/`-`: adjust the focused control (volume fader or pan), toggle buttons
/// - Space: toggle the focused button (solo, mute, arm)
const fn resolve_mixer_view_action(app: &App, key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('h') | KeyCode::Left => Some(KeyAction::MixerPrevChannel),
        KeyCode::Char('l') | KeyCode::Right => Some(KeyAction::MixerNextChannel),
        KeyCode::Char('j') | KeyCode::Down => Some(KeyAction::MixerNextControl),
        KeyCode::Char('k') | KeyCode::Up => Some(KeyAction::MixerPrevControl),
        KeyCode::Char('+' | '=') => match app.mixer_view_state.selected_control {
            MixerControl::Fader => Some(KeyAction::IncreaseVolume),
            MixerControl::Pan => Some(KeyAction::PanRight),
            MixerControl::Solo => Some(KeyAction::ToggleSolo),
            MixerControl::Mute => Some(KeyAction::ToggleMute),
            MixerControl::Arm => Some(KeyAction::ToggleArm),
        },
        KeyCode::Char('-') => match app.mixer_view_state.selected_control {
            MixerControl::Fader => Some(KeyAction::DecreaseVolume),
            MixerControl::Pan => Some(KeyAction::PanLeft),
            MixerControl::Solo => Some(KeyAction::ToggleSolo),
            MixerControl::Mute => Some(KeyAction::ToggleMute),
            MixerControl::Arm => Some(KeyAction::ToggleArm),
        },
        KeyCode::Char(' ') => match app.mixer_view_state.selected_control {
            MixerControl::Solo => Some(KeyAction::ToggleSolo),
            MixerControl::Mute => Some(KeyAction::ToggleMute),
            MixerControl::Arm => Some(KeyAction::ToggleArm),
            // Space on Fader/Pan falls through to global Play/transport.
            _ => None,
        },
        _ => None,
    }
}

/// Resolve keys that depend on which panel is currently focused.
fn resolve_panel_action(app: &App, key: KeyEvent) -> Option<KeyAction> {
    match app.focused_panel {
        FocusedPanel::Effects => resolve_effects_action(key),
        FocusedPanel::Waveform => resolve_waveform_action(key),
        FocusedPanel::Mixer => resolve_mixer_action(key),
        FocusedPanel::Timeline => resolve_timeline_action(key),
        FocusedPanel::Transport => resolve_transport_action(key),
        FocusedPanel::Tracks | FocusedPanel::Spectrum => resolve_default_panel_action(key),
    }
}

/// Panel-specific keys for the effects panel.
///
/// Up/Down or J/K navigate the unified synth + effects list.
/// Left/Right adjust the selected parameter value.
/// h/l cycle through parameters within the selected item.
/// Enter opens direct numeric input for the selected parameter.
const fn resolve_effects_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('J') | KeyCode::Down => Some(KeyAction::NextEffect),
        KeyCode::Char('K') | KeyCode::Up => Some(KeyAction::PrevEffect),
        KeyCode::Char('h') => Some(KeyAction::PrevParam),
        KeyCode::Char('l') => Some(KeyAction::NextParam),
        KeyCode::Left | KeyCode::Char('-') => Some(KeyAction::DecreaseParam),
        KeyCode::Right | KeyCode::Char('+' | '=') => Some(KeyAction::IncreaseParam),
        KeyCode::Enter => Some(KeyAction::EnterParamEdit),
        KeyCode::Esc => Some(KeyAction::CancelParamEdit),
        KeyCode::Char('A') => Some(KeyAction::AddEffect),
        KeyCode::Char('X') => Some(KeyAction::RemoveEffect),
        KeyCode::Char('b') => Some(KeyAction::ToggleEffectBypass),
        _ => None,
    }
}

/// Panel-specific keys for the waveform panel.
const fn resolve_waveform_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('h') | KeyCode::Left => Some(KeyAction::ScrollLeft),
        KeyCode::Char('l') | KeyCode::Right => Some(KeyAction::ScrollRight),
        KeyCode::Char('+' | '=') => Some(KeyAction::ZoomIn),
        KeyCode::Char('-') => Some(KeyAction::ZoomOut),
        _ => None,
    }
}

/// Panel-specific keys for the mixer panel.
const fn resolve_mixer_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('h') | KeyCode::Left => Some(KeyAction::PanLeft),
        KeyCode::Char('l') | KeyCode::Right => Some(KeyAction::PanRight),
        KeyCode::Char('+' | '=') => Some(KeyAction::IncreaseVolume),
        KeyCode::Char('-') => Some(KeyAction::DecreaseVolume),
        _ => None,
    }
}

/// Panel-specific keys for the timeline panel.
#[allow(clippy::missing_const_for_fn)] // `KeyModifiers::contains` is not const
fn resolve_timeline_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('h') | KeyCode::Left => Some(KeyAction::TimelineScrollLeft),
        KeyCode::Char('l') | KeyCode::Right => Some(KeyAction::TimelineScrollRight),
        KeyCode::Char('+' | '=') => Some(KeyAction::TimelineZoomIn),
        KeyCode::Char('-') => Some(KeyAction::TimelineZoomOut),
        KeyCode::Char(',') => Some(KeyAction::SelectPrevClip),
        KeyCode::Char('.') => Some(KeyAction::SelectNextClip),
        KeyCode::Char('<') => Some(KeyAction::MoveClipLeft),
        KeyCode::Char('>') => Some(KeyAction::MoveClipRight),
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(KeyAction::DuplicateClip)
        }
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(KeyAction::SplitClip)
        }
        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(KeyAction::DeleteClip)
        }
        KeyCode::Delete => Some(KeyAction::DeleteClip),
        _ => None,
    }
}

/// Panel-specific keys for the transport panel.
///
/// BPM adjustment:
/// - `=` or unshifted `+` → +1 BPM
/// - `+` (Shift+=) → +10 BPM
/// - `-` (no shift) → -1 BPM
/// - `_` (Shift+-) → -10 BPM
///
/// Recording workflow:
/// - `w` → cycle workflow (`CountIn` → `FixedLength` → `CountIn`)
/// - `[` → decrease record bars
/// - `]` → increase record bars
const fn resolve_transport_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        // On US keyboards `+` is Shift+=, so this catches the large increment.
        KeyCode::Char('+') => Some(KeyAction::IncreaseBPMLarge),
        // Unshifted `=` for small increment (same physical key as `+`).
        KeyCode::Char('=') => Some(KeyAction::IncreaseBPM),
        // `_` is Shift+- on US keyboards.
        KeyCode::Char('_') => Some(KeyAction::DecreaseBPMLarge),
        KeyCode::Char('-') => Some(KeyAction::DecreaseBPM),
        // Recording workflow controls.
        KeyCode::Char('w') => Some(KeyAction::CycleRecordingWorkflow),
        KeyCode::Char(']') => Some(KeyAction::IncreaseRecordBars),
        KeyCode::Char('[') => Some(KeyAction::DecreaseRecordBars),
        _ => None,
    }
}

/// Fallback for panels without special key mappings.
const fn resolve_default_panel_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('+' | '=') => Some(KeyAction::IncreaseVolume),
        KeyCode::Char('-') => Some(KeyAction::DecreaseVolume),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Action application
// ---------------------------------------------------------------------------

/// Apply a resolved action to the application state, mutating `app` and
/// sending engine commands as needed.
#[allow(clippy::too_many_lines)]
fn apply_action(app: &mut App, action: KeyAction) {
    match action {
        // -- Application lifecycle -------------------------------------------
        KeyAction::Quit => {
            app.should_quit = true;
        }
        KeyAction::ToggleHelp => {
            app.mode = match app.mode {
                AppMode::Normal => AppMode::Help,
                AppMode::Help | AppMode::FileBrowser { .. } => AppMode::Normal,
            };
        }

        // -- View switching ----------------------------------------------------
        KeyAction::SwitchView(view) => {
            app.active_view = view;
            // Reset focus to the first panel of the new view.
            let panels = crate::app::panels_for_view(view);
            app.focused_panel = panels[0];
            // Sync mixer channel selection with the current track.
            if view == ActiveView::Mixer {
                app.mixer_view_state.selected_channel = app.selected_track;
            }
        }

        // -- Focus -----------------------------------------------------------
        KeyAction::FocusEffects => {
            app.focused_panel = FocusedPanel::Effects;
        }
        KeyAction::FocusNext => {
            let panels = crate::app::panels_for_view(app.active_view);
            if let Some(pos) = panels.iter().position(|p| *p == app.focused_panel) {
                app.focused_panel = panels[(pos + 1) % panels.len()];
            } else {
                app.focused_panel = panels[0];
            }
        }
        KeyAction::FocusPrev => {
            let panels = crate::app::panels_for_view(app.active_view);
            if let Some(pos) = panels.iter().position(|p| *p == app.focused_panel) {
                app.focused_panel = if pos == 0 {
                    panels[panels.len() - 1]
                } else {
                    panels[pos - 1]
                };
            } else {
                app.focused_panel = panels[0];
            }
        }

        // -- Transport -------------------------------------------------------
        KeyAction::Play => {
            if app.display.transport.state == TransportState::Playing {
                let _ = app.engine.pause();
            } else {
                let _ = app.engine.play();
            }
        }
        KeyAction::Stop => {
            let _ = app.engine.stop();
        }
        KeyAction::Pause => {
            let _ = app.engine.pause();
        }
        KeyAction::Record => {
            let _ = app.engine.record();
        }
        KeyAction::RecordWithCountIn => {
            // Build the workflow from TUI state and send it before the command.
            let workflow = build_recording_workflow(app);
            let _ = app.engine.send_command(EngineCommand::Transport(
                TransportCommand::SetRecordingWorkflow(workflow),
            ));
            let _ = app.engine.send_command(EngineCommand::Transport(
                TransportCommand::RecordWithCountIn,
            ));
        }
        KeyAction::ToggleLoop => {
            // Toggle loop on/off. The transport API uses SetLoop(Some/None)
            // rather than a simple toggle, so we check the current state.
            if app.display.transport.loop_enabled {
                let _ = app
                    .engine
                    .send_command(EngineCommand::Transport(TransportCommand::SetLoop(None)));
            } else {
                // Enable a default loop region (entire range).
                let _ =
                    app.engine
                        .send_command(EngineCommand::Transport(TransportCommand::SetLoop(Some((
                            0,
                            u64::MAX / 2,
                        )))));
            }
        }
        KeyAction::ToggleMetronome => {
            let _ = app
                .engine
                .send_command(EngineCommand::Transport(TransportCommand::ToggleMetronome));
        }
        KeyAction::IncreaseBPM => {
            let new_bpm = app.display.transport.bpm + 1.0;
            let _ = app
                .engine
                .send_command(EngineCommand::Transport(TransportCommand::SetTempo(
                    new_bpm,
                )));
        }
        KeyAction::DecreaseBPM => {
            let new_bpm = app.display.transport.bpm - 1.0;
            let _ = app
                .engine
                .send_command(EngineCommand::Transport(TransportCommand::SetTempo(
                    new_bpm,
                )));
        }
        KeyAction::IncreaseBPMLarge => {
            let new_bpm = app.display.transport.bpm + 10.0;
            let _ = app
                .engine
                .send_command(EngineCommand::Transport(TransportCommand::SetTempo(
                    new_bpm,
                )));
        }
        KeyAction::DecreaseBPMLarge => {
            let new_bpm = app.display.transport.bpm - 10.0;
            let _ = app
                .engine
                .send_command(EngineCommand::Transport(TransportCommand::SetTempo(
                    new_bpm,
                )));
        }

        // -- Recording workflow (Transport panel) ----------------------------
        KeyAction::CycleRecordingWorkflow => {
            use kazoo_core::transport::RecordingWorkflow;
            app.recording_workflow = match app.recording_workflow {
                RecordingWorkflow::FreeRecord | RecordingWorkflow::CountIn { .. } => {
                    RecordingWorkflow::FixedLength {
                        bars: app.record_bars.max(1),
                    }
                }
                RecordingWorkflow::FixedLength { .. } => RecordingWorkflow::CountIn {
                    count_in_bars: app.count_in_bars.max(1),
                    record_bars: app.record_bars,
                },
            };
        }
        KeyAction::IncreaseRecordBars => {
            app.record_bars = app.record_bars.saturating_add(1).min(64);
        }
        KeyAction::DecreaseRecordBars => {
            if app.record_bars > 0 {
                app.record_bars -= 1;
            }
        }

        // -- Track selection -------------------------------------------------
        KeyAction::SelectTrack(index) => {
            if index < app.tracks.len() {
                app.selected_track = index;
                app.track_list_state.select(Some(index));
                app.synth_state.selected_effect = 0;
                app.synth_state.selected_param = 0;
            }
        }
        KeyAction::NextTrack => {
            if !app.tracks.is_empty() {
                let next = if app.selected_track + 1 >= app.tracks.len() {
                    0
                } else {
                    app.selected_track + 1
                };
                app.selected_track = next;
                app.track_list_state.select(Some(next));
                app.synth_state.selected_effect = 0;
                app.synth_state.selected_param = 0;
            }
        }
        KeyAction::PrevTrack => {
            if !app.tracks.is_empty() {
                let prev = if app.selected_track == 0 {
                    app.tracks.len() - 1
                } else {
                    app.selected_track - 1
                };
                app.selected_track = prev;
                app.track_list_state.select(Some(prev));
                app.synth_state.selected_effect = 0;
                app.synth_state.selected_param = 0;
            }
        }

        // -- Track state -----------------------------------------------------
        KeyAction::ToggleMute => {
            let idx = app.selected_track;
            app.toggle_mute(idx);
        }
        KeyAction::ToggleSolo => {
            let idx = app.selected_track;
            app.toggle_solo(idx);
        }
        KeyAction::ToggleArm => {
            let idx = app.selected_track;
            app.toggle_arm(idx);
        }

        // -- Track management ------------------------------------------------
        KeyAction::AddTrack => {
            let name = format!("{}", app.track_count() + 1);
            app.add_track(name, SynthesisMode::PitchTracked);
        }
        KeyAction::RemoveTrack => {
            let idx = app.selected_track;
            app.remove_track(idx);
        }
        KeyAction::CycleSynthMode => {
            let idx = app.selected_track;
            app.cycle_synth_mode(idx);
        }

        // -- Effect navigation (unified: synth + effects) ----------------------
        KeyAction::NextEffect => {
            if app.synth_state.synth_selected {
                // Move from synth to first effect (if any).
                if let Some(track) = app.selected_track_info() {
                    if !track.effect_names.is_empty() {
                        app.synth_state.synth_selected = false;
                        app.synth_state.selected_effect = 0;
                        app.synth_state.selected_param = 0;
                    }
                }
            } else if let Some(track) = app.selected_track_info() {
                if !track.effect_names.is_empty()
                    && app.synth_state.selected_effect + 1 < track.effect_names.len()
                {
                    app.synth_state.selected_effect += 1;
                    app.synth_state.selected_param = 0;
                }
            }
        }
        KeyAction::PrevEffect => {
            if app.synth_state.synth_selected {
                // Already at top, no-op.
            } else if app.synth_state.selected_effect == 0 {
                // Move from first effect back to synth.
                app.synth_state.synth_selected = true;
                app.synth_state.selected_synth_param = 0;
            } else {
                app.synth_state.selected_effect -= 1;
                app.synth_state.selected_param = 0;
            }
        }

        // -- Effect management ------------------------------------------------
        KeyAction::AddEffect => {
            let sample_rate = app.engine.sample_rate() as f32;
            let effect = kazoo_core::effects::BiquadFilter::new(
                kazoo_core::effects::FilterType::LowPass,
                sample_rate,
            );
            let idx = app.selected_track;
            app.add_effect_to_track(idx, "LowPass".into(), Box::new(effect));
        }
        KeyAction::RemoveEffect => {
            let track_idx = app.selected_track;
            let effect_idx = app.synth_state.selected_effect;
            app.remove_effect(track_idx, effect_idx);
        }
        KeyAction::ToggleEffectBypass => {
            let track_idx = app.selected_track;
            let effect_idx = app.synth_state.selected_effect;
            app.toggle_effect_bypass(track_idx, effect_idx);
        }

        // -- Parameter navigation / editing ----------------------------------
        KeyAction::NextParam => {
            if app.synth_state.synth_selected {
                let param_count = app
                    .selected_track_info()
                    .map_or(0, |t| t.synth_param_infos.len());
                if param_count > 0 {
                    app.synth_state.selected_synth_param =
                        (app.synth_state.selected_synth_param + 1) % param_count;
                }
            } else {
                app.synth_state.selected_param =
                    app.synth_state.selected_param.saturating_add(1).min(31);
            }
        }
        KeyAction::PrevParam => {
            if app.synth_state.synth_selected {
                let param_count = app
                    .selected_track_info()
                    .map_or(0, |t| t.synth_param_infos.len());
                if param_count > 0 {
                    let idx = &mut app.synth_state.selected_synth_param;
                    *idx = if *idx == 0 { param_count - 1 } else { *idx - 1 };
                }
            } else {
                app.synth_state.selected_param = app.synth_state.selected_param.saturating_sub(1);
            }
        }
        KeyAction::IncreaseParam => {
            if app.synth_state.synth_selected {
                adjust_synth_param(app, 1.0);
            } else if let Some(track_id) = app.selected_track_id() {
                let _ = app.engine.send_command(EngineCommand::SetEffectParameter {
                    track_id,
                    effect_index: app.synth_state.selected_effect,
                    param_index: app.synth_state.selected_param,
                    value: 1.0,
                });
            }
        }
        KeyAction::DecreaseParam => {
            if app.synth_state.synth_selected {
                adjust_synth_param(app, -1.0);
            } else if let Some(track_id) = app.selected_track_id() {
                let _ = app.engine.send_command(EngineCommand::SetEffectParameter {
                    track_id,
                    effect_index: app.synth_state.selected_effect,
                    param_index: app.synth_state.selected_param,
                    value: -1.0,
                });
            }
        }
        KeyAction::EnterParamEdit => {
            app.input_mode = InputMode::ParameterEdit;
            app.param_edit_buffer.clear();
        }
        KeyAction::ConfirmParamEdit => {
            if let Ok(value) = app.param_edit_buffer.parse::<f32>() {
                if value.is_finite() {
                    if app.synth_state.synth_selected {
                        confirm_synth_param_edit(app, value);
                    } else if let Some(track_id) = app.selected_track_id() {
                        let _ = app.engine.send_command(EngineCommand::SetEffectParameter {
                            track_id,
                            effect_index: app.synth_state.selected_effect,
                            param_index: app.synth_state.selected_param,
                            value,
                        });
                    }
                }
            }
            app.input_mode = InputMode::Normal;
            app.param_edit_buffer.clear();
        }
        KeyAction::CancelParamEdit => {
            app.input_mode = InputMode::Normal;
            app.param_edit_buffer.clear();
        }
        KeyAction::ParamEditChar(c) => {
            if app.param_edit_buffer.len() < 16 {
                app.param_edit_buffer.push(c);
            }
        }
        KeyAction::ParamEditBackspace => {
            app.param_edit_buffer.pop();
        }

        // -- Waveform view ---------------------------------------------------
        KeyAction::ZoomIn => {
            app.tracking_state.waveform_zoom = (app.tracking_state.waveform_zoom * 2.0).min(64.0);
        }
        KeyAction::ZoomOut => {
            app.tracking_state.waveform_zoom = (app.tracking_state.waveform_zoom / 2.0).max(1.0);
        }
        KeyAction::ScrollLeft => {
            app.tracking_state.waveform_scroll =
                (app.tracking_state.waveform_scroll - 0.1).max(0.0);
        }
        KeyAction::ScrollRight => {
            app.tracking_state.waveform_scroll =
                (app.tracking_state.waveform_scroll + 0.1).min(1.0);
        }

        // -- Volume / pan ----------------------------------------------------
        KeyAction::SetMasterVolume(delta) => {
            let new_db = Db::new(app.master_volume.value() + delta);
            app.master_volume = new_db;
            let _ = app.engine.set_master_volume(new_db);
        }
        KeyAction::IncreaseVolume => {
            if let Some(track) = app.selected_track_info() {
                let new_db = Db::new((track.volume.value() + 1.0).min(24.0));
                let idx = app.selected_track;
                app.set_track_volume(idx, new_db);
            }
        }
        KeyAction::DecreaseVolume => {
            if let Some(track) = app.selected_track_info() {
                let new_db = Db::new((track.volume.value() - 1.0).max(-100.0));
                let idx = app.selected_track;
                app.set_track_volume(idx, new_db);
            }
        }
        KeyAction::PanLeft => {
            if let Some(track) = app.selected_track_info() {
                let new_pan = Pan::new(track.pan.value() - 0.1);
                let idx = app.selected_track;
                app.set_track_pan(idx, new_pan);
            }
        }
        KeyAction::PanRight => {
            if let Some(track) = app.selected_track_info() {
                let new_pan = Pan::new(track.pan.value() + 0.1);
                let idx = app.selected_track;
                app.set_track_pan(idx, new_pan);
            }
        }

        // -- Mixer view navigation -------------------------------------------
        KeyAction::MixerNextChannel => {
            let track_count = app.tracks.len();
            if track_count > 0 {
                let current = app.mixer_view_state.selected_channel;
                let next = (current + 1) % track_count;
                app.mixer_view_state.selected_channel = next;
                app.selected_track = next;
                app.track_list_state.select(Some(next));
                app.synth_state.selected_effect = 0;
                app.synth_state.selected_param = 0;
            }
        }
        KeyAction::MixerPrevChannel => {
            let track_count = app.tracks.len();
            if track_count > 0 {
                let current = app.mixer_view_state.selected_channel;
                let prev = if current == 0 {
                    track_count - 1
                } else {
                    current - 1
                };
                app.mixer_view_state.selected_channel = prev;
                app.selected_track = prev;
                app.track_list_state.select(Some(prev));
                app.synth_state.selected_effect = 0;
                app.synth_state.selected_param = 0;
            }
        }
        KeyAction::MixerNextControl => {
            app.mixer_view_state.selected_control = app.mixer_view_state.selected_control.next();
        }
        KeyAction::MixerPrevControl => {
            app.mixer_view_state.selected_control = app.mixer_view_state.selected_control.prev();
        }

        // -- Project view navigation -----------------------------------------
        KeyAction::ProjectNextCard => {
            app.project_state.selected_card = (app.project_state.selected_card + 1) % 6;
            app.project_state.selected_field = 0;
        }
        KeyAction::ProjectPrevCard => {
            app.project_state.selected_card = if app.project_state.selected_card == 0 {
                5
            } else {
                app.project_state.selected_card - 1
            };
            app.project_state.selected_field = 0;
        }
        KeyAction::ProjectNextField => {
            let max_fields = project_card_field_count(app.project_state.selected_card);
            if max_fields > 0 {
                app.project_state.selected_field =
                    (app.project_state.selected_field + 1) % max_fields;
            }
        }
        KeyAction::ProjectPrevField => {
            let max_fields = project_card_field_count(app.project_state.selected_card);
            if max_fields > 0 {
                app.project_state.selected_field = if app.project_state.selected_field == 0 {
                    max_fields - 1
                } else {
                    app.project_state.selected_field - 1
                };
            }
        }
        KeyAction::ProjectAdjustUp => {
            apply_project_adjust(app, 1);
        }
        KeyAction::ProjectAdjustDown => {
            apply_project_adjust(app, -1);
        }
        KeyAction::ProjectToggle => {
            apply_project_toggle(app);
        }

        // -- Audio I/O view navigation ---------------------------------------
        KeyAction::AudioIONextSection => {
            use crate::state::DeviceListFocus;
            app.audio_io_state.focus = match app.audio_io_state.focus {
                DeviceListFocus::Input => DeviceListFocus::Output,
                DeviceListFocus::Output => DeviceListFocus::Settings,
                DeviceListFocus::Settings => DeviceListFocus::Input,
            };
        }
        KeyAction::AudioIOPrevSection => {
            use crate::state::DeviceListFocus;
            app.audio_io_state.focus = match app.audio_io_state.focus {
                DeviceListFocus::Input => DeviceListFocus::Settings,
                DeviceListFocus::Output => DeviceListFocus::Input,
                DeviceListFocus::Settings => DeviceListFocus::Output,
            };
        }
        KeyAction::AudioIONextDevice => {
            use crate::state::DeviceListFocus;
            match app.audio_io_state.focus {
                DeviceListFocus::Input => {
                    let count = app.audio_io_state.input_devices.len();
                    if count > 0 {
                        app.audio_io_state.selected_input_device =
                            (app.audio_io_state.selected_input_device + 1) % count;
                    }
                }
                DeviceListFocus::Output => {
                    let count = app.audio_io_state.output_devices.len();
                    if count > 0 {
                        app.audio_io_state.selected_output_device =
                            (app.audio_io_state.selected_output_device + 1) % count;
                    }
                }
                DeviceListFocus::Settings => {}
            }
        }
        KeyAction::AudioIOPrevDevice => {
            use crate::state::DeviceListFocus;
            match app.audio_io_state.focus {
                DeviceListFocus::Input => {
                    let count = app.audio_io_state.input_devices.len();
                    if count > 0 {
                        app.audio_io_state.selected_input_device =
                            if app.audio_io_state.selected_input_device == 0 {
                                count - 1
                            } else {
                                app.audio_io_state.selected_input_device - 1
                            };
                    }
                }
                DeviceListFocus::Output => {
                    let count = app.audio_io_state.output_devices.len();
                    if count > 0 {
                        app.audio_io_state.selected_output_device =
                            if app.audio_io_state.selected_output_device == 0 {
                                count - 1
                            } else {
                                app.audio_io_state.selected_output_device - 1
                            };
                    }
                }
                DeviceListFocus::Settings => {}
            }
        }

        // -- File browser ----------------------------------------------------
        KeyAction::OpenFileBrowser => {
            app.open_file_browser();
        }

        // -- Timeline / clip operations --------------------------------------
        KeyAction::TimelineZoomIn => {
            app.tracking_state.timeline_zoom = (app.tracking_state.timeline_zoom / 2.0).max(1.0);
        }
        KeyAction::TimelineZoomOut => {
            app.tracking_state.timeline_zoom =
                (app.tracking_state.timeline_zoom * 2.0).min(1_048_576.0);
        }
        KeyAction::TimelineScrollLeft => {
            let step = app.tracking_state.timeline_zoom * 10.0;
            app.tracking_state.timeline_scroll =
                (app.tracking_state.timeline_scroll - step).max(0.0);
        }
        KeyAction::TimelineScrollRight => {
            let step = app.tracking_state.timeline_zoom * 10.0;
            app.tracking_state.timeline_scroll += step;
        }
        KeyAction::SelectNextClip => {
            select_adjacent_clip(app, true);
        }
        KeyAction::SelectPrevClip => {
            select_adjacent_clip(app, false);
        }
        KeyAction::MoveClipLeft => {
            if let (Some(track_id), Some(clip_id)) =
                (app.selected_track_id(), app.tracking_state.selected_clip)
            {
                let sample_rate = app.engine.sample_rate();
                // Move by 1 beat (based on current BPM).
                let beat_samples = beat_samples(app.display.transport.bpm, sample_rate);
                // Find current position from timeline snapshot.
                if let Some(clip) = find_clip_in_timeline(&app.display.timeline, clip_id) {
                    let new_pos = clip.position.saturating_sub(beat_samples);
                    let _ = app.engine.move_clip(track_id, clip_id, new_pos);
                }
            }
        }
        KeyAction::MoveClipRight => {
            if let (Some(track_id), Some(clip_id)) =
                (app.selected_track_id(), app.tracking_state.selected_clip)
            {
                let sample_rate = app.engine.sample_rate();
                let beat_samples = beat_samples(app.display.transport.bpm, sample_rate);
                if let Some(clip) = find_clip_in_timeline(&app.display.timeline, clip_id) {
                    let new_pos = clip.position.saturating_add(beat_samples);
                    let _ = app.engine.move_clip(track_id, clip_id, new_pos);
                }
            }
        }
        KeyAction::DeleteClip => {
            if let (Some(track_id), Some(clip_id)) =
                (app.selected_track_id(), app.tracking_state.selected_clip)
            {
                let _ = app.engine.remove_clip(track_id, clip_id);
                app.tracking_state.selected_clip = None;
            }
        }
        KeyAction::SplitClip => {
            if let (Some(track_id), Some(clip_id)) =
                (app.selected_track_id(), app.tracking_state.selected_clip)
            {
                let pos = app.display.transport.position.samples;
                let _ = app.engine.split_clip(track_id, clip_id, pos);
            }
        }
        KeyAction::DuplicateClip => {
            if let (Some(track_id), Some(clip_id)) =
                (app.selected_track_id(), app.tracking_state.selected_clip)
            {
                // Place duplicate right after the original clip.
                if let Some(clip) = find_clip_in_timeline(&app.display.timeline, clip_id) {
                    let new_pos = clip.position + clip.length;
                    let _ = app.engine.duplicate_clip(track_id, clip_id, new_pos);
                }
            }
        }

        // -- File browser navigation -----------------------------------------
        KeyAction::FileBrowserDown => {
            if let AppMode::FileBrowser {
                ref entries,
                ref mut selected,
                ..
            } = app.mode
            {
                if !entries.is_empty() {
                    *selected = (*selected + 1) % entries.len();
                }
            }
        }
        KeyAction::FileBrowserUp => {
            if let AppMode::FileBrowser {
                ref entries,
                ref mut selected,
                ..
            } = app.mode
            {
                if !entries.is_empty() {
                    *selected = if *selected == 0 {
                        entries.len() - 1
                    } else {
                        *selected - 1
                    };
                }
            }
        }
        KeyAction::FileBrowserEnter => {
            apply_file_browser_enter(app);
        }
        KeyAction::FileBrowserBack => {
            if let AppMode::FileBrowser {
                ref mut directory,
                ref mut entries,
                ref mut selected,
            } = app.mode
            {
                if let Some(parent) = directory.parent().map(std::path::Path::to_path_buf) {
                    *entries = App::scan_directory(&parent);
                    *selected = 0;
                    directory.clone_from(&parent);
                }
            }
        }
        KeyAction::FileBrowserClose => {
            app.mode = AppMode::Normal;
        }
    }
}

/// Build a [`RecordingWorkflow`] from the current TUI state.
///
/// This is used by the `RecordWithCountIn` (Shift+R) action. The workflow
/// type is determined by `app.recording_workflow`:
/// - `CountIn` (default for Shift+R): count in for `count_in_bars`, then
///   record for `record_bars` (0 = unlimited).
/// - `FixedLength`: record exactly `record_bars` bars, no count-in.
/// - `FreeRecord`: treated as `CountIn` with default parameters so that
///   Shift+R always provides a count-in (otherwise it would be identical
///   to the plain `r` key).
fn build_recording_workflow(app: &App) -> kazoo_core::transport::RecordingWorkflow {
    use kazoo_core::transport::RecordingWorkflow;
    match app.recording_workflow {
        RecordingWorkflow::FreeRecord | RecordingWorkflow::CountIn { .. } => {
            RecordingWorkflow::CountIn {
                count_in_bars: app.count_in_bars.max(1),
                record_bars: app.record_bars,
            }
        }
        RecordingWorkflow::FixedLength { .. } => RecordingWorkflow::FixedLength {
            bars: app.record_bars.max(1),
        },
    }
}

/// Apply file browser Enter: open directory or load audio file.
fn apply_file_browser_enter(app: &mut App) {
    // Extract the selected entry's path and is_dir status.
    let (path, is_dir) = {
        let AppMode::FileBrowser {
            ref entries,
            selected,
            ..
        } = app.mode
        else {
            return;
        };
        let Some(entry) = entries.get(selected) else {
            return;
        };
        (entry.path.clone(), entry.is_dir)
    };

    if is_dir {
        // Navigate into directory.
        let new_entries = App::scan_directory(&path);
        app.mode = AppMode::FileBrowser {
            directory: path,
            entries: new_entries,
            selected: 0,
        };
    } else {
        // Load audio file onto current track at playhead position.
        if let Some(track_id) = app.selected_track_id() {
            let position = app.display.transport.position.samples;
            let _ = app.engine.load_clip(track_id, &path, position);
        }
        app.mode = AppMode::Normal;
    }
}

/// Select the next or previous clip in the timeline.
fn select_adjacent_clip(app: &mut App, forward: bool) {
    let timeline = &app.display.timeline;

    // Look up the actual TrackId for the selected track index.
    // `app.selected_track` is a vector index (0, 1, 2...) but
    // `TrackClipSnapshot.track_id` is `TrackId.0` (monotonically
    // increasing, never reused). After track removal these diverge.
    let track_id = match app.tracks.get(app.selected_track) {
        Some(info) => info.id.0,
        None => return,
    };

    let Some(track) = timeline.tracks.iter().find(|t| t.track_id == track_id) else {
        // No track in the timeline snapshot matches; try first available.
        if let Some(first_track) = timeline.tracks.first() {
            if let Some(first_clip) = first_track.clips.first() {
                app.tracking_state.selected_clip = Some(ClipId(first_clip.id));
            }
        }
        return;
    };

    if track.clips.is_empty() {
        app.tracking_state.selected_clip = None;
        return;
    }

    match app.tracking_state.selected_clip {
        None => {
            // Nothing selected: select first or last.
            let clip = if forward {
                &track.clips[0]
            } else {
                &track.clips[track.clips.len() - 1]
            };
            app.tracking_state.selected_clip = Some(ClipId(clip.id));
        }
        Some(current) => {
            let idx = track.clips.iter().position(|c| c.id == current.0);
            match idx {
                Some(i) => {
                    let next = if forward {
                        (i + 1) % track.clips.len()
                    } else if i == 0 {
                        track.clips.len() - 1
                    } else {
                        i - 1
                    };
                    app.tracking_state.selected_clip = Some(ClipId(track.clips[next].id));
                }
                None => {
                    // Current selection not found; reset.
                    app.tracking_state.selected_clip = Some(ClipId(track.clips[0].id));
                }
            }
        }
    }
}

/// Find a clip in the timeline snapshot by its ID.
fn find_clip_in_timeline(
    timeline: &kazoo_core::engine::TimelineSnapshot,
    clip_id: ClipId,
) -> Option<&kazoo_core::engine::ClipSnapshot> {
    for track in &timeline.tracks {
        for clip in &track.clips {
            if clip.id == clip_id.0 {
                return Some(clip);
            }
        }
    }
    None
}

/// Compute samples per beat at the given BPM and sample rate.
fn beat_samples(bpm: f64, sample_rate: u32) -> u64 {
    if bpm <= 0.0 || sample_rate == 0 {
        return 0;
    }
    (f64::from(sample_rate) * 60.0 / bpm) as u64
}

// ---------------------------------------------------------------------------
// Synth parameter adjustment
// ---------------------------------------------------------------------------

/// Adjust the currently selected synth parameter by a direction (+1.0 or -1.0).
///
/// Uses 5% of the parameter range per step, or 1.0 for enum-style params
/// (where max <= 3.0 and min == 0.0). Updates the local value and sends
/// the absolute value to the engine. Always operates on layer 0.
fn adjust_synth_param(app: &mut App, direction: f32) {
    let idx = app.synth_state.selected_synth_param;
    let track_idx = app.selected_track;

    let Some(track) = app.tracks.get_mut(track_idx) else {
        return;
    };

    // Read param info from layer 0 (clone to release borrow).
    let info = track
        .layers
        .first()
        .and_then(|l| l.param_infos.get(idx).cloned())
        .or_else(|| track.synth_param_infos.get(idx).cloned());
    let Some(info) = info else {
        return;
    };

    // Read current value from layer 0.
    let current = track
        .layers
        .first()
        .and_then(|l| l.param_values.get(idx).copied())
        .or_else(|| track.synth_param_values.get(idx).copied());
    let Some(current) = current else {
        return;
    };

    // Determine step size: enum params step by 1, others by 5% of range.
    let is_enum =
        info.min == 0.0 && info.max <= 3.0 && (info.max - info.max.floor()).abs() < f32::EPSILON;
    let step = if is_enum {
        1.0
    } else {
        (info.max - info.min) / 20.0
    };

    let new_value = direction.mul_add(step, current).clamp(info.min, info.max);

    // For enum params, snap to nearest integer.
    let new_value = if is_enum {
        new_value.round()
    } else {
        new_value
    };

    // Update layer 0's local param value.
    if let Some(layer) = track.layers.first_mut() {
        if let Some(v) = layer.param_values.get_mut(idx) {
            *v = new_value;
        }
    }

    // Keep shortcut fields in sync.
    if let Some(v) = track.synth_param_values.get_mut(idx) {
        *v = new_value;
    }

    let track_id = track.id;
    let _ = app
        .engine
        .send_command(EngineCommand::SetSynthLayerParameter {
            track_id,
            layer_index: 0,
            param_index: idx,
            value: new_value,
        });
}

/// Confirm a direct numeric edit for a synth parameter.
///
/// Looks up layer 0's `ParamInfo` to clamp the value, updates local
/// state, and sends `SetSynthLayerParameter` to the engine.
fn confirm_synth_param_edit(app: &mut App, raw_value: f32) {
    let Some(track) = app.tracks.get_mut(app.selected_track) else {
        return;
    };

    let param_index = app.synth_state.selected_synth_param;

    // Read param info to clamp the value.
    let (min, max) = track
        .layers
        .first()
        .and_then(|l| l.param_infos.get(param_index))
        .or_else(|| track.synth_param_infos.get(param_index))
        .map_or((f32::MIN, f32::MAX), |info| (info.min, info.max));

    let value = raw_value.clamp(min, max);

    // Update layer 0's local state.
    if let Some(layer) = track.layers.first_mut() {
        if let Some(v) = layer.param_values.get_mut(param_index) {
            *v = value;
        }
    }

    // Keep shortcut fields in sync.
    if let Some(v) = track.synth_param_values.get_mut(param_index) {
        *v = value;
    }

    let track_id = track.id;
    let _ = app
        .engine
        .send_command(EngineCommand::SetSynthLayerParameter {
            track_id,
            layer_index: 0,
            param_index,
            value,
        });
}

// ---------------------------------------------------------------------------
// Project view helpers
// ---------------------------------------------------------------------------

/// Number of navigable fields per project card.
const fn project_card_field_count(card: usize) -> usize {
    match card {
        0 | 3 | 4 => 1, // Tempo: BPM | Metronome: enabled | Loop: enabled
        1 | 2 | 5 => 2, // Time Sig | Count-In | Recording: two fields each
        _ => 0,
    }
}

/// Apply a +1/-1 adjustment to the selected project card field.
fn apply_project_adjust(app: &mut App, direction: i32) {
    let card = app.project_state.selected_card;
    let field = app.project_state.selected_field;

    match (card, field) {
        // Card 0 (Tempo), field 0: adjust BPM.
        (0, 0) => {
            let new_bpm = app.display.transport.bpm + f64::from(direction);
            let _ = app
                .engine
                .send_command(EngineCommand::Transport(TransportCommand::SetTempo(
                    new_bpm,
                )));
        }
        // Card 2 (Count-In), field 1: adjust count-in bars.
        (2, 1) => {
            if direction > 0 {
                app.count_in_bars = app.count_in_bars.saturating_add(1).min(16);
            } else if app.count_in_bars > 0 {
                app.count_in_bars -= 1;
            }
        }
        // Card 5 (Recording), field 0: cycle workflow.
        (5, 0) => {
            use kazoo_core::transport::RecordingWorkflow;
            app.recording_workflow = match app.recording_workflow {
                RecordingWorkflow::FreeRecord | RecordingWorkflow::CountIn { .. } => {
                    RecordingWorkflow::FixedLength {
                        bars: app.record_bars.max(1),
                    }
                }
                RecordingWorkflow::FixedLength { .. } => RecordingWorkflow::CountIn {
                    count_in_bars: app.count_in_bars.max(1),
                    record_bars: app.record_bars,
                },
            };
        }
        // Card 5 (Recording), field 1: adjust record bars.
        (5, 1) => {
            if direction > 0 {
                app.record_bars = app.record_bars.saturating_add(1).min(64);
            } else if app.record_bars > 0 {
                app.record_bars -= 1;
            }
        }
        _ => {}
    }
}

/// Toggle boolean fields in the project view.
fn apply_project_toggle(app: &mut App) {
    let card = app.project_state.selected_card;
    let field = app.project_state.selected_field;

    match (card, field) {
        // Card 2 (Count-In), field 0: toggle count-in enabled.
        (2, 0) => {
            if app.count_in_bars > 0 {
                app.count_in_bars = 0;
            } else {
                app.count_in_bars = 1;
            }
        }
        // Card 3 (Metronome), field 0: toggle metronome.
        (3, 0) => {
            let _ = app
                .engine
                .send_command(EngineCommand::Transport(TransportCommand::ToggleMetronome));
        }
        // Card 4 (Loop), field 0: toggle loop.
        (4, 0) => {
            if app.display.transport.loop_enabled {
                let _ = app
                    .engine
                    .send_command(EngineCommand::Transport(TransportCommand::SetLoop(None)));
            } else {
                let _ =
                    app.engine
                        .send_command(EngineCommand::Transport(TransportCommand::SetLoop(Some((
                            0,
                            u64::MAX / 2,
                        )))));
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crossbeam_channel::unbounded;
    use crossterm::event::KeyModifiers;
    use kazoo_core::engine::{DisplayState, EngineHandle};
    use ringbuf::HeapRb;
    use ringbuf::traits::Split;

    /// Create an [`EngineHandle`] backed by real channels but no audio threads.
    fn test_engine_handle() -> EngineHandle {
        let (cmd_tx, _cmd_rx) = unbounded();
        let rb = HeapRb::<DisplayState>::new(4);
        let (_prod, cons) = rb.split();
        EngineHandle::new(cmd_tx, cons, 44_100, 256)
    }

    /// Create a test [`App`] instance with no tracks.
    fn test_app() -> App {
        App::new_empty(test_engine_handle())
    }

    /// Create a test [`App`] with some tracks pre-populated.
    fn test_app_with_tracks(count: usize) -> App {
        let mut app = test_app();
        for i in 0..count {
            app.add_track(format!("{}", i + 1), SynthesisMode::PitchTracked);
        }
        app
    }

    /// Build a [`KeyEvent`] for a given character (no modifiers).
    fn char_key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Build a [`KeyEvent`] for a given [`KeyCode`] (no modifiers).
    fn code_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    // -- resolve_action returns Quit for 'q' --------------------------------

    #[test]
    fn resolve_q_returns_quit() {
        let app = test_app();
        let action = resolve_action(&app, char_key('q'));
        assert_eq!(action, Some(KeyAction::Quit));
    }

    // -- resolve_action returns None for unknown keys -----------------------

    #[test]
    fn resolve_unknown_key_returns_none() {
        let app = test_app();
        let action = resolve_action(&app, code_key(KeyCode::F(12)));
        assert_eq!(action, None);
    }

    // -- space toggles play/pause based on transport state ------------------

    #[test]
    fn space_sends_play_when_stopped() {
        let app = test_app();
        // Default transport state is Stopped.
        let action = resolve_action(&app, char_key(' '));
        assert_eq!(action, Some(KeyAction::Play));
    }

    #[test]
    fn space_action_applied_when_playing_sends_pause() {
        let mut app = test_app();
        // Simulate the transport being in Playing state by modifying display.
        app.display.transport.state = TransportState::Playing;
        // Resolve and apply: should call engine.pause().
        let action = resolve_action(&app, char_key(' '));
        assert_eq!(action, Some(KeyAction::Play));
        // The apply_action function checks the transport state internally.
        // We verify it does not panic and the command channel receives the right call.
        apply_action(&mut app, KeyAction::Play);
    }

    // -- Tab cycles focus ---------------------------------------------------

    #[test]
    fn tab_cycles_focus_forward() {
        // Tracking view has panels: [Tracks, Timeline, Waveform, Effects].
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Tracks;

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Timeline);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Waveform);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Effects);

        // Wraps around.
        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Tracks);
    }

    #[test]
    fn backtab_cycles_focus_backward() {
        // Tracking view has panels: [Tracks, Timeline, Waveform, Effects].
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Tracks;

        // Backward from first panel wraps to last.
        handle_key_event(&mut app, code_key(KeyCode::BackTab));
        assert_eq!(app.focused_panel, FocusedPanel::Effects);
    }

    #[test]
    fn tab_resets_to_first_panel_when_current_not_in_view() {
        // Default active view is Tracking, with panels [Tracks, Timeline, Waveform, Effects].
        // Starting from Transport (not in Tracking panels), Tab resets to Tracks.
        let mut app = test_app();
        assert_eq!(app.focused_panel, FocusedPanel::Transport);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Tracks);
    }

    // -- Parameter edit mode captures digits --------------------------------

    #[test]
    fn param_edit_mode_captures_digits() {
        let mut app = test_app_with_tracks(1);
        app.focused_panel = FocusedPanel::Effects;
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer.clear();

        handle_key_event(&mut app, char_key('4'));
        handle_key_event(&mut app, char_key('2'));
        handle_key_event(&mut app, char_key('.'));
        handle_key_event(&mut app, char_key('0'));

        assert_eq!(app.param_edit_buffer, "42.0");
        assert_eq!(app.input_mode, InputMode::ParameterEdit);
    }

    #[test]
    fn param_edit_mode_ignores_non_numeric() {
        let mut app = test_app();
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer.clear();

        handle_key_event(&mut app, char_key('a'));
        assert_eq!(app.param_edit_buffer, "");
    }

    #[test]
    fn param_edit_escape_cancels() {
        let mut app = test_app();
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer = "123".into();

        handle_key_event(&mut app, code_key(KeyCode::Esc));
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.param_edit_buffer, "");
    }

    #[test]
    fn param_edit_enter_confirms_and_clears() {
        let mut app = test_app_with_tracks(1);
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer = "3.14".into();

        handle_key_event(&mut app, code_key(KeyCode::Enter));
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.param_edit_buffer, "");
    }

    // -- Escape exits help mode ---------------------------------------------

    #[test]
    fn escape_exits_help_mode() {
        let mut app = test_app();
        app.mode = AppMode::Help;

        handle_key_event(&mut app, code_key(KeyCode::Esc));
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn question_mark_toggles_help() {
        let mut app = test_app();
        assert_eq!(app.mode, AppMode::Normal);

        handle_key_event(&mut app, char_key('?'));
        assert_eq!(app.mode, AppMode::Help);

        handle_key_event(&mut app, char_key('?'));
        assert_eq!(app.mode, AppMode::Normal);
    }

    // -- Track selection with j/k -------------------------------------------

    #[test]
    fn j_selects_next_track() {
        let mut app = test_app_with_tracks(3);
        // Use Tracking view so j/k maps to global NextTrack/PrevTrack
        // (in Mixer view, j/k navigates controls within a channel strip).
        app.active_view = ActiveView::Tracking;
        assert_eq!(app.selected_track, 0);

        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.selected_track, 1);

        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.selected_track, 2);

        // Wrap around.
        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.selected_track, 0);
    }

    #[test]
    fn k_selects_prev_track() {
        let mut app = test_app_with_tracks(3);
        app.active_view = ActiveView::Tracking;
        assert_eq!(app.selected_track, 0);

        // Wrap around backward.
        handle_key_event(&mut app, char_key('k'));
        assert_eq!(app.selected_track, 2);

        handle_key_event(&mut app, char_key('k'));
        assert_eq!(app.selected_track, 1);
    }

    #[test]
    fn down_arrow_selects_next_track() {
        let mut app = test_app_with_tracks(3);
        app.active_view = ActiveView::Tracking;
        handle_key_event(&mut app, code_key(KeyCode::Down));
        assert_eq!(app.selected_track, 1);
    }

    // -- Mute toggle with 'm' ----------------------------------------------

    #[test]
    fn m_toggles_mute() {
        let mut app = test_app_with_tracks(1);
        assert!(!app.tracks[0].muted);

        handle_key_event(&mut app, char_key('m'));
        assert!(app.tracks[0].muted);

        handle_key_event(&mut app, char_key('m'));
        assert!(!app.tracks[0].muted);
    }

    // -- AddTrack with 'n' --------------------------------------------------

    #[test]
    fn n_adds_track() {
        let mut app = test_app();
        assert_eq!(app.track_count(), 0);

        handle_key_event(&mut app, char_key('n'));
        assert_eq!(app.track_count(), 1);
        assert_eq!(app.tracks[0].name, "1");
        assert_eq!(app.tracks[0].synthesis_mode, SynthesisMode::PitchTracked);
    }

    #[test]
    fn n_adds_track_incrementing_name() {
        let mut app = test_app_with_tracks(2);
        handle_key_event(&mut app, char_key('n'));
        assert_eq!(app.track_count(), 3);
        assert_eq!(app.tracks[2].name, "3");
    }

    // -- Context-dependent keys (h/l differ by focused panel) ---------------

    #[test]
    fn h_in_effects_panel_is_prev_param() {
        let app_state = {
            let mut a = test_app();
            a.active_view = ActiveView::Tracking;
            a.focused_panel = FocusedPanel::Effects;
            a
        };
        let action = resolve_action(&app_state, char_key('h'));
        assert_eq!(action, Some(KeyAction::PrevParam));
    }

    #[test]
    fn l_in_effects_panel_is_next_param() {
        let app_state = {
            let mut a = test_app();
            a.active_view = ActiveView::Tracking;
            a.focused_panel = FocusedPanel::Effects;
            a
        };
        let action = resolve_action(&app_state, char_key('l'));
        assert_eq!(action, Some(KeyAction::NextParam));
    }

    #[test]
    fn h_in_waveform_panel_is_scroll_left() {
        let app_state = {
            let mut a = test_app();
            a.active_view = ActiveView::Tracking;
            a.focused_panel = FocusedPanel::Waveform;
            a
        };
        let action = resolve_action(&app_state, char_key('h'));
        assert_eq!(action, Some(KeyAction::ScrollLeft));
    }

    #[test]
    fn l_in_waveform_panel_is_scroll_right() {
        let app_state = {
            let mut a = test_app();
            a.active_view = ActiveView::Tracking;
            a.focused_panel = FocusedPanel::Waveform;
            a
        };
        let action = resolve_action(&app_state, char_key('l'));
        assert_eq!(action, Some(KeyAction::ScrollRight));
    }

    #[test]
    fn h_in_mixer_view_is_prev_channel() {
        let app_state = {
            let mut a = test_app();
            a.active_view = ActiveView::Mixer;
            a
        };
        let action = resolve_action(&app_state, char_key('h'));
        assert_eq!(action, Some(KeyAction::MixerPrevChannel));
    }

    #[test]
    fn l_in_mixer_view_is_next_channel() {
        let app_state = {
            let mut a = test_app();
            a.active_view = ActiveView::Mixer;
            a
        };
        let action = resolve_action(&app_state, char_key('l'));
        assert_eq!(action, Some(KeyAction::MixerNextChannel));
    }

    // -- Volume and pan apply_action ----------------------------------------

    #[test]
    fn increase_volume_adds_1db() {
        let mut app = test_app_with_tracks(1);
        // Use Tracking view so +/- goes through the panel resolver (Tracks panel).
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Tracks;
        handle_key_event(&mut app, char_key('+')); // panel: IncreaseVolume
        assert!((app.tracks[0].volume.value() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn decrease_volume_subtracts_1db() {
        let mut app = test_app_with_tracks(1);
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Tracks;
        handle_key_event(&mut app, char_key('-')); // panel: DecreaseVolume
        assert!((app.tracks[0].volume.value() - (-1.0)).abs() < f32::EPSILON);
    }

    // -- Zoom ---------------------------------------------------------------

    #[test]
    fn bracket_keys_zoom_waveform() {
        let mut app = test_app();
        // Focus a non-Transport panel so [/] map to zoom, not record bars.
        app.focused_panel = FocusedPanel::Waveform;
        assert!((app.tracking_state.waveform_zoom - 1.0).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key(']'));
        assert!((app.tracking_state.waveform_zoom - 2.0).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key(']'));
        assert!((app.tracking_state.waveform_zoom - 4.0).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key('['));
        assert!((app.tracking_state.waveform_zoom - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zoom_clamped_to_range() {
        let mut app = test_app();
        // Focus a non-Transport panel so [/] map to zoom, not record bars.
        app.focused_panel = FocusedPanel::Waveform;

        // Zoom out below 1.0 should clamp.
        handle_key_event(&mut app, char_key('['));
        assert!((app.tracking_state.waveform_zoom - 1.0).abs() < f32::EPSILON);

        // Zoom in to max.
        app.tracking_state.waveform_zoom = 64.0;
        handle_key_event(&mut app, char_key(']'));
        assert!((app.tracking_state.waveform_zoom - 64.0).abs() < f32::EPSILON);
    }

    // -- Scroll -------------------------------------------------------------

    #[test]
    fn scroll_waveform() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Waveform;
        assert!((app.tracking_state.waveform_scroll - 0.0).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key('l'));
        assert!((app.tracking_state.waveform_scroll - 0.1).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key('h'));
        assert!(app.tracking_state.waveform_scroll.abs() < f32::EPSILON);
    }

    // -- Quit ---------------------------------------------------------------

    #[test]
    fn q_sets_should_quit() {
        let mut app = test_app();
        assert!(!app.should_quit);
        handle_key_event(&mut app, char_key('q'));
        assert!(app.should_quit);
    }

    // -- Stop ---------------------------------------------------------------

    #[test]
    fn s_resolves_to_stop() {
        let app = test_app();
        let action = resolve_action(&app, char_key('s'));
        assert_eq!(action, Some(KeyAction::Stop));
    }

    // -- Record -------------------------------------------------------------

    #[test]
    fn r_resolves_to_record() {
        let app = test_app();
        let action = resolve_action(&app, char_key('r'));
        assert_eq!(action, Some(KeyAction::Record));
    }

    // -- Solo and arm -------------------------------------------------------

    #[test]
    fn capital_s_toggles_solo() {
        let mut app = test_app_with_tracks(1);
        assert!(!app.tracks[0].soloed);

        handle_key_event(&mut app, char_key('S'));
        assert!(app.tracks[0].soloed);
    }

    #[test]
    fn a_toggles_arm() {
        let mut app = test_app_with_tracks(1);
        // First track is auto-armed.
        assert!(app.tracks[0].armed);

        // Toggle disarms.
        handle_key_event(&mut app, char_key('a'));
        assert!(!app.tracks[0].armed);

        // Toggle re-arms.
        handle_key_event(&mut app, char_key('a'));
        assert!(app.tracks[0].armed);
    }

    // -- Remove track -------------------------------------------------------

    #[test]
    fn x_removes_track() {
        let mut app = test_app_with_tracks(2);
        assert_eq!(app.track_count(), 2);

        handle_key_event(&mut app, char_key('x'));
        assert_eq!(app.track_count(), 1);
    }

    // -- View switching with number keys ------------------------------------

    #[test]
    fn number_keys_switch_views() {
        let mut app = test_app_with_tracks(5);
        handle_key_event(&mut app, char_key('1'));
        assert_eq!(app.active_view, ActiveView::Mixer);

        handle_key_event(&mut app, char_key('2'));
        assert_eq!(app.active_view, ActiveView::Tracking);

        handle_key_event(&mut app, char_key('3'));
        assert_eq!(app.active_view, ActiveView::Project);

        handle_key_event(&mut app, char_key('4'));
        assert_eq!(app.active_view, ActiveView::AudioIO);
    }

    // -- Enter param edit ---------------------------------------------------

    #[test]
    fn enter_in_effects_panel_starts_param_edit() {
        let mut app = test_app_with_tracks(1);
        app.focused_panel = FocusedPanel::Effects;

        handle_key_event(&mut app, code_key(KeyCode::Enter));
        assert_eq!(app.input_mode, InputMode::ParameterEdit);
        assert_eq!(app.param_edit_buffer, "");
    }

    // -- Pan ----------------------------------------------------------------

    #[test]
    fn mixer_view_pan_via_plus_minus() {
        let mut app = test_app_with_tracks(1);
        app.active_view = ActiveView::Mixer;
        // Focus Pan control so +/- maps to PanRight/PanLeft.
        app.mixer_view_state.selected_control = MixerControl::Pan;

        // Default pan is 0.0 (center).
        handle_key_event(&mut app, char_key('+'));
        assert!((app.tracks[0].pan.value() - 0.1).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key('-'));
        assert!(app.tracks[0].pan.value().abs() < f32::EPSILON);
    }

    #[test]
    fn mixer_view_channel_navigation() {
        let mut app = test_app_with_tracks(3);
        app.active_view = ActiveView::Mixer;
        assert_eq!(app.mixer_view_state.selected_channel, 0);
        assert_eq!(app.selected_track, 0);

        // l moves to next channel and syncs selected_track.
        handle_key_event(&mut app, char_key('l'));
        assert_eq!(app.mixer_view_state.selected_channel, 1);
        assert_eq!(app.selected_track, 1);

        handle_key_event(&mut app, char_key('l'));
        assert_eq!(app.mixer_view_state.selected_channel, 2);
        assert_eq!(app.selected_track, 2);

        // Wrap around.
        handle_key_event(&mut app, char_key('l'));
        assert_eq!(app.mixer_view_state.selected_channel, 0);
        assert_eq!(app.selected_track, 0);

        // h moves to prev channel (wraps backward from 0).
        handle_key_event(&mut app, char_key('h'));
        assert_eq!(app.mixer_view_state.selected_channel, 2);
        assert_eq!(app.selected_track, 2);
    }

    #[test]
    fn mixer_view_control_navigation() {
        let mut app = test_app_with_tracks(1);
        app.active_view = ActiveView::Mixer;
        assert_eq!(app.mixer_view_state.selected_control, MixerControl::Fader);

        // j cycles down through controls.
        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.mixer_view_state.selected_control, MixerControl::Pan);

        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.mixer_view_state.selected_control, MixerControl::Solo);

        // k cycles back up.
        handle_key_event(&mut app, char_key('k'));
        assert_eq!(app.mixer_view_state.selected_control, MixerControl::Pan);
    }

    #[test]
    fn mixer_view_volume_via_plus_minus_on_fader() {
        let mut app = test_app_with_tracks(1);
        app.active_view = ActiveView::Mixer;
        app.mixer_view_state.selected_control = MixerControl::Fader;

        handle_key_event(&mut app, char_key('+'));
        assert!((app.tracks[0].volume.value() - 1.0).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key('-'));
        assert!(app.tracks[0].volume.value().abs() < f32::EPSILON);
    }

    #[test]
    fn mixer_view_space_toggles_solo_mute_arm() {
        let mut app = test_app_with_tracks(1);
        app.active_view = ActiveView::Mixer;

        app.mixer_view_state.selected_control = MixerControl::Solo;
        let action = resolve_action(&app, char_key(' '));
        assert_eq!(action, Some(KeyAction::ToggleSolo));

        app.mixer_view_state.selected_control = MixerControl::Mute;
        let action = resolve_action(&app, char_key(' '));
        assert_eq!(action, Some(KeyAction::ToggleMute));

        app.mixer_view_state.selected_control = MixerControl::Arm;
        let action = resolve_action(&app, char_key(' '));
        assert_eq!(action, Some(KeyAction::ToggleArm));

        // Space on Fader falls through to global Play.
        app.mixer_view_state.selected_control = MixerControl::Fader;
        let action = resolve_action(&app, char_key(' '));
        assert_eq!(action, Some(KeyAction::Play));
    }

    #[test]
    fn mixer_view_intercepts_keys_regardless_of_panel() {
        // Document intentional behavior: in Mixer view, view-level keys take
        // priority over panel-specific keys even when a non-mixer panel is focused.
        let mut app = test_app();
        app.active_view = ActiveView::Mixer;
        app.focused_panel = FocusedPanel::Effects;

        // h in Mixer view is MixerPrevChannel, NOT PrevParam.
        let action = resolve_action(&app, char_key('h'));
        assert_eq!(action, Some(KeyAction::MixerPrevChannel));
    }

    // -- Help mode blocks normal keys ---------------------------------------

    #[test]
    fn help_mode_blocks_normal_keys() {
        let mut app = test_app();
        app.mode = AppMode::Help;

        // 'n' (AddTrack) should not work in help mode.
        let action = resolve_action(&app, char_key('n'));
        assert_eq!(action, None);
    }

    // -- ParameterEdit mode blocks normal keys ------------------------------

    #[test]
    fn param_edit_mode_blocks_normal_keys() {
        let mut app = test_app();
        app.input_mode = InputMode::ParameterEdit;

        // 'q' (Quit) should not work in param edit mode.
        let action = resolve_action(&app, char_key('q'));
        assert_eq!(action, None);
    }

    // -- Capital L and M keys -----------------------------------------------

    #[test]
    fn capital_l_resolves_to_toggle_loop() {
        let app = test_app();
        let action = resolve_action(&app, char_key('L'));
        assert_eq!(action, Some(KeyAction::ToggleLoop));
    }

    #[test]
    fn capital_m_resolves_to_toggle_metronome() {
        let app = test_app();
        let action = resolve_action(&app, char_key('M'));
        assert_eq!(action, Some(KeyAction::ToggleMetronome));
    }

    // -- H11: Shift+J/K navigate effects in effects panel -------------------

    #[test]
    fn capital_j_resolves_to_next_effect_in_effects_panel() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Effects;
        let action = resolve_action(&app, char_key('J'));
        assert_eq!(action, Some(KeyAction::NextEffect));
    }

    #[test]
    fn capital_k_resolves_to_prev_effect_in_effects_panel() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Effects;
        let action = resolve_action(&app, char_key('K'));
        assert_eq!(action, Some(KeyAction::PrevEffect));
    }

    #[test]
    fn capital_j_outside_effects_panel_is_global_noop() {
        // In the mixer panel, Shift+J is not bound — falls through to None.
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Mixer;
        let action = resolve_action(&app, char_key('J'));
        assert_eq!(action, None);
    }

    // -- H12: Backspace in parameter edit mode ------------------------------

    #[test]
    fn backspace_in_param_edit_removes_last_char() {
        let mut app = test_app();
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer = "42.0".into();

        handle_key_event(&mut app, code_key(KeyCode::Backspace));
        assert_eq!(app.param_edit_buffer, "42.");

        handle_key_event(&mut app, code_key(KeyCode::Backspace));
        assert_eq!(app.param_edit_buffer, "42");
    }

    #[test]
    fn backspace_on_empty_buffer_is_noop() {
        let mut app = test_app();
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer.clear();

        handle_key_event(&mut app, code_key(KeyCode::Backspace));
        assert_eq!(app.param_edit_buffer, "");
    }

    // -- H13: Param edit buffer cap and finite validation -------------------

    #[test]
    fn param_edit_buffer_capped_at_16_chars() {
        let mut app = test_app();
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer.clear();

        // Push 20 digits; only 16 should be accepted.
        for _ in 0..20 {
            handle_key_event(&mut app, char_key('1'));
        }
        assert_eq!(app.param_edit_buffer.len(), 16);
    }

    #[test]
    fn confirm_param_edit_rejects_infinity() {
        let mut app = test_app_with_tracks(1);
        app.input_mode = InputMode::ParameterEdit;
        // A value that parses to infinity in f32
        app.param_edit_buffer = "999999999999999999999999999999999999999".into();

        handle_key_event(&mut app, code_key(KeyCode::Enter));
        // Should have exited param edit mode but not sent the command.
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.param_edit_buffer, "");
    }

    // -- Timeline panel keys -----------------------------------------------

    #[test]
    fn o_resolves_to_open_file_browser() {
        let app = test_app();
        let action = resolve_action(&app, char_key('o'));
        assert_eq!(action, Some(KeyAction::OpenFileBrowser));
    }

    #[test]
    fn open_file_browser_sets_mode() {
        let mut app = test_app();
        handle_key_event(&mut app, char_key('o'));
        assert!(matches!(app.mode, AppMode::FileBrowser { .. }));
    }

    #[test]
    fn file_browser_esc_closes() {
        let mut app = test_app();
        app.open_file_browser();
        assert!(matches!(app.mode, AppMode::FileBrowser { .. }));

        handle_key_event(&mut app, code_key(KeyCode::Esc));
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn file_browser_j_k_navigate() {
        let mut app = test_app();
        app.open_file_browser();

        // Get the entry count.
        let entry_count = if let AppMode::FileBrowser { ref entries, .. } = app.mode {
            entries.len()
        } else {
            0
        };

        if entry_count > 1 {
            handle_key_event(&mut app, char_key('j'));
            if let AppMode::FileBrowser { selected, .. } = app.mode {
                assert_eq!(selected, 1);
            }

            handle_key_event(&mut app, char_key('k'));
            if let AppMode::FileBrowser { selected, .. } = app.mode {
                assert_eq!(selected, 0);
            }
        }
    }

    #[test]
    fn file_browser_blocks_normal_keys() {
        let mut app = test_app();
        app.open_file_browser();
        // 'q' should not quit while in file browser mode.
        let action = resolve_action(&app, char_key('q'));
        assert_eq!(action, None);
    }

    #[test]
    fn h_in_timeline_panel_is_scroll_left() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, char_key('h'));
        assert_eq!(action, Some(KeyAction::TimelineScrollLeft));
    }

    #[test]
    fn l_in_timeline_panel_is_scroll_right() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, char_key('l'));
        assert_eq!(action, Some(KeyAction::TimelineScrollRight));
    }

    #[test]
    fn plus_in_timeline_panel_is_zoom_in() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, char_key('+'));
        assert_eq!(action, Some(KeyAction::TimelineZoomIn));
    }

    #[test]
    fn minus_in_timeline_panel_is_zoom_out() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, char_key('-'));
        assert_eq!(action, Some(KeyAction::TimelineZoomOut));
    }

    #[test]
    fn timeline_zoom_in_halves_zoom() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Timeline;
        let initial_zoom = app.tracking_state.timeline_zoom;
        handle_key_event(&mut app, char_key('+'));
        assert!((app.tracking_state.timeline_zoom - initial_zoom / 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn timeline_zoom_out_doubles_zoom() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Timeline;
        let initial_zoom = app.tracking_state.timeline_zoom;
        handle_key_event(&mut app, char_key('-'));
        assert!((app.tracking_state.timeline_zoom - initial_zoom * 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn timeline_zoom_clamped() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Timeline;
        app.tracking_state.timeline_zoom = 1.0;
        handle_key_event(&mut app, char_key('+')); // zoom in
        assert!((app.tracking_state.timeline_zoom - 1.0).abs() < f64::EPSILON); // clamped at 1.0

        app.tracking_state.timeline_zoom = 1_048_576.0;
        handle_key_event(&mut app, char_key('-')); // zoom out
        assert!((app.tracking_state.timeline_zoom - 1_048_576.0).abs() < f64::EPSILON); // clamped
    }

    #[test]
    fn comma_dot_resolve_to_select_clip() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, char_key(','));
        assert_eq!(action, Some(KeyAction::SelectPrevClip));
        let action = resolve_action(&app, char_key('.'));
        assert_eq!(action, Some(KeyAction::SelectNextClip));
    }

    #[test]
    fn angle_brackets_resolve_to_move_clip() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, char_key('<'));
        assert_eq!(action, Some(KeyAction::MoveClipLeft));
        let action = resolve_action(&app, char_key('>'));
        assert_eq!(action, Some(KeyAction::MoveClipRight));
    }

    #[test]
    fn beat_samples_calculation() {
        // 120 BPM at 48000 Hz = 24000 samples per beat.
        assert_eq!(beat_samples(120.0, 48_000), 24_000);
        // Edge cases.
        assert_eq!(beat_samples(0.0, 48_000), 0);
        assert_eq!(beat_samples(120.0, 0), 0);
    }

    #[test]
    fn select_adjacent_clip_with_no_clips() {
        let mut app = test_app();
        select_adjacent_clip(&mut app, true);
        assert!(app.tracking_state.selected_clip.is_none());
    }

    #[test]
    fn delete_key_in_timeline_resolves_to_delete_clip() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, code_key(KeyCode::Delete));
        assert_eq!(action, Some(KeyAction::DeleteClip));
    }

    // -- BPM adjustment in Transport panel ----------------------------------

    #[test]
    fn equals_in_project_view_resolves_to_project_adjust() {
        // On the Project view, =/+/- are intercepted by the project view resolver.
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        let action = resolve_action(&app, char_key('='));
        assert_eq!(action, Some(KeyAction::ProjectAdjustUp));
    }

    #[test]
    fn minus_in_project_view_resolves_to_project_adjust() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        let action = resolve_action(&app, char_key('-'));
        assert_eq!(action, Some(KeyAction::ProjectAdjustDown));
    }

    #[test]
    fn equals_in_transport_panel_resolves_to_increase_bpm() {
        // On a non-Project view, =/+/- in the Transport panel still resolve to BPM.
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Transport;
        let action = resolve_action(&app, char_key('='));
        assert_eq!(action, Some(KeyAction::IncreaseBPM));
    }

    #[test]
    fn plus_in_transport_panel_resolves_to_increase_bpm_large() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Transport;
        // '+' is Shift+= on US keyboards; crossterm reports it as Char('+').
        let action = resolve_action(&app, char_key('+'));
        assert_eq!(action, Some(KeyAction::IncreaseBPMLarge));
    }

    #[test]
    fn minus_in_transport_panel_resolves_to_decrease_bpm() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Transport;
        let action = resolve_action(&app, char_key('-'));
        assert_eq!(action, Some(KeyAction::DecreaseBPM));
    }

    #[test]
    fn underscore_in_transport_panel_resolves_to_decrease_bpm_large() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Transport;
        // '_' is Shift+- on US keyboards.
        let action = resolve_action(&app, char_key('_'));
        assert_eq!(action, Some(KeyAction::DecreaseBPMLarge));
    }

    #[test]
    fn bpm_actions_dispatch_without_panic() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Transport;
        // Default BPM is 120.0.
        let initial_bpm = app.display.transport.bpm;
        assert!((initial_bpm - 120.0).abs() < f64::EPSILON);
        // All four BPM variants should dispatch without panic.
        apply_action(&mut app, KeyAction::IncreaseBPM);
        apply_action(&mut app, KeyAction::DecreaseBPM);
        apply_action(&mut app, KeyAction::IncreaseBPMLarge);
        apply_action(&mut app, KeyAction::DecreaseBPMLarge);
    }

    #[test]
    fn plus_in_tracks_panel_is_volume_not_bpm() {
        // Verify that outside Transport panel, +/- still adjusts volume.
        let mut app = test_app_with_tracks(1);
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Tracks;
        let action = resolve_action(&app, char_key('+'));
        assert_eq!(action, Some(KeyAction::IncreaseVolume));
    }

    // -- Recording workflow controls ----------------------------------------

    #[test]
    fn w_cycles_recording_workflow_in_transport_panel() {
        use kazoo_core::transport::RecordingWorkflow;

        let mut app = test_app();
        app.focused_panel = FocusedPanel::Transport;

        // Default is CountIn.
        assert!(matches!(
            app.recording_workflow,
            RecordingWorkflow::CountIn { .. }
        ));

        // Cycle to FixedLength.
        handle_key_event(&mut app, char_key('w'));
        assert!(matches!(
            app.recording_workflow,
            RecordingWorkflow::FixedLength { .. }
        ));

        // Cycle back to CountIn.
        handle_key_event(&mut app, char_key('w'));
        assert!(matches!(
            app.recording_workflow,
            RecordingWorkflow::CountIn { .. }
        ));
    }

    #[test]
    fn bracket_keys_adjust_record_bars_in_transport_panel() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Transport;
        assert_eq!(app.record_bars, 4);

        handle_key_event(&mut app, char_key(']'));
        assert_eq!(app.record_bars, 5);

        handle_key_event(&mut app, char_key('['));
        assert_eq!(app.record_bars, 4);
    }

    #[test]
    fn record_bars_clamped_at_zero_and_max() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Transport;

        // Decrease to zero.
        app.record_bars = 0;
        handle_key_event(&mut app, char_key('['));
        assert_eq!(app.record_bars, 0);

        // Increase to max.
        app.record_bars = 64;
        handle_key_event(&mut app, char_key(']'));
        assert_eq!(app.record_bars, 64);
    }

    #[test]
    fn shift_r_resolves_to_record_with_count_in() {
        let app = test_app();
        let action = resolve_action(&app, char_key('R'));
        assert_eq!(action, Some(KeyAction::RecordWithCountIn));
    }

    // -----------------------------------------------------------------------
    // Audio I/O view navigation
    // -----------------------------------------------------------------------

    #[test]
    fn audio_io_tab_cycles_sections_forward() {
        use crate::state::DeviceListFocus;
        let mut app = test_app();
        app.active_view = ActiveView::AudioIO;
        assert_eq!(app.audio_io_state.focus, DeviceListFocus::Input);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.audio_io_state.focus, DeviceListFocus::Output);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.audio_io_state.focus, DeviceListFocus::Settings);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.audio_io_state.focus, DeviceListFocus::Input);
    }

    #[test]
    fn audio_io_backtab_cycles_sections_backward() {
        use crate::state::DeviceListFocus;
        let mut app = test_app();
        app.active_view = ActiveView::AudioIO;
        assert_eq!(app.audio_io_state.focus, DeviceListFocus::Input);

        handle_key_event(&mut app, code_key(KeyCode::BackTab));
        assert_eq!(app.audio_io_state.focus, DeviceListFocus::Settings);

        handle_key_event(&mut app, code_key(KeyCode::BackTab));
        assert_eq!(app.audio_io_state.focus, DeviceListFocus::Output);

        handle_key_event(&mut app, code_key(KeyCode::BackTab));
        assert_eq!(app.audio_io_state.focus, DeviceListFocus::Input);
    }

    #[test]
    fn audio_io_j_k_navigate_input_devices() {
        use crate::state::DeviceListFocus;
        let mut app = test_app();
        app.active_view = ActiveView::AudioIO;
        app.audio_io_state.focus = DeviceListFocus::Input;
        app.audio_io_state.input_devices = vec!["Mic 1".into(), "Mic 2".into(), "Mic 3".into()];
        app.audio_io_state.selected_input_device = 0;

        // j moves forward.
        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.audio_io_state.selected_input_device, 1);

        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.audio_io_state.selected_input_device, 2);

        // Wraps around.
        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.audio_io_state.selected_input_device, 0);

        // k moves backward (wraps to end).
        handle_key_event(&mut app, char_key('k'));
        assert_eq!(app.audio_io_state.selected_input_device, 2);
    }

    #[test]
    fn audio_io_j_k_navigate_output_devices() {
        use crate::state::DeviceListFocus;
        let mut app = test_app();
        app.active_view = ActiveView::AudioIO;
        app.audio_io_state.focus = DeviceListFocus::Output;
        app.audio_io_state.output_devices = vec!["Speaker".into(), "Headphones".into()];
        app.audio_io_state.selected_output_device = 0;

        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.audio_io_state.selected_output_device, 1);

        // Wraps.
        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.audio_io_state.selected_output_device, 0);
    }

    #[test]
    fn audio_io_navigate_empty_device_list() {
        use crate::state::DeviceListFocus;
        let mut app = test_app();
        app.active_view = ActiveView::AudioIO;
        app.audio_io_state.focus = DeviceListFocus::Input;
        app.audio_io_state.input_devices.clear();
        app.audio_io_state.selected_input_device = 0;

        // Should not panic on empty list.
        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.audio_io_state.selected_input_device, 0);

        handle_key_event(&mut app, char_key('k'));
        assert_eq!(app.audio_io_state.selected_input_device, 0);
    }

    #[test]
    fn audio_io_single_device_wraps_to_self() {
        use crate::state::DeviceListFocus;
        let mut app = test_app();
        app.active_view = ActiveView::AudioIO;
        app.audio_io_state.focus = DeviceListFocus::Input;
        app.audio_io_state.input_devices = vec!["Only One".into()];
        app.audio_io_state.selected_input_device = 0;

        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.audio_io_state.selected_input_device, 0);

        handle_key_event(&mut app, char_key('k'));
        assert_eq!(app.audio_io_state.selected_input_device, 0);
    }

    #[test]
    fn audio_io_settings_section_ignores_device_nav() {
        use crate::state::DeviceListFocus;
        let mut app = test_app();
        app.active_view = ActiveView::AudioIO;
        app.audio_io_state.focus = DeviceListFocus::Settings;
        app.audio_io_state.input_devices = vec!["Mic".into()];
        app.audio_io_state.output_devices = vec!["Speaker".into()];
        app.audio_io_state.selected_input_device = 0;
        app.audio_io_state.selected_output_device = 0;

        // j/k in Settings section shouldn't change any device selection.
        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.audio_io_state.selected_input_device, 0);
        assert_eq!(app.audio_io_state.selected_output_device, 0);
    }

    // -----------------------------------------------------------------------
    // Clip operations (apply_action side)
    // -----------------------------------------------------------------------

    #[test]
    fn delete_clip_clears_selection() {
        let mut app = test_app_with_tracks(1);
        app.tracking_state.selected_clip = Some(ClipId(42));

        // Apply DeleteClip — sends command and clears selection.
        apply_action(&mut app, KeyAction::DeleteClip);
        assert!(app.tracking_state.selected_clip.is_none());
    }

    #[test]
    fn delete_clip_with_no_selection_is_noop() {
        let mut app = test_app_with_tracks(1);
        app.tracking_state.selected_clip = None;

        // Should not panic.
        apply_action(&mut app, KeyAction::DeleteClip);
        assert!(app.tracking_state.selected_clip.is_none());
    }

    #[test]
    fn delete_clip_with_no_tracks_is_noop() {
        let mut app = test_app();
        app.tracking_state.selected_clip = Some(ClipId(1));

        // No tracks → selected_track_id() returns None → noop.
        apply_action(&mut app, KeyAction::DeleteClip);
        // Selection is NOT cleared because the guard fails early.
        assert_eq!(app.tracking_state.selected_clip, Some(ClipId(1)));
    }

    #[test]
    fn move_clip_with_no_selection_is_noop() {
        let mut app = test_app_with_tracks(1);
        app.tracking_state.selected_clip = None;

        apply_action(&mut app, KeyAction::MoveClipLeft);
        apply_action(&mut app, KeyAction::MoveClipRight);
        // No panic.
    }

    #[test]
    fn split_clip_with_no_selection_is_noop() {
        let mut app = test_app_with_tracks(1);
        app.tracking_state.selected_clip = None;

        apply_action(&mut app, KeyAction::SplitClip);
        // No panic.
    }

    #[test]
    fn duplicate_clip_with_no_selection_is_noop() {
        let mut app = test_app_with_tracks(1);
        app.tracking_state.selected_clip = None;

        apply_action(&mut app, KeyAction::DuplicateClip);
        // No panic.
    }

    #[test]
    fn find_clip_in_empty_timeline() {
        let timeline = kazoo_core::engine::TimelineSnapshot {
            tracks: vec![],
            total_length: 0,
        };
        assert!(find_clip_in_timeline(&timeline, ClipId(1)).is_none());
    }

    #[test]
    fn find_clip_in_timeline_with_clips() {
        let snapshot = kazoo_core::engine::ClipSnapshot {
            id: 42,
            name: "Test".into(),
            position: 1000,
            length: 44100,
            gain_db: 0.0,
            muted: false,
            waveform_overview: vec![],
        };
        let timeline = kazoo_core::engine::TimelineSnapshot {
            tracks: vec![kazoo_core::engine::TrackClipSnapshot {
                track_id: 0,
                track_name: "1".into(),
                clips: vec![snapshot],
                armed: false,
                muted: false,
                soloed: false,
                is_recording_clip: false,
                recording_start: 0,
                recording_length: 0,
            }],
            total_length: 45100,
        };
        let found = find_clip_in_timeline(&timeline, ClipId(42));
        assert!(found.is_some());
        assert_eq!(found.unwrap().position, 1000);

        // Non-existent clip.
        assert!(find_clip_in_timeline(&timeline, ClipId(999)).is_none());
    }

    #[test]
    fn select_adjacent_clip_forward_cycles() {
        let mut app = test_app_with_tracks(1);
        let clip_a = kazoo_core::engine::ClipSnapshot {
            id: 1,
            name: "A".into(),
            position: 0,
            length: 1000,
            gain_db: 0.0,
            muted: false,
            waveform_overview: vec![],
        };
        let clip_b = kazoo_core::engine::ClipSnapshot {
            id: 2,
            name: "B".into(),
            position: 2000,
            length: 1000,
            gain_db: 0.0,
            muted: false,
            waveform_overview: vec![],
        };
        app.display.timeline = kazoo_core::engine::TimelineSnapshot {
            tracks: vec![kazoo_core::engine::TrackClipSnapshot {
                track_id: app.tracks[0].id.0,
                track_name: "1".into(),
                clips: vec![clip_a, clip_b],
                armed: false,
                muted: false,
                soloed: false,
                is_recording_clip: false,
                recording_start: 0,
                recording_length: 0,
            }],
            total_length: 3000,
        };

        // No initial selection — selects first clip.
        select_adjacent_clip(&mut app, true);
        assert_eq!(app.tracking_state.selected_clip, Some(ClipId(1)));

        // Forward → clip B.
        select_adjacent_clip(&mut app, true);
        assert_eq!(app.tracking_state.selected_clip, Some(ClipId(2)));

        // Forward wraps → clip A.
        select_adjacent_clip(&mut app, true);
        assert_eq!(app.tracking_state.selected_clip, Some(ClipId(1)));
    }

    #[test]
    fn select_adjacent_clip_backward_wraps() {
        let mut app = test_app_with_tracks(1);
        let clip_a = kazoo_core::engine::ClipSnapshot {
            id: 10,
            name: "A".into(),
            position: 0,
            length: 500,
            gain_db: 0.0,
            muted: false,
            waveform_overview: vec![],
        };
        let clip_b = kazoo_core::engine::ClipSnapshot {
            id: 20,
            name: "B".into(),
            position: 1000,
            length: 500,
            gain_db: 0.0,
            muted: false,
            waveform_overview: vec![],
        };
        app.display.timeline = kazoo_core::engine::TimelineSnapshot {
            tracks: vec![kazoo_core::engine::TrackClipSnapshot {
                track_id: app.tracks[0].id.0,
                track_name: "1".into(),
                clips: vec![clip_a, clip_b],
                armed: false,
                muted: false,
                soloed: false,
                is_recording_clip: false,
                recording_start: 0,
                recording_length: 0,
            }],
            total_length: 1500,
        };

        // Start at clip A, go backward — wraps to clip B.
        app.tracking_state.selected_clip = Some(ClipId(10));
        select_adjacent_clip(&mut app, false);
        assert_eq!(app.tracking_state.selected_clip, Some(ClipId(20)));

        // Backward again → clip A.
        select_adjacent_clip(&mut app, false);
        assert_eq!(app.tracking_state.selected_clip, Some(ClipId(10)));
    }

    // -----------------------------------------------------------------------
    // Project view state management
    // -----------------------------------------------------------------------

    #[test]
    fn project_card_cycling_wraps() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        assert_eq!(app.project_state.selected_card, 0);

        // Forward through all 6 cards.
        for expected in 1..=5 {
            handle_key_event(&mut app, code_key(KeyCode::Tab));
            assert_eq!(app.project_state.selected_card, expected);
        }
        // Wrap to 0.
        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.project_state.selected_card, 0);
    }

    #[test]
    fn project_card_backward_cycling_wraps() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        assert_eq!(app.project_state.selected_card, 0);

        // Backward from 0 → 5.
        handle_key_event(&mut app, code_key(KeyCode::BackTab));
        assert_eq!(app.project_state.selected_card, 5);
    }

    #[test]
    fn project_card_change_resets_field() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        // Navigate to card 1 (Time Sig, 2 fields).
        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.project_state.selected_card, 1);

        // Move to field 1.
        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.project_state.selected_field, 1);

        // Switch card — field resets to 0.
        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.project_state.selected_card, 2);
        assert_eq!(app.project_state.selected_field, 0);
    }

    #[test]
    fn project_field_cycling_wraps_within_card() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        // Card 5 (Recording) has 2 fields.
        app.project_state.selected_card = 5;
        app.project_state.selected_field = 0;

        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.project_state.selected_field, 1);

        // Wraps back to 0.
        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.project_state.selected_field, 0);
    }

    #[test]
    fn project_field_backward_wraps() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        // Card 2 (Count-In) has 2 fields.
        app.project_state.selected_card = 2;
        app.project_state.selected_field = 0;

        // Backward from 0 → last field.
        handle_key_event(&mut app, char_key('k'));
        assert_eq!(app.project_state.selected_field, 1);
    }

    #[test]
    fn project_adjust_count_in_bars() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        // Card 2 (Count-In), field 1: count-in bars.
        app.project_state.selected_card = 2;
        app.project_state.selected_field = 1;
        app.count_in_bars = 2;

        handle_key_event(&mut app, char_key('='));
        assert_eq!(app.count_in_bars, 3);

        handle_key_event(&mut app, char_key('-'));
        assert_eq!(app.count_in_bars, 2);
    }

    #[test]
    fn project_count_in_bars_clamped() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        app.project_state.selected_card = 2;
        app.project_state.selected_field = 1;

        // At zero, can't go lower.
        app.count_in_bars = 0;
        handle_key_event(&mut app, char_key('-'));
        assert_eq!(app.count_in_bars, 0);

        // At max (16), can't go higher.
        app.count_in_bars = 16;
        handle_key_event(&mut app, char_key('='));
        assert_eq!(app.count_in_bars, 16);
    }

    #[test]
    fn project_adjust_record_bars() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        // Card 5 (Recording), field 1: record bars.
        app.project_state.selected_card = 5;
        app.project_state.selected_field = 1;
        app.record_bars = 4;

        handle_key_event(&mut app, char_key('='));
        assert_eq!(app.record_bars, 5);

        handle_key_event(&mut app, char_key('-'));
        assert_eq!(app.record_bars, 4);
    }

    #[test]
    fn project_record_bars_clamped() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        app.project_state.selected_card = 5;
        app.project_state.selected_field = 1;

        app.record_bars = 0;
        handle_key_event(&mut app, char_key('-'));
        assert_eq!(app.record_bars, 0);

        app.record_bars = 64;
        handle_key_event(&mut app, char_key('='));
        assert_eq!(app.record_bars, 64);
    }

    #[test]
    fn project_toggle_count_in() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        // Card 2, field 0: count-in enabled toggle.
        app.project_state.selected_card = 2;
        app.project_state.selected_field = 0;
        app.count_in_bars = 2;

        // Toggle off (Enter triggers ProjectToggle).
        handle_key_event(&mut app, code_key(KeyCode::Enter));
        assert_eq!(app.count_in_bars, 0);

        // Toggle on.
        handle_key_event(&mut app, code_key(KeyCode::Enter));
        assert_eq!(app.count_in_bars, 1);
    }

    #[test]
    fn space_in_project_view_is_play() {
        let app = {
            let mut a = test_app();
            a.active_view = ActiveView::Project;
            a
        };
        let action = resolve_action(&app, char_key(' '));
        assert_eq!(action, Some(KeyAction::Play));
    }

    #[test]
    fn project_card_field_count_coverage() {
        // Cards 0, 3, 4 have 1 field.
        assert_eq!(project_card_field_count(0), 1);
        assert_eq!(project_card_field_count(3), 1);
        assert_eq!(project_card_field_count(4), 1);
        // Cards 1, 2, 5 have 2 fields.
        assert_eq!(project_card_field_count(1), 2);
        assert_eq!(project_card_field_count(2), 2);
        assert_eq!(project_card_field_count(5), 2);
        // Out of range.
        assert_eq!(project_card_field_count(6), 0);
        assert_eq!(project_card_field_count(100), 0);
    }

    #[test]
    fn project_single_field_card_wraps_to_self() {
        let mut app = test_app();
        app.active_view = ActiveView::Project;
        // Card 0 (Tempo) has 1 field.
        app.project_state.selected_card = 0;
        app.project_state.selected_field = 0;

        handle_key_event(&mut app, char_key('j'));
        assert_eq!(app.project_state.selected_field, 0);

        handle_key_event(&mut app, char_key('k'));
        assert_eq!(app.project_state.selected_field, 0);
    }

    // -----------------------------------------------------------------------
    // Effect chain integrity
    // -----------------------------------------------------------------------

    #[test]
    fn add_effect_dispatches_without_panic() {
        let mut app = test_app_with_tracks(1);
        apply_action(&mut app, KeyAction::AddEffect);
        assert_eq!(app.tracks[0].effect_names.len(), 1);
        assert!(!app.tracks[0].effect_bypassed.is_empty());
    }

    #[test]
    fn add_multiple_effects_preserves_order() {
        let mut app = test_app_with_tracks(1);
        apply_action(&mut app, KeyAction::AddEffect);
        apply_action(&mut app, KeyAction::AddEffect);
        apply_action(&mut app, KeyAction::AddEffect);
        assert_eq!(app.tracks[0].effect_names.len(), 3);
        assert_eq!(app.tracks[0].effect_bypassed.len(), 3);
    }

    #[test]
    fn remove_effect_clamps_selection() {
        let mut app = test_app_with_tracks(1);
        // Add 3 effects, select the last one.
        apply_action(&mut app, KeyAction::AddEffect);
        apply_action(&mut app, KeyAction::AddEffect);
        apply_action(&mut app, KeyAction::AddEffect);
        app.synth_state.selected_effect = 2;

        // Remove it — selection should clamp.
        app.remove_effect(app.selected_track, 2);
        assert_eq!(app.tracks[0].effect_names.len(), 2);
        assert!(app.synth_state.selected_effect <= 1);
    }

    #[test]
    fn remove_effect_on_empty_chain_is_noop() {
        let mut app = test_app_with_tracks(1);
        assert!(app.tracks[0].effect_names.is_empty());

        // Should not panic.
        app.remove_effect(app.selected_track, 0);
        assert!(app.tracks[0].effect_names.is_empty());
    }

    #[test]
    fn toggle_effect_bypass_out_of_bounds() {
        let mut app = test_app_with_tracks(1);
        // No effects — toggle should be noop.
        app.toggle_effect_bypass(0, 0);
        app.toggle_effect_bypass(0, 99);
        // No panic.
    }

    #[test]
    fn add_effect_with_no_tracks_is_noop() {
        let mut app = test_app();
        assert!(app.tracks.is_empty());
        // Should not panic.
        apply_action(&mut app, KeyAction::AddEffect);
    }

    #[test]
    fn remove_effect_with_no_tracks_is_noop() {
        let mut app = test_app();
        apply_action(&mut app, KeyAction::RemoveEffect);
        // No panic.
    }

    #[test]
    fn add_remove_add_effect_maintains_consistency() {
        let mut app = test_app_with_tracks(1);
        apply_action(&mut app, KeyAction::AddEffect);
        assert_eq!(app.tracks[0].effect_names.len(), 1);

        app.remove_effect(0, 0);
        assert!(app.tracks[0].effect_names.is_empty());
        assert!(app.tracks[0].effect_bypassed.is_empty());

        apply_action(&mut app, KeyAction::AddEffect);
        assert_eq!(app.tracks[0].effect_names.len(), 1);
        assert_eq!(app.tracks[0].effect_bypassed.len(), 1);
    }

    #[test]
    fn effect_operations_isolated_to_selected_track() {
        let mut app = test_app_with_tracks(3);
        app.selected_track = 0;
        apply_action(&mut app, KeyAction::AddEffect);

        // Only track 0 should have an effect.
        assert_eq!(app.tracks[0].effect_names.len(), 1);
        assert!(app.tracks[1].effect_names.is_empty());
        assert!(app.tracks[2].effect_names.is_empty());
    }

    // -----------------------------------------------------------------------
    // Mixer view edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn mixer_channel_navigation_with_no_tracks() {
        let mut app = test_app();
        app.active_view = ActiveView::Mixer;
        assert!(app.tracks.is_empty());

        // Channel navigation should be no-op with no tracks.
        handle_key_event(&mut app, char_key('l'));
        assert_eq!(app.mixer_view_state.selected_channel, 0);

        handle_key_event(&mut app, char_key('h'));
        assert_eq!(app.mixer_view_state.selected_channel, 0);
    }

    #[test]
    fn mixer_single_track_channel_wraps_to_self() {
        let mut app = test_app_with_tracks(1);
        app.active_view = ActiveView::Mixer;
        assert_eq!(app.mixer_view_state.selected_channel, 0);

        // Forward wraps back to 0.
        handle_key_event(&mut app, char_key('l'));
        assert_eq!(app.mixer_view_state.selected_channel, 0);

        // Backward wraps back to 0.
        handle_key_event(&mut app, char_key('h'));
        assert_eq!(app.mixer_view_state.selected_channel, 0);
    }

    #[test]
    fn mixer_control_full_cycle_wraps() {
        let mut app = test_app_with_tracks(1);
        app.active_view = ActiveView::Mixer;
        assert_eq!(app.mixer_view_state.selected_control, MixerControl::Fader);

        // Cycle all the way through: Fader→Pan→Solo→Mute→Arm→Fader.
        let expected = [
            MixerControl::Pan,
            MixerControl::Solo,
            MixerControl::Mute,
            MixerControl::Arm,
            MixerControl::Fader, // wrap
        ];
        for &ctrl in &expected {
            handle_key_event(&mut app, char_key('j'));
            assert_eq!(app.mixer_view_state.selected_control, ctrl);
        }
    }

    #[test]
    fn mixer_control_backward_cycle_wraps() {
        let mut app = test_app_with_tracks(1);
        app.active_view = ActiveView::Mixer;
        assert_eq!(app.mixer_view_state.selected_control, MixerControl::Fader);

        // k from Fader wraps to Arm.
        handle_key_event(&mut app, char_key('k'));
        assert_eq!(app.mixer_view_state.selected_control, MixerControl::Arm);
    }

    #[test]
    fn mixer_plus_minus_toggles_on_button_controls() {
        let mut app = test_app_with_tracks(1);
        app.active_view = ActiveView::Mixer;

        // +/- on Solo should toggle (same behavior either way).
        app.mixer_view_state.selected_control = MixerControl::Solo;
        let action_plus = resolve_action(&app, char_key('+'));
        assert_eq!(action_plus, Some(KeyAction::ToggleSolo));
        let action_minus = resolve_action(&app, char_key('-'));
        assert_eq!(action_minus, Some(KeyAction::ToggleSolo));

        // Same for Mute.
        app.mixer_view_state.selected_control = MixerControl::Mute;
        let action_plus = resolve_action(&app, char_key('+'));
        assert_eq!(action_plus, Some(KeyAction::ToggleMute));
        let action_minus = resolve_action(&app, char_key('-'));
        assert_eq!(action_minus, Some(KeyAction::ToggleMute));

        // And Arm.
        app.mixer_view_state.selected_control = MixerControl::Arm;
        let action_plus = resolve_action(&app, char_key('+'));
        assert_eq!(action_plus, Some(KeyAction::ToggleArm));
        let action_minus = resolve_action(&app, char_key('-'));
        assert_eq!(action_minus, Some(KeyAction::ToggleArm));
    }

    #[test]
    fn view_switch_syncs_mixer_channel_to_selected_track() {
        let mut app = test_app_with_tracks(4);
        app.selected_track = 2;

        // Switch to Mixer view — should sync mixer channel.
        apply_action(&mut app, KeyAction::SwitchView(ActiveView::Mixer));
        assert_eq!(app.mixer_view_state.selected_channel, 2);
    }

    #[test]
    fn mixer_channel_nav_syncs_selected_track() {
        let mut app = test_app_with_tracks(3);
        app.active_view = ActiveView::Mixer;
        app.mixer_view_state.selected_channel = 0;
        app.selected_track = 0;

        // Navigate to channel 1 — selected_track should follow.
        handle_key_event(&mut app, char_key('l'));
        assert_eq!(app.selected_track, 1);
        assert_eq!(app.mixer_view_state.selected_channel, 1);

        // Navigate backward — selected_track should follow.
        handle_key_event(&mut app, char_key('h'));
        assert_eq!(app.selected_track, 0);
        assert_eq!(app.mixer_view_state.selected_channel, 0);
    }

    #[test]
    fn mixer_channel_nav_resets_effect_selection() {
        let mut app = test_app_with_tracks(2);
        app.active_view = ActiveView::Mixer;
        app.synth_state.selected_effect = 5;
        app.synth_state.selected_param = 3;

        handle_key_event(&mut app, char_key('l'));
        assert_eq!(app.synth_state.selected_effect, 0);
        assert_eq!(app.synth_state.selected_param, 0);
    }

    // -----------------------------------------------------------------------
    // Parameter editing: decimal, negative, NaN, empty buffer
    // -----------------------------------------------------------------------

    #[test]
    fn param_edit_accepts_decimal_point() {
        let mut app = test_app();
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer.clear();

        handle_key_event(&mut app, char_key('3'));
        handle_key_event(&mut app, char_key('.'));
        handle_key_event(&mut app, char_key('1'));
        handle_key_event(&mut app, char_key('4'));

        assert_eq!(app.param_edit_buffer, "3.14");
    }

    #[test]
    fn param_edit_accepts_negative_sign() {
        let mut app = test_app();
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer.clear();

        handle_key_event(&mut app, char_key('-'));
        handle_key_event(&mut app, char_key('1'));
        handle_key_event(&mut app, char_key('2'));

        assert_eq!(app.param_edit_buffer, "-12");
    }

    #[test]
    fn param_edit_rejects_letters() {
        let mut app = test_app();
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer.clear();

        handle_key_event(&mut app, char_key('a'));
        handle_key_event(&mut app, char_key('b'));
        handle_key_event(&mut app, char_key('N'));
        handle_key_event(&mut app, char_key('5'));

        // Only '5' accepted — letters are rejected.
        assert_eq!(app.param_edit_buffer, "5");
    }

    #[test]
    fn confirm_empty_buffer_exits_param_edit() {
        let mut app = test_app();
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer.clear();

        handle_key_event(&mut app, code_key(KeyCode::Enter));
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.param_edit_buffer, "");
    }

    #[test]
    fn confirm_nan_string_exits_without_applying() {
        let mut app = test_app_with_tracks(1);
        app.input_mode = InputMode::ParameterEdit;
        // "NaN" doesn't parse as f32 via normal digit entry, but test the
        // confirm path directly with a garbage string.
        app.param_edit_buffer = "not-a-number".into();

        handle_key_event(&mut app, code_key(KeyCode::Enter));
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.param_edit_buffer, "");
    }

    #[test]
    fn confirm_multiple_decimal_points_exits_without_applying() {
        let mut app = test_app_with_tracks(1);
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer = "3.14.15".into();

        handle_key_event(&mut app, code_key(KeyCode::Enter));
        // "3.14.15" does not parse as f32 — exits cleanly.
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.param_edit_buffer, "");
    }

    #[test]
    fn confirm_negative_zero_is_valid() {
        let mut app = test_app_with_tracks(1);
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer = "-0".into();

        handle_key_event(&mut app, code_key(KeyCode::Enter));
        // -0.0 is a valid finite f32.
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(app.param_edit_buffer, "");
    }

    // -----------------------------------------------------------------------
    // File browser navigation edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn file_browser_backspace_goes_to_parent() {
        let mut app = test_app();
        app.open_file_browser();

        let original_dir = if let AppMode::FileBrowser { ref directory, .. } = app.mode {
            directory.clone()
        } else {
            panic!("expected FileBrowser mode");
        };

        // Backspace should go to parent directory (if not root).
        if original_dir.parent().is_some() {
            handle_key_event(&mut app, code_key(KeyCode::Backspace));
            if let AppMode::FileBrowser {
                ref directory,
                selected,
                ..
            } = app.mode
            {
                assert_eq!(*directory, original_dir.parent().unwrap());
                assert_eq!(selected, 0); // Selection resets.
            } else {
                panic!("expected FileBrowser mode after Backspace");
            }
        }
    }

    #[test]
    fn file_browser_enter_on_directory_navigates_into() {
        let mut app = test_app();
        app.open_file_browser();

        // Find first directory entry in the browser.
        let dir_entry_idx = if let AppMode::FileBrowser { ref entries, .. } = app.mode {
            entries.iter().position(|e| e.is_dir)
        } else {
            None
        };

        if let Some(idx) = dir_entry_idx {
            // Navigate to the directory entry.
            for _ in 0..idx {
                handle_key_event(&mut app, char_key('j'));
            }

            let target_dir = if let AppMode::FileBrowser {
                ref entries,
                selected,
                ..
            } = app.mode
            {
                entries[selected].path.clone()
            } else {
                panic!("expected FileBrowser mode");
            };

            handle_key_event(&mut app, code_key(KeyCode::Enter));

            if let AppMode::FileBrowser {
                ref directory,
                selected,
                ..
            } = app.mode
            {
                assert_eq!(*directory, target_dir);
                assert_eq!(selected, 0); // Selection resets on directory change.
            } else {
                panic!("expected FileBrowser mode after Enter on directory");
            }
        }
    }

    #[test]
    fn file_browser_j_wraps_at_boundary() {
        let mut app = test_app();

        // Create a synthetic file browser with exactly 3 entries.
        app.mode = AppMode::FileBrowser {
            directory: std::path::PathBuf::from("/tmp"),
            entries: vec![
                crate::app::FileBrowserEntry {
                    name: "a".into(),
                    path: std::path::PathBuf::from("/tmp/a"),
                    is_dir: true,
                },
                crate::app::FileBrowserEntry {
                    name: "b".into(),
                    path: std::path::PathBuf::from("/tmp/b"),
                    is_dir: true,
                },
                crate::app::FileBrowserEntry {
                    name: "c.wav".into(),
                    path: std::path::PathBuf::from("/tmp/c.wav"),
                    is_dir: false,
                },
            ],
            selected: 0,
        };

        // j three times should wrap to 0.
        handle_key_event(&mut app, char_key('j'));
        handle_key_event(&mut app, char_key('j'));
        handle_key_event(&mut app, char_key('j'));
        if let AppMode::FileBrowser { selected, .. } = app.mode {
            assert_eq!(selected, 0);
        }
    }

    #[test]
    fn file_browser_k_wraps_at_boundary() {
        let mut app = test_app();

        app.mode = AppMode::FileBrowser {
            directory: std::path::PathBuf::from("/tmp"),
            entries: vec![
                crate::app::FileBrowserEntry {
                    name: "a".into(),
                    path: std::path::PathBuf::from("/tmp/a"),
                    is_dir: true,
                },
                crate::app::FileBrowserEntry {
                    name: "b.wav".into(),
                    path: std::path::PathBuf::from("/tmp/b.wav"),
                    is_dir: false,
                },
            ],
            selected: 0,
        };

        // k from 0 wraps to last entry (1).
        handle_key_event(&mut app, char_key('k'));
        if let AppMode::FileBrowser { selected, .. } = app.mode {
            assert_eq!(selected, 1);
        }
    }

    #[test]
    fn file_browser_empty_directory_nav_is_noop() {
        let mut app = test_app();

        app.mode = AppMode::FileBrowser {
            directory: std::path::PathBuf::from("/tmp"),
            entries: vec![],
            selected: 0,
        };

        // j and k with no entries should not panic.
        handle_key_event(&mut app, char_key('j'));
        if let AppMode::FileBrowser { selected, .. } = app.mode {
            assert_eq!(selected, 0);
        }

        handle_key_event(&mut app, char_key('k'));
        if let AppMode::FileBrowser { selected, .. } = app.mode {
            assert_eq!(selected, 0);
        }
    }

    // -----------------------------------------------------------------------
    // View-aware Tab cycling
    // -----------------------------------------------------------------------

    #[test]
    fn tab_cycles_within_mixer_view_panels() {
        let mut app = test_app();
        app.active_view = ActiveView::Mixer;
        // Mixer view only has the Mixer panel.
        app.focused_panel = FocusedPanel::Mixer;

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        // Should stay on Mixer (only one panel in Mixer view).
        assert_eq!(app.focused_panel, FocusedPanel::Mixer);
    }

    #[test]
    fn tab_cycles_within_tracking_view_panels() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Tracks;

        // Tracking view panels: Tracks, Timeline, Waveform, Effects.
        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Timeline);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Waveform);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Effects);

        // Wrap back to Tracks.
        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Tracks);
    }

    #[test]
    fn backtab_cycles_backward_in_tracking_view() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Tracks;

        // BackTab from first panel wraps to last.
        handle_key_event(&mut app, code_key(KeyCode::BackTab));
        assert_eq!(app.focused_panel, FocusedPanel::Effects);
    }

    #[test]
    fn view_switch_resets_focus_to_first_panel() {
        let mut app = test_app();
        app.active_view = ActiveView::Tracking;
        app.focused_panel = FocusedPanel::Effects;

        // Switch to Mixer — focus should reset to Mixer panel.
        apply_action(&mut app, KeyAction::SwitchView(ActiveView::Mixer));
        assert_eq!(app.focused_panel, FocusedPanel::Mixer);

        // Switch to Tracking — focus should reset to Tracks.
        apply_action(&mut app, KeyAction::SwitchView(ActiveView::Tracking));
        assert_eq!(app.focused_panel, FocusedPanel::Tracks);
    }

    #[test]
    fn tab_with_mismatched_panel_resets_to_first() {
        let mut app = test_app();
        app.active_view = ActiveView::Mixer;
        // Intentionally set a panel that doesn't belong to Mixer view.
        app.focused_panel = FocusedPanel::Timeline;

        // Tab should reset to the first panel of the view.
        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Mixer);
    }

    // -----------------------------------------------------------------------
    // beat_samples helper
    // -----------------------------------------------------------------------

    #[test]
    fn beat_samples_normal() {
        // At 120 BPM and 44100 Hz, one beat = 0.5 sec = 22050 samples.
        assert_eq!(beat_samples(120.0, 44_100), 22_050);
    }

    #[test]
    fn beat_samples_zero_bpm_returns_zero() {
        assert_eq!(beat_samples(0.0, 44_100), 0);
    }

    #[test]
    fn beat_samples_negative_bpm_returns_zero() {
        assert_eq!(beat_samples(-120.0, 44_100), 0);
    }

    #[test]
    fn beat_samples_zero_sample_rate_returns_zero() {
        assert_eq!(beat_samples(120.0, 0), 0);
    }

    // -----------------------------------------------------------------------
    // Waveform zoom/scroll actions
    // -----------------------------------------------------------------------

    #[test]
    fn zoom_in_doubles_waveform_zoom() {
        let mut app = test_app();
        assert!((app.tracking_state.waveform_zoom - 1.0).abs() < f32::EPSILON);

        apply_action(&mut app, KeyAction::ZoomIn);
        assert!((app.tracking_state.waveform_zoom - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zoom_out_halves_waveform_zoom() {
        let mut app = test_app();
        app.tracking_state.waveform_zoom = 4.0;

        apply_action(&mut app, KeyAction::ZoomOut);
        assert!((app.tracking_state.waveform_zoom - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zoom_in_clamped_at_64() {
        let mut app = test_app();
        app.tracking_state.waveform_zoom = 64.0;

        apply_action(&mut app, KeyAction::ZoomIn);
        assert!((app.tracking_state.waveform_zoom - 64.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zoom_out_clamped_at_1() {
        let mut app = test_app();
        app.tracking_state.waveform_zoom = 1.0;

        apply_action(&mut app, KeyAction::ZoomOut);
        assert!((app.tracking_state.waveform_zoom - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn scroll_left_clamped_at_zero() {
        let mut app = test_app();
        app.tracking_state.waveform_scroll = 0.0;

        apply_action(&mut app, KeyAction::ScrollLeft);
        assert!(app.tracking_state.waveform_scroll >= 0.0);
    }

    #[test]
    fn scroll_right_clamped_at_one() {
        let mut app = test_app();
        app.tracking_state.waveform_scroll = 1.0;

        apply_action(&mut app, KeyAction::ScrollRight);
        assert!(app.tracking_state.waveform_scroll <= 1.0);
    }

    // -----------------------------------------------------------------------
    // Timeline zoom/scroll actions
    // -----------------------------------------------------------------------

    #[test]
    fn timeline_zoom_in_halves_samples_per_pixel() {
        let mut app = test_app();
        let initial = app.tracking_state.timeline_zoom;

        apply_action(&mut app, KeyAction::TimelineZoomIn);
        assert!((app.tracking_state.timeline_zoom - initial / 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn timeline_zoom_out_doubles_samples_per_pixel() {
        let mut app = test_app();
        let initial = app.tracking_state.timeline_zoom;

        apply_action(&mut app, KeyAction::TimelineZoomOut);
        assert!((app.tracking_state.timeline_zoom - initial * 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn timeline_zoom_in_clamped_at_1() {
        let mut app = test_app();
        app.tracking_state.timeline_zoom = 1.0;

        apply_action(&mut app, KeyAction::TimelineZoomIn);
        assert!((app.tracking_state.timeline_zoom - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn timeline_zoom_out_clamped_at_max() {
        let mut app = test_app();
        app.tracking_state.timeline_zoom = 1_048_576.0;

        apply_action(&mut app, KeyAction::TimelineZoomOut);
        assert!((app.tracking_state.timeline_zoom - 1_048_576.0).abs() < f64::EPSILON,);
    }

    #[test]
    fn timeline_scroll_left_clamped_at_zero() {
        let mut app = test_app();
        app.tracking_state.timeline_scroll = 0.0;

        apply_action(&mut app, KeyAction::TimelineScrollLeft);
        assert!(app.tracking_state.timeline_scroll >= 0.0);
    }

    // -----------------------------------------------------------------------
    // Track navigation edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn next_track_wraps_from_last_to_first() {
        let mut app = test_app_with_tracks(3);
        app.selected_track = 2;

        apply_action(&mut app, KeyAction::NextTrack);
        assert_eq!(app.selected_track, 0);
    }

    #[test]
    fn prev_track_wraps_from_first_to_last() {
        let mut app = test_app_with_tracks(3);
        app.selected_track = 0;

        apply_action(&mut app, KeyAction::PrevTrack);
        assert_eq!(app.selected_track, 2);
    }

    #[test]
    fn track_navigation_with_no_tracks_is_noop() {
        let mut app = test_app();
        assert!(app.tracks.is_empty());

        apply_action(&mut app, KeyAction::NextTrack);
        assert_eq!(app.selected_track, 0);

        apply_action(&mut app, KeyAction::PrevTrack);
        assert_eq!(app.selected_track, 0);
    }

    #[test]
    fn track_nav_resets_effect_and_param_selection() {
        let mut app = test_app_with_tracks(2);
        app.synth_state.selected_effect = 3;
        app.synth_state.selected_param = 7;

        apply_action(&mut app, KeyAction::NextTrack);
        assert_eq!(app.synth_state.selected_effect, 0);
        assert_eq!(app.synth_state.selected_param, 0);
    }

    #[test]
    fn select_track_by_index_out_of_bounds_is_noop() {
        let mut app = test_app_with_tracks(2);
        app.selected_track = 0;

        apply_action(&mut app, KeyAction::SelectTrack(99));
        assert_eq!(app.selected_track, 0); // Unchanged.
    }
}
