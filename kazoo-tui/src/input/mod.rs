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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    Record,
    RecordWithCountIn,
    ToggleLoop,
    ToggleMetronome,

    // Track selection
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
const fn resolve_panel_action(app: &App, key: KeyEvent) -> Option<KeyAction> {
    match app.focused_panel {
        FocusedPanel::Effects => resolve_effects_action(key),
        FocusedPanel::Waveform => resolve_waveform_action(key),
        FocusedPanel::Mixer => resolve_mixer_action(key),
        FocusedPanel::Timeline => resolve_timeline_action(key),
        FocusedPanel::Transport => resolve_transport_action(key),
        FocusedPanel::Tracks => resolve_default_panel_action(key),
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
const fn resolve_timeline_action(key: KeyEvent) -> Option<KeyAction> {
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
///
/// Every engine command's outcome is reported through [`App::status`]; local
/// state only changes when the engine accepted the command.
fn apply_action(app: &mut App, action: KeyAction) {
    match action {
        // -- Application lifecycle / views / focus ---------------------------
        KeyAction::Quit => app.should_quit = true,
        KeyAction::ToggleHelp => toggle_help(app),
        KeyAction::SwitchView(view) => switch_view(app, view),
        KeyAction::FocusEffects => app.focused_panel = FocusedPanel::Effects,
        KeyAction::FocusNext => cycle_focus(app, true),
        KeyAction::FocusPrev => cycle_focus(app, false),

        // -- Transport -------------------------------------------------------
        KeyAction::Play => toggle_play(app),
        KeyAction::Stop => send_transport(app, "Stop", TransportCommand::Stop),
        KeyAction::Record => send_transport(app, "Record", TransportCommand::Record),
        KeyAction::RecordWithCountIn => record_with_count_in(app),
        KeyAction::ToggleLoop => toggle_loop(app),
        KeyAction::ToggleMetronome => toggle_metronome(app),
        KeyAction::IncreaseBPM => nudge_tempo(app, 1.0),
        KeyAction::DecreaseBPM => nudge_tempo(app, -1.0),
        KeyAction::IncreaseBPMLarge => nudge_tempo(app, 10.0),
        KeyAction::DecreaseBPMLarge => nudge_tempo(app, -10.0),
        KeyAction::CycleRecordingWorkflow => cycle_recording_workflow(app),
        KeyAction::IncreaseRecordBars => adjust_record_bars(app, 1),
        KeyAction::DecreaseRecordBars => adjust_record_bars(app, -1),

        // -- Tracks ----------------------------------------------------------
        KeyAction::NextTrack => select_adjacent_track(app, true),
        KeyAction::PrevTrack => select_adjacent_track(app, false),
        KeyAction::ToggleMute => app.toggle_mute(app.selected_track),
        KeyAction::ToggleSolo => app.toggle_solo(app.selected_track),
        KeyAction::ToggleArm => app.toggle_arm(app.selected_track),
        KeyAction::AddTrack => add_numbered_track(app),
        KeyAction::RemoveTrack => app.remove_track(app.selected_track),
        KeyAction::CycleSynthMode => app.cycle_synth_mode(app.selected_track),

        // -- Effects ---------------------------------------------------------
        KeyAction::NextEffect => select_next_effect(app),
        KeyAction::PrevEffect => select_prev_effect(app),
        KeyAction::AddEffect => add_lowpass_effect(app),
        KeyAction::RemoveEffect => {
            app.remove_effect(app.selected_track, app.synth_state.selected_effect);
        }
        KeyAction::ToggleEffectBypass => {
            app.toggle_effect_bypass(app.selected_track, app.synth_state.selected_effect);
        }

        // -- Parameter navigation / editing ----------------------------------
        KeyAction::NextParam => select_adjacent_param(app, true),
        KeyAction::PrevParam => select_adjacent_param(app, false),
        KeyAction::IncreaseParam => step_selected_param(app, 1.0),
        KeyAction::DecreaseParam => step_selected_param(app, -1.0),
        KeyAction::EnterParamEdit => enter_param_edit(app),
        KeyAction::ConfirmParamEdit => confirm_param_edit(app),
        KeyAction::CancelParamEdit => leave_param_edit(app),
        KeyAction::ParamEditChar(c) => push_param_edit_char(app, c),
        KeyAction::ParamEditBackspace => pop_param_edit_char(app),

        // -- Waveform view ---------------------------------------------------
        KeyAction::ZoomIn => zoom_waveform(app, 2.0),
        KeyAction::ZoomOut => zoom_waveform(app, 0.5),
        KeyAction::ScrollLeft => scroll_waveform(app, -0.1),
        KeyAction::ScrollRight => scroll_waveform(app, 0.1),

        // -- Volume / pan ----------------------------------------------------
        KeyAction::IncreaseVolume => nudge_volume(app, 1.0),
        KeyAction::DecreaseVolume => nudge_volume(app, -1.0),
        KeyAction::PanLeft => nudge_pan(app, -0.1),
        KeyAction::PanRight => nudge_pan(app, 0.1),

        // -- Mixer view ------------------------------------------------------
        KeyAction::MixerNextChannel => select_adjacent_mixer_channel(app, true),
        KeyAction::MixerPrevChannel => select_adjacent_mixer_channel(app, false),
        KeyAction::MixerNextControl => cycle_mixer_control(app, true),
        KeyAction::MixerPrevControl => cycle_mixer_control(app, false),

        // -- Project view ----------------------------------------------------
        KeyAction::ProjectNextCard => cycle_project_card(app, true),
        KeyAction::ProjectPrevCard => cycle_project_card(app, false),
        KeyAction::ProjectNextField => cycle_project_field(app, true),
        KeyAction::ProjectPrevField => cycle_project_field(app, false),
        KeyAction::ProjectAdjustUp => apply_project_adjust(app, 1),
        KeyAction::ProjectAdjustDown => apply_project_adjust(app, -1),
        KeyAction::ProjectToggle => apply_project_toggle(app),

        // -- Audio I/O view --------------------------------------------------
        KeyAction::AudioIONextSection => cycle_audio_io_section(app, true),
        KeyAction::AudioIOPrevSection => cycle_audio_io_section(app, false),
        KeyAction::AudioIONextDevice => select_adjacent_device(app, true),
        KeyAction::AudioIOPrevDevice => select_adjacent_device(app, false),

        // -- Timeline / clip operations --------------------------------------
        KeyAction::TimelineZoomIn => zoom_timeline(app, 0.5),
        KeyAction::TimelineZoomOut => zoom_timeline(app, 2.0),
        KeyAction::TimelineScrollLeft => scroll_timeline(app, false),
        KeyAction::TimelineScrollRight => scroll_timeline(app, true),
        KeyAction::SelectNextClip => select_adjacent_clip(app, true),
        KeyAction::SelectPrevClip => select_adjacent_clip(app, false),
        KeyAction::MoveClipLeft => move_selected_clip(app, false),
        KeyAction::MoveClipRight => move_selected_clip(app, true),
        KeyAction::DeleteClip => delete_selected_clip(app),
        KeyAction::SplitClip => split_selected_clip(app),
        KeyAction::DuplicateClip => duplicate_selected_clip(app),

        // -- File browser ----------------------------------------------------
        KeyAction::OpenFileBrowser => app.open_file_browser(),
        KeyAction::FileBrowserDown => move_file_browser_selection(app, true),
        KeyAction::FileBrowserUp => move_file_browser_selection(app, false),
        KeyAction::FileBrowserEnter => apply_file_browser_enter(app),
        KeyAction::FileBrowserBack => apply_file_browser_back(app),
        KeyAction::FileBrowserClose => app.mode = AppMode::Normal,
    }
}

// ---------------------------------------------------------------------------
// Engine command helpers
// ---------------------------------------------------------------------------

/// Send a command to the engine, reporting failure in the status line.
///
/// Returns `true` if the engine accepted the command.
fn send(app: &mut App, action: &str, command: EngineCommand) -> bool {
    let result = app.engine.send_command(command);
    app.status.report(action, result)
}

/// Send a transport command, reporting failure in the status line.
fn send_transport(app: &mut App, action: &str, command: TransportCommand) {
    send(app, action, EngineCommand::Transport(command));
}

/// Report an operation that could not even be attempted.
fn refuse(app: &mut App, reason: &str) {
    app.status.error(reason);
}

// ---------------------------------------------------------------------------
// Lifecycle, views and focus
// ---------------------------------------------------------------------------

fn toggle_help(app: &mut App) {
    app.mode = match app.mode {
        AppMode::Normal => AppMode::Help,
        AppMode::Help | AppMode::FileBrowser { .. } => AppMode::Normal,
    };
}

fn switch_view(app: &mut App, view: ActiveView) {
    app.active_view = view;
    // Reset focus to the first panel of the new view.
    app.focused_panel = crate::app::panels_for_view(view)[0];
    // Sync mixer channel selection with the current track.
    if view == ActiveView::Mixer {
        app.mixer_view_state.selected_channel = app.selected_track;
    }
}

fn cycle_focus(app: &mut App, forward: bool) {
    let panels = crate::app::panels_for_view(app.active_view);
    app.focused_panel = panels
        .iter()
        .position(|p| *p == app.focused_panel)
        .map_or(panels[0], |pos| {
            panels[wrap_index(pos, panels.len(), forward)]
        });
}

/// Step `index` one position forward or backward within `0..len`, wrapping
/// at both ends. `len` must be non-zero.
const fn wrap_index(index: usize, len: usize, forward: bool) -> usize {
    if forward {
        (index + 1) % len
    } else if index == 0 {
        len - 1
    } else {
        index - 1
    }
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

fn toggle_play(app: &mut App) {
    if app.display.transport.state == TransportState::Playing {
        send_transport(app, "Pause", TransportCommand::Pause);
    } else {
        send_transport(app, "Play", TransportCommand::Play);
    }
}

fn record_with_count_in(app: &mut App) {
    // Build the workflow from TUI state and send it before the command. If
    // the workflow cannot be set, do not start a recording with the wrong one.
    let workflow = build_recording_workflow(app);
    if send(
        app,
        "Set recording workflow",
        EngineCommand::Transport(TransportCommand::SetRecordingWorkflow(workflow)),
    ) {
        send_transport(app, "Record", TransportCommand::RecordWithCountIn);
    }
}

fn toggle_loop(app: &mut App) {
    // The transport API uses SetLoop(Some/None) rather than a simple toggle,
    // so check the current state. Enabling uses a default loop region
    // covering the whole range.
    let region = if app.display.transport.is_looping() {
        None
    } else {
        Some((0, u64::MAX / 2))
    };
    send_transport(app, "Toggle loop", TransportCommand::SetLoop(region));
}

fn toggle_metronome(app: &mut App) {
    send_transport(app, "Toggle metronome", TransportCommand::ToggleMetronome);
}

fn nudge_tempo(app: &mut App, delta: f64) {
    let new_bpm = app.display.transport.bpm + delta;
    send_transport(app, "Set tempo", TransportCommand::SetTempo(new_bpm));
}

/// Switch between the count-in and fixed-length recording workflows.
fn cycle_recording_workflow(app: &mut App) {
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

/// Adjust the number of bars to record, within `0..=64`.
fn adjust_record_bars(app: &mut App, direction: i8) {
    app.record_bars = if direction > 0 {
        app.record_bars.saturating_add(1).min(64)
    } else {
        app.record_bars.saturating_sub(1)
    };
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
///
/// [`RecordingWorkflow`]: kazoo_core::transport::RecordingWorkflow
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

// ---------------------------------------------------------------------------
// Tracks, volume and pan
// ---------------------------------------------------------------------------

fn select_adjacent_track(app: &mut App, forward: bool) {
    if app.tracks.is_empty() {
        return;
    }
    let index = wrap_index(app.selected_track, app.tracks.len(), forward);
    app.selected_track = index;
    app.track_list_state.select(Some(index));
    app.synth_state.selected_effect = 0;
    app.synth_state.selected_param = 0;
}

fn add_numbered_track(app: &mut App) {
    let name = format!("{}", app.track_count() + 1);
    // Failure is reported in the status line by `add_track`.
    if app.add_track(name, SynthesisMode::PitchTracked) {
        let last = app.tracks.len() - 1;
        app.status
            .info(format!("Added track {}", app.tracks[last].name));
    }
}

fn nudge_volume(app: &mut App, delta_db: f32) {
    if let Some(track) = app.selected_track_info() {
        let new_db = Db::new((track.volume.value() + delta_db).clamp(-100.0, 24.0));
        app.set_track_volume(app.selected_track, new_db);
    }
}

fn nudge_pan(app: &mut App, delta: f32) {
    if let Some(track) = app.selected_track_info() {
        let new_pan = Pan::new(track.pan.value() + delta);
        app.set_track_pan(app.selected_track, new_pan);
    }
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

fn select_next_effect(app: &mut App) {
    let effect_count = app.selected_track_info().map_or(0, |t| t.effects.len());
    if effect_count == 0 {
        return;
    }
    if app.synth_state.synth_selected {
        // Move from synth to first effect.
        app.synth_state.synth_selected = false;
        app.synth_state.selected_effect = 0;
        app.synth_state.selected_param = 0;
    } else if app.synth_state.selected_effect + 1 < effect_count {
        app.synth_state.selected_effect += 1;
        app.synth_state.selected_param = 0;
    }
}

const fn select_prev_effect(app: &mut App) {
    if app.synth_state.synth_selected {
        // Already at the top of the list.
        return;
    }
    if app.synth_state.selected_effect == 0 {
        // Move from first effect back to synth.
        app.synth_state.synth_selected = true;
        app.synth_state.selected_synth_param = 0;
    } else {
        app.synth_state.selected_effect -= 1;
        app.synth_state.selected_param = 0;
    }
}

fn add_lowpass_effect(app: &mut App) {
    if app.selected_track_info().is_none() {
        refuse(app, "No track selected \u{2014} add a track (n) first");
        return;
    }
    let sample_rate = app.engine.sample_rate() as f32;
    let effect = kazoo_core::effects::BiquadFilter::new(
        kazoo_core::effects::FilterType::LowPass,
        sample_rate,
    );
    app.add_effect_to_track(app.selected_track, "LowPass".into(), Box::new(effect));
}

// ---------------------------------------------------------------------------
// Parameter navigation and editing
// ---------------------------------------------------------------------------

/// The parameter currently targeted by the synth/effects sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParamTarget {
    /// A parameter of the selected track's synth.
    Synth { track: usize, param: usize },
    /// A parameter of one effect in the selected track's chain.
    Effect {
        track: usize,
        effect: usize,
        param: usize,
    },
}

impl ParamTarget {
    /// Resolve the selection in `app` to a target, without checking that the
    /// indices exist.
    const fn selected(app: &App) -> Self {
        if app.synth_state.synth_selected {
            Self::Synth {
                track: app.selected_track,
                param: app.synth_state.selected_synth_param,
            }
        } else {
            Self::Effect {
                track: app.selected_track,
                effect: app.synth_state.selected_effect,
                param: app.synth_state.selected_param,
            }
        }
    }

    /// The parameter's metadata and current value, if the target exists.
    fn lookup(self, app: &App) -> Option<(&kazoo_core::ParamInfo, f32)> {
        match self {
            Self::Synth { track, param } => {
                let track = app.tracks.get(track)?;
                Some((
                    track.synth_param_infos.get(param)?,
                    *track.synth_param_values.get(param)?,
                ))
            }
            Self::Effect {
                track,
                effect,
                param,
            } => {
                let effect = app.tracks.get(track)?.effects.get(effect)?;
                Some((
                    effect.param_infos.get(param)?,
                    *effect.param_values.get(param)?,
                ))
            }
        }
    }

    /// Number of parameters on the targeted synth or effect.
    fn param_count(self, app: &App) -> usize {
        match self {
            Self::Synth { track, .. } => app
                .tracks
                .get(track)
                .map_or(0, |t| t.synth_param_infos.len()),
            Self::Effect { track, effect, .. } => app
                .tracks
                .get(track)
                .and_then(|t| t.effects.get(effect))
                .map_or(0, |e| e.param_infos.len()),
        }
    }
}

/// Move the parameter selection within the selected synth or effect,
/// wrapping at both ends.
fn select_adjacent_param(app: &mut App, forward: bool) {
    let count = ParamTarget::selected(app).param_count(app);
    if count == 0 {
        return;
    }
    let index = if app.synth_state.synth_selected {
        &mut app.synth_state.selected_synth_param
    } else {
        &mut app.synth_state.selected_param
    };
    // The selection can be stale (e.g. after switching to an effect with
    // fewer parameters); bring it into range before stepping.
    *index = wrap_index((*index).min(count - 1), count, forward);
}

/// Compute the next value when stepping a parameter one notch.
///
/// Enum-style parameters (`min == 0`, integral `max <= 3`) step by 1 and
/// snap to integers; all others step by 5% of their range. The result is
/// clamped to the parameter's range.
fn stepped_param_value(info: &kazoo_core::ParamInfo, current: f32, direction: f32) -> f32 {
    let is_enum =
        info.min == 0.0 && info.max <= 3.0 && (info.max - info.max.floor()).abs() < f32::EPSILON;
    let step = if is_enum {
        1.0
    } else {
        (info.max - info.min) / 20.0
    };
    let new_value = info.clamp(direction.mul_add(step, current));
    if is_enum {
        new_value.round()
    } else {
        new_value
    }
}

/// Step the selected synth or effect parameter by one notch in `direction`.
fn step_selected_param(app: &mut App, direction: f32) {
    let target = ParamTarget::selected(app);
    let Some((info, current)) = target.lookup(app) else {
        refuse(app, "No parameter selected");
        return;
    };
    let new_value = stepped_param_value(info, current, direction);
    set_param(app, target, new_value);
}

/// Send `value` for `target` to the engine and, if accepted, store it
/// locally. `value` must already be clamped to the parameter's range.
fn set_param(app: &mut App, target: ParamTarget, value: f32) {
    let accepted = match target {
        ParamTarget::Synth { track, param } => {
            let Some(track_id) = app.tracks.get(track).map(|t| t.id) else {
                return;
            };
            send(
                app,
                "Set synth parameter",
                EngineCommand::SetSynthLayerParameter {
                    track_id,
                    layer_index: 0,
                    param_index: param,
                    value,
                },
            )
        }
        ParamTarget::Effect {
            track,
            effect,
            param,
        } => {
            let Some(track_id) = app.tracks.get(track).map(|t| t.id) else {
                return;
            };
            send(
                app,
                "Set effect parameter",
                EngineCommand::SetEffectParameter {
                    track_id,
                    effect_index: effect,
                    param_index: param,
                    value,
                },
            )
        }
    };
    if !accepted {
        return;
    }
    let slot = match target {
        ParamTarget::Synth { track, param } => app
            .tracks
            .get_mut(track)
            .and_then(|t| t.synth_param_values.get_mut(param)),
        ParamTarget::Effect {
            track,
            effect,
            param,
        } => app
            .tracks
            .get_mut(track)
            .and_then(|t| t.effects.get_mut(effect))
            .and_then(|e| e.param_values.get_mut(param)),
    };
    if let Some(slot) = slot {
        *slot = value;
    }
}

fn enter_param_edit(app: &mut App) {
    if ParamTarget::selected(app).lookup(app).is_none() {
        refuse(app, "No parameter selected to edit");
        return;
    }
    app.input_mode = InputMode::ParameterEdit;
    app.param_edit_buffer.clear();
}

fn leave_param_edit(app: &mut App) {
    app.input_mode = InputMode::Normal;
    app.param_edit_buffer.clear();
}

/// Apply the typed value to the selected parameter.
///
/// Invalid input (not a number, NaN/infinite) is rejected with a status
/// message and nothing is sent. Valid values are clamped to the parameter's
/// range; clamping is reported so the user knows the value was changed.
fn confirm_param_edit(app: &mut App) {
    let target = ParamTarget::selected(app);
    let raw = app.param_edit_buffer.trim().to_owned();
    leave_param_edit(app);

    let value = match raw.parse::<f32>() {
        Ok(value) if value.is_finite() => value,
        Ok(_) => {
            refuse(app, &format!("'{raw}' is not a finite number"));
            return;
        }
        Err(err) => {
            refuse(app, &format!("'{raw}' is not a number: {err}"));
            return;
        }
    };
    let Some((info, _)) = target.lookup(app) else {
        refuse(app, "The parameter being edited no longer exists");
        return;
    };
    let clamped = info.clamp(value);
    let range_note = ((clamped - value).abs() > f32::EPSILON)
        .then(|| format!("{value} clamped to {clamped} ({}..={})", info.min, info.max));
    set_param(app, target, clamped);
    if let Some(note) = range_note {
        app.status.info(note);
    }
}

/// Maximum number of characters accepted in the numeric edit buffer.
const PARAM_EDIT_MAX_LEN: usize = 16;

fn push_param_edit_char(app: &mut App, c: char) {
    if app.param_edit_buffer.len() < PARAM_EDIT_MAX_LEN {
        app.param_edit_buffer.push(c);
    }
}

fn pop_param_edit_char(app: &mut App) {
    // Backspace on an empty buffer is a no-op, as in any text field.
    app.param_edit_buffer.pop();
}

// ---------------------------------------------------------------------------
// Waveform and timeline navigation
// ---------------------------------------------------------------------------

fn zoom_waveform(app: &mut App, factor: f32) {
    app.tracking_state.waveform_zoom = (app.tracking_state.waveform_zoom * factor).clamp(1.0, 64.0);
}

fn scroll_waveform(app: &mut App, delta: f32) {
    app.tracking_state.waveform_scroll =
        (app.tracking_state.waveform_scroll + delta).clamp(0.0, 1.0);
}

fn zoom_timeline(app: &mut App, factor: f64) {
    app.tracking_state.timeline_zoom =
        (app.tracking_state.timeline_zoom * factor).clamp(1.0, 1_048_576.0);
}

fn scroll_timeline(app: &mut App, forward: bool) {
    let step = app.tracking_state.timeline_zoom * 10.0;
    let scroll = &mut app.tracking_state.timeline_scroll;
    *scroll = if forward {
        *scroll + step
    } else {
        (*scroll - step).max(0.0)
    };
}

// ---------------------------------------------------------------------------
// Mixer, project and audio I/O views
// ---------------------------------------------------------------------------

fn select_adjacent_mixer_channel(app: &mut App, forward: bool) {
    let track_count = app.tracks.len();
    if track_count == 0 {
        return;
    }
    let index = wrap_index(
        app.mixer_view_state.selected_channel.min(track_count - 1),
        track_count,
        forward,
    );
    app.mixer_view_state.selected_channel = index;
    app.selected_track = index;
    app.track_list_state.select(Some(index));
    app.synth_state.selected_effect = 0;
    app.synth_state.selected_param = 0;
}

const fn cycle_mixer_control(app: &mut App, forward: bool) {
    let control = app.mixer_view_state.selected_control;
    app.mixer_view_state.selected_control = if forward {
        control.next()
    } else {
        control.prev()
    };
}

/// Number of settings cards in the Project view.
const PROJECT_CARD_COUNT: usize = 6;

const fn cycle_project_card(app: &mut App, forward: bool) {
    app.project_state.selected_card =
        wrap_index(app.project_state.selected_card, PROJECT_CARD_COUNT, forward);
    app.project_state.selected_field = 0;
}

const fn cycle_project_field(app: &mut App, forward: bool) {
    let field_count = project_card_field_count(app.project_state.selected_card);
    if field_count > 0 {
        app.project_state.selected_field =
            wrap_index(app.project_state.selected_field, field_count, forward);
    }
}

const fn cycle_audio_io_section(app: &mut App, forward: bool) {
    use crate::state::DeviceListFocus;
    app.audio_io_state.focus = match (app.audio_io_state.focus, forward) {
        (DeviceListFocus::Input, true) | (DeviceListFocus::Settings, false) => {
            DeviceListFocus::Output
        }
        (DeviceListFocus::Output, true) | (DeviceListFocus::Input, false) => {
            DeviceListFocus::Settings
        }
        (DeviceListFocus::Settings, true) | (DeviceListFocus::Output, false) => {
            DeviceListFocus::Input
        }
    };
}

fn select_adjacent_device(app: &mut App, forward: bool) {
    use crate::state::DeviceListFocus;
    let state = &mut app.audio_io_state;
    let (count, selected) = match state.focus {
        DeviceListFocus::Input => (state.input_devices.len(), &mut state.selected_input_device),
        DeviceListFocus::Output => (
            state.output_devices.len(),
            &mut state.selected_output_device,
        ),
        // The settings section has no selectable devices.
        DeviceListFocus::Settings => return,
    };
    if count > 0 {
        *selected = wrap_index((*selected).min(count - 1), count, forward);
    }
}

// ---------------------------------------------------------------------------
// Clip operations
// ---------------------------------------------------------------------------

/// The selected track and clip, if both exist. Reports why not otherwise.
fn selected_clip_target(app: &mut App) -> Option<(kazoo_core::mixer::TrackId, ClipId)> {
    let Some(track_id) = app.selected_track_id() else {
        refuse(app, "No track selected");
        return None;
    };
    let Some(clip_id) = app.tracking_state.selected_clip else {
        refuse(app, "No clip selected \u{2014} use , and . to select one");
        return None;
    };
    Some((track_id, clip_id))
}

/// Look up the selected clip's `(position, length)` in the latest timeline
/// snapshot. If it no longer exists the stale selection is cleared and the
/// user is told.
fn selected_clip_extent(app: &mut App, clip_id: ClipId) -> Option<(u64, u64)> {
    if let Some(clip) = find_clip_in_timeline(&app.display.timeline, clip_id) {
        return Some((clip.position, clip.length));
    }
    app.tracking_state.selected_clip = None;
    refuse(app, "The selected clip no longer exists");
    None
}

fn move_selected_clip(app: &mut App, forward: bool) {
    let Some((track_id, clip_id)) = selected_clip_target(app) else {
        return;
    };
    let Some((position, _)) = selected_clip_extent(app, clip_id) else {
        return;
    };
    // Move by 1 beat (based on current BPM).
    let beat = beat_samples(app.display.transport.bpm, app.engine.sample_rate());
    let new_pos = if forward {
        position.saturating_add(beat)
    } else {
        position.saturating_sub(beat)
    };
    let result = app.engine.move_clip(track_id, clip_id, new_pos);
    app.status.report("Move clip", result);
}

fn delete_selected_clip(app: &mut App) {
    let Some((track_id, clip_id)) = selected_clip_target(app) else {
        return;
    };
    let result = app.engine.remove_clip(track_id, clip_id);
    if app.status.report("Delete clip", result) {
        app.tracking_state.selected_clip = None;
    }
}

fn split_selected_clip(app: &mut App) {
    let Some((track_id, clip_id)) = selected_clip_target(app) else {
        return;
    };
    let pos = app.display.transport.position.samples;
    let result = app.engine.split_clip(track_id, clip_id, pos);
    app.status.report("Split clip", result);
}

fn duplicate_selected_clip(app: &mut App) {
    let Some((track_id, clip_id)) = selected_clip_target(app) else {
        return;
    };
    let Some((position, length)) = selected_clip_extent(app, clip_id) else {
        return;
    };
    // Place the duplicate right after the original clip.
    let new_pos = position.saturating_add(length);
    let result = app.engine.duplicate_clip(track_id, clip_id, new_pos);
    app.status.report("Duplicate clip", result);
}

// ---------------------------------------------------------------------------
// File browser
// ---------------------------------------------------------------------------

fn move_file_browser_selection(app: &mut App, forward: bool) {
    if let AppMode::FileBrowser {
        ref entries,
        ref mut selected,
        ..
    } = app.mode
    {
        if !entries.is_empty() {
            *selected = wrap_index((*selected).min(entries.len() - 1), entries.len(), forward);
        }
    }
}

fn apply_file_browser_back(app: &mut App) {
    let AppMode::FileBrowser { ref directory, .. } = app.mode else {
        return;
    };
    // At the filesystem root there is no parent; staying put is the
    // expected behaviour of "go up".
    if let Some(parent) = directory.parent().map(std::path::Path::to_path_buf) {
        app.browse_to(parent);
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
        // Navigate into directory; failures are reported by `browse_to`.
        app.browse_to(path);
        return;
    }

    // Load audio file onto current track at playhead position.
    app.mode = AppMode::Normal;
    let Some(track_id) = app.selected_track_id() else {
        refuse(app, "No track selected \u{2014} cannot load clip");
        return;
    };
    let position = app.display.transport.position.samples;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    match app.engine.load_clip(track_id, &path, position) {
        Ok(()) => app.status.info(format!("Loaded {name}")),
        Err(err) => app.status.error(format!("Load {name} failed: {err}")),
    }
}

// ---------------------------------------------------------------------------
// Clip selection helpers
// ---------------------------------------------------------------------------

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
        if let Some(first_clip) = timeline.tracks.first().and_then(|t| t.clips.first()) {
            app.tracking_state.selected_clip = Some(ClipId(first_clip.id));
        }
        return;
    };

    if track.clips.is_empty() {
        app.tracking_state.selected_clip = None;
        return;
    }

    let clip_count = track.clips.len();
    let next = app.tracking_state.selected_clip.map_or(
        // Nothing selected: select first or last.
        if forward { 0 } else { clip_count - 1 },
        // Selected: step from it; if it vanished, reset to the first clip.
        |current| {
            track
                .clips
                .iter()
                .position(|c| c.id == current.0)
                .map_or(0, |i| wrap_index(i, clip_count, forward))
        },
    );
    app.tracking_state.selected_clip = Some(ClipId(track.clips[next].id));
}

/// Find a clip in the timeline snapshot by its ID.
fn find_clip_in_timeline(
    timeline: &kazoo_core::engine::TimelineSnapshot,
    clip_id: ClipId,
) -> Option<&kazoo_core::engine::ClipSnapshot> {
    timeline
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .find(|clip| clip.id == clip_id.0)
}

/// Compute samples per beat at the given BPM and sample rate.
///
/// Returns 0 for non-positive or non-finite BPM, or a zero sample rate.
fn beat_samples(bpm: f64, sample_rate: u32) -> u64 {
    if !bpm.is_finite() || bpm <= 0.0 || sample_rate == 0 {
        return 0;
    }
    (f64::from(sample_rate) * 60.0 / bpm) as u64
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
///
/// Fields without an adjustable value (time signature, toggles) ignore
/// +/-; toggles respond to Enter instead.
fn apply_project_adjust(app: &mut App, direction: i8) {
    match (
        app.project_state.selected_card,
        app.project_state.selected_field,
    ) {
        // Card 0 (Tempo), field 0: adjust BPM.
        (0, 0) => nudge_tempo(app, f64::from(direction)),
        // Card 2 (Count-In), field 1: adjust count-in bars.
        (2, 1) => {
            app.count_in_bars = if direction > 0 {
                app.count_in_bars.saturating_add(1).min(16)
            } else {
                app.count_in_bars.saturating_sub(1)
            };
        }
        // Card 5 (Recording), field 0: cycle workflow.
        (5, 0) => cycle_recording_workflow(app),
        // Card 5 (Recording), field 1: adjust record bars.
        (5, 1) => adjust_record_bars(app, direction),
        _ => {}
    }
}

/// Toggle boolean fields in the project view.
fn apply_project_toggle(app: &mut App) {
    match (
        app.project_state.selected_card,
        app.project_state.selected_field,
    ) {
        // Card 2 (Count-In), field 0: toggle count-in enabled.
        (2, 0) => app.count_in_bars = u8::from(app.count_in_bars == 0),
        // Card 3 (Metronome), field 0: toggle metronome.
        (3, 0) => toggle_metronome(app),
        // Card 4 (Loop), field 0: toggle loop.
        (4, 0) => toggle_loop(app),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::KeyModifiers;

    use crate::test_support::TestApp;

    /// Create a test app with no tracks.
    fn test_app() -> TestApp {
        TestApp::empty()
    }

    /// Create a test app with some tracks pre-populated.
    fn test_app_with_tracks(count: usize) -> TestApp {
        TestApp::with_tracks(count)
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
        let expected = initial_zoom * 2.0;
        assert!((app.tracking_state.timeline_zoom - expected).abs() < f64::EPSILON);
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
                recording: None,
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
                recording: None,
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
                recording: None,
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
        assert_eq!(app.tracks[0].effects.len(), 1);
        assert!(!app.tracks[0].effects[0].bypassed);
        assert!(
            !app.tracks[0].effects[0].param_infos.is_empty(),
            "effect parameters must be captured for display and editing"
        );
    }

    #[test]
    fn add_multiple_effects_preserves_order() {
        let mut app = test_app_with_tracks(1);
        apply_action(&mut app, KeyAction::AddEffect);
        apply_action(&mut app, KeyAction::AddEffect);
        apply_action(&mut app, KeyAction::AddEffect);
        assert_eq!(app.tracks[0].effects.len(), 3);
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
        let track = app.selected_track;
        app.remove_effect(track, 2);
        assert_eq!(app.tracks[0].effects.len(), 2);
        assert!(app.synth_state.selected_effect <= 1);
    }

    #[test]
    fn remove_effect_on_empty_chain_is_noop() {
        let mut app = test_app_with_tracks(1);
        assert!(app.tracks[0].effects.is_empty());

        // Should not panic.
        let track = app.selected_track;
        app.remove_effect(track, 0);
        assert!(app.tracks[0].effects.is_empty());
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
        assert_eq!(app.tracks[0].effects.len(), 1);

        app.remove_effect(0, 0);
        assert!(app.tracks[0].effects.is_empty());

        apply_action(&mut app, KeyAction::AddEffect);
        assert_eq!(app.tracks[0].effects.len(), 1);
    }

    #[test]
    fn effect_operations_isolated_to_selected_track() {
        let mut app = test_app_with_tracks(3);
        app.selected_track = 0;
        apply_action(&mut app, KeyAction::AddEffect);

        // Only track 0 should have an effect.
        assert_eq!(app.tracks[0].effects.len(), 1);
        assert!(app.tracks[1].effects.is_empty());
        assert!(app.tracks[2].effects.is_empty());
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
        let expected = initial * 2.0;
        assert!((app.tracking_state.timeline_zoom - expected).abs() < f64::EPSILON);
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

    // -----------------------------------------------------------------------
    // Error reporting and engine command outcomes
    // -----------------------------------------------------------------------

    use crate::status::StatusLevel;

    fn visible_status(app: &App) -> Option<(StatusLevel, String)> {
        app.status
            .visible(std::time::Instant::now())
            .map(|m| (m.level, m.text.clone()))
    }

    #[test]
    fn play_with_engine_down_reports_error() {
        let mut app = test_app();
        app.disconnect_engine();
        apply_action(&mut app, KeyAction::Play);
        assert_eq!(
            visible_status(&app),
            Some((StatusLevel::Error, "Play failed: Engine not running".into()))
        );
    }

    #[test]
    fn stop_sends_stop_command() {
        let mut app = test_app();
        apply_action(&mut app, KeyAction::Stop);
        let commands = app.take_commands();
        assert!(matches!(
            commands.as_slice(),
            [EngineCommand::Transport(TransportCommand::Stop)]
        ));
        assert!(visible_status(&app).is_none());
    }

    #[test]
    fn record_with_count_in_sends_workflow_then_record() {
        let mut app = test_app();
        apply_action(&mut app, KeyAction::RecordWithCountIn);
        let commands = app.take_commands();
        assert_eq!(commands.len(), 2);
        assert!(matches!(
            commands[0],
            EngineCommand::Transport(TransportCommand::SetRecordingWorkflow(_))
        ));
        assert!(matches!(
            commands[1],
            EngineCommand::Transport(TransportCommand::RecordWithCountIn)
        ));
    }

    #[test]
    fn toggle_metronome_with_engine_down_reports_error() {
        let mut app = test_app();
        app.disconnect_engine();
        apply_action(&mut app, KeyAction::ToggleMetronome);
        let (level, text) = visible_status(&app).unwrap();
        assert_eq!(level, StatusLevel::Error);
        assert!(text.starts_with("Toggle metronome failed"), "{text}");
    }

    #[test]
    fn add_effect_without_track_reports_why() {
        let mut app = test_app();
        apply_action(&mut app, KeyAction::AddEffect);
        let (level, text) = visible_status(&app).unwrap();
        assert_eq!(level, StatusLevel::Error);
        assert!(text.contains("No track selected"), "{text}");
    }

    /// An app with one track carrying one low-pass filter, with the filter
    /// selected in the sidebar.
    fn app_with_selected_filter() -> TestApp {
        let mut app = test_app_with_tracks(1);
        apply_action(&mut app, KeyAction::AddEffect);
        app.synth_state.synth_selected = false;
        app.synth_state.selected_effect = 0;
        app.synth_state.selected_param = 0;
        app.take_commands();
        app
    }

    #[test]
    fn increase_effect_param_steps_from_current_value() {
        let mut app = app_with_selected_filter();
        let info = app.tracks[0].effects[0].param_infos[0];
        let before = app.tracks[0].effects[0].param_values[0];

        apply_action(&mut app, KeyAction::IncreaseParam);

        let expected = stepped_param_value(&info, before, 1.0);
        assert!(expected > before);
        let commands = app.take_commands();
        match commands.as_slice() {
            [
                EngineCommand::SetEffectParameter {
                    effect_index: 0,
                    param_index: 0,
                    value,
                    ..
                },
            ] => assert!((value - expected).abs() < f32::EPSILON),
            other => panic!("unexpected commands: {other:?}"),
        }
        assert!((app.tracks[0].effects[0].param_values[0] - expected).abs() < f32::EPSILON);
    }

    #[test]
    fn effect_param_step_with_engine_down_keeps_value() {
        let mut app = app_with_selected_filter();
        let before = app.tracks[0].effects[0].param_values[0];
        app.disconnect_engine();
        apply_action(&mut app, KeyAction::DecreaseParam);
        assert!((app.tracks[0].effects[0].param_values[0] - before).abs() < f32::EPSILON);
        assert_eq!(visible_status(&app).unwrap().0, StatusLevel::Error);
    }

    #[test]
    fn effect_param_navigation_wraps_within_effect() {
        let mut app = app_with_selected_filter();
        let count = app.tracks[0].effects[0].param_infos.len();
        for _ in 0..count {
            apply_action(&mut app, KeyAction::NextParam);
        }
        assert_eq!(app.synth_state.selected_param, 0);
        apply_action(&mut app, KeyAction::PrevParam);
        assert_eq!(app.synth_state.selected_param, count - 1);
    }

    #[test]
    fn confirm_effect_param_edit_clamps_and_reports() {
        let mut app = app_with_selected_filter();
        let max = app.tracks[0].effects[0].param_infos[0].max;
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer = "99999999".into();

        apply_action(&mut app, KeyAction::ConfirmParamEdit);

        assert!((app.tracks[0].effects[0].param_values[0] - max).abs() < f32::EPSILON);
        let (level, text) = visible_status(&app).unwrap();
        assert_eq!(level, StatusLevel::Info);
        assert!(text.contains("clamped"), "{text}");
    }

    #[test]
    fn confirm_invalid_number_reports_and_sends_nothing() {
        let mut app = test_app_with_tracks(1);
        app.take_commands();
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer = "1.2.3".into();

        apply_action(&mut app, KeyAction::ConfirmParamEdit);

        assert!(app.take_commands().is_empty());
        let (level, text) = visible_status(&app).unwrap();
        assert_eq!(level, StatusLevel::Error);
        assert!(text.contains("is not a number"), "{text}");
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    #[test]
    fn confirm_synth_param_edit_sends_clamped_value() {
        let mut app = test_app_with_tracks(1);
        app.take_commands();
        app.synth_state.synth_selected = true;
        app.synth_state.selected_synth_param = 0;
        let info = app.tracks[0].synth_param_infos[0];
        app.input_mode = InputMode::ParameterEdit;
        app.param_edit_buffer = format!("{}", info.max + 1000.0);

        apply_action(&mut app, KeyAction::ConfirmParamEdit);

        match app.take_commands().as_slice() {
            [
                EngineCommand::SetSynthLayerParameter {
                    layer_index: 0,
                    param_index: 0,
                    value,
                    ..
                },
            ] => assert!((value - info.max).abs() < f32::EPSILON),
            other => panic!("unexpected commands: {other:?}"),
        }
        assert!((app.tracks[0].synth_param_values[0] - info.max).abs() < f32::EPSILON);
    }

    #[test]
    fn enter_param_edit_without_parameter_is_refused() {
        let mut app = test_app();
        apply_action(&mut app, KeyAction::EnterParamEdit);
        assert_eq!(app.input_mode, InputMode::Normal);
        assert_eq!(visible_status(&app).unwrap().0, StatusLevel::Error);
    }

    #[test]
    fn stepped_param_value_snaps_enum_params() {
        let info = kazoo_core::ParamInfo {
            name: "Wave",
            min: 0.0,
            max: 3.0,
            default: 0.0,
            unit: "",
        };
        assert!((stepped_param_value(&info, 1.0, 1.0) - 2.0).abs() < f32::EPSILON);
        assert!((stepped_param_value(&info, 3.0, 1.0) - 3.0).abs() < f32::EPSILON);
        assert!((stepped_param_value(&info, 0.0, -1.0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn stepped_param_value_rejects_nan_current() {
        let info = kazoo_core::ParamInfo {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 1_000.0,
            unit: "Hz",
        };
        let value = stepped_param_value(&info, f32::NAN, 1.0);
        assert!(value.is_finite());
        assert!((20.0..=20_000.0).contains(&value));
    }

    #[test]
    fn stale_clip_selection_is_cleared_and_reported() {
        let mut app = test_app_with_tracks(1);
        app.take_commands();
        app.tracking_state.selected_clip = Some(ClipId(42));

        apply_action(&mut app, KeyAction::MoveClipRight);

        assert!(app.tracking_state.selected_clip.is_none());
        assert!(app.take_commands().is_empty());
        let (_, text) = visible_status(&app).unwrap();
        assert!(text.contains("no longer exists"), "{text}");
    }

    #[test]
    fn clip_action_without_selection_reports_why() {
        let mut app = test_app_with_tracks(1);
        apply_action(&mut app, KeyAction::SplitClip);
        let (_, text) = visible_status(&app).unwrap();
        assert!(text.contains("No clip selected"), "{text}");
    }

    #[test]
    fn delete_clip_with_engine_down_keeps_selection() {
        let mut app = test_app_with_tracks(1);
        app.tracking_state.selected_clip = Some(ClipId(7));
        app.disconnect_engine();
        apply_action(&mut app, KeyAction::DeleteClip);
        assert_eq!(app.tracking_state.selected_clip, Some(ClipId(7)));
        assert_eq!(visible_status(&app).unwrap().0, StatusLevel::Error);
    }

    #[test]
    fn loading_missing_file_reports_error() {
        let mut app = test_app_with_tracks(1);
        let missing = std::env::temp_dir().join(format!(
            "kazoo-tui-missing-{}-{:?}.wav",
            std::process::id(),
            std::thread::current().id()
        ));
        app.mode = AppMode::FileBrowser {
            directory: std::env::temp_dir(),
            entries: vec![crate::app::FileBrowserEntry {
                name: "missing.wav".into(),
                path: missing,
                is_dir: false,
            }],
            selected: 0,
        };

        apply_action(&mut app, KeyAction::FileBrowserEnter);

        assert_eq!(app.mode, AppMode::Normal);
        let (level, text) = visible_status(&app).unwrap();
        assert_eq!(level, StatusLevel::Error);
        assert!(text.starts_with("Load kazoo-tui-missing-"), "{text}");
    }

    #[test]
    fn beat_samples_rejects_non_finite_bpm() {
        assert_eq!(beat_samples(f64::NAN, 44_100), 0);
        assert_eq!(beat_samples(f64::INFINITY, 44_100), 0);
        assert_eq!(beat_samples(120.0, 44_100), 22_050);
    }

    #[test]
    fn wrap_index_wraps_both_ways() {
        assert_eq!(wrap_index(0, 3, true), 1);
        assert_eq!(wrap_index(2, 3, true), 0);
        assert_eq!(wrap_index(0, 3, false), 2);
        assert_eq!(wrap_index(1, 3, false), 0);
    }
}
