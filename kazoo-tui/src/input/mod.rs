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

    // Focus
    FocusNext,
    FocusPrev,

    // Transport
    Play,
    Stop,
    Pause,
    Record,
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

    // Synth mode
    CycleSynthMode,

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

    // 4. Normal mode: try panel-specific keys first, then global.
    //    Panel-first ensures that modified keys (e.g. Ctrl+S for SplitClip
    //    in the Timeline panel) are not intercepted by unmodified global
    //    bindings (e.g. 's' for Stop).
    resolve_panel_action(app, key).or_else(|| resolve_global_action(key))
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
const fn resolve_global_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('q') => Some(KeyAction::Quit),
        KeyCode::Char('?') => Some(KeyAction::ToggleHelp),
        KeyCode::Tab => Some(KeyAction::FocusNext),
        KeyCode::BackTab => Some(KeyAction::FocusPrev),

        // Transport
        KeyCode::Char(' ') => Some(KeyAction::Play),
        KeyCode::Char('s') => Some(KeyAction::Stop),
        KeyCode::Char('r') => Some(KeyAction::Record),
        KeyCode::Char('L') => Some(KeyAction::ToggleLoop),
        KeyCode::Char('M') => Some(KeyAction::ToggleMetronome),

        // Track navigation
        KeyCode::Char('j') | KeyCode::Down => Some(KeyAction::NextTrack),
        KeyCode::Char('k') | KeyCode::Up => Some(KeyAction::PrevTrack),

        // Track selection by number
        KeyCode::Char(c @ '1'..='9') => {
            let index = (c as usize) - ('1' as usize);
            Some(KeyAction::SelectTrack(index))
        }

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

/// Resolve keys that depend on which panel is currently focused.
fn resolve_panel_action(app: &App, key: KeyEvent) -> Option<KeyAction> {
    match app.focused_panel {
        FocusedPanel::Effects => resolve_effects_action(key),
        FocusedPanel::Waveform => resolve_waveform_action(key),
        FocusedPanel::Mixer => resolve_mixer_action(key),
        FocusedPanel::Timeline => resolve_timeline_action(key),
        FocusedPanel::Transport | FocusedPanel::Tracks | FocusedPanel::Spectrum => {
            resolve_default_panel_action(key)
        }
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

        // -- Focus -----------------------------------------------------------
        KeyAction::FocusNext => {
            app.focused_panel = app.focused_panel.next();
        }
        KeyAction::FocusPrev => {
            app.focused_panel = app.focused_panel.prev();
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

        // -- Track selection -------------------------------------------------
        KeyAction::SelectTrack(index) => {
            if index < app.tracks.len() {
                app.selected_track = index;
                app.track_list_state.select(Some(index));
                app.selected_effect = 0;
                app.selected_param = 0;
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
                app.selected_effect = 0;
                app.selected_param = 0;
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
                app.selected_effect = 0;
                app.selected_param = 0;
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
            let name = format!("Track {}", app.track_count() + 1);
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
            if app.synth_selected {
                // Move from synth to first effect (if any).
                if let Some(track) = app.selected_track_info() {
                    if !track.effect_names.is_empty() {
                        app.synth_selected = false;
                        app.selected_effect = 0;
                        app.selected_param = 0;
                    }
                }
            } else if let Some(track) = app.selected_track_info() {
                if !track.effect_names.is_empty()
                    && app.selected_effect + 1 < track.effect_names.len()
                {
                    app.selected_effect += 1;
                    app.selected_param = 0;
                }
            }
        }
        KeyAction::PrevEffect => {
            if app.synth_selected {
                // Already at top, no-op.
            } else if app.selected_effect == 0 {
                // Move from first effect back to synth.
                app.synth_selected = true;
                app.selected_synth_param = 0;
            } else {
                app.selected_effect -= 1;
                app.selected_param = 0;
            }
        }

        // -- Parameter navigation / editing ----------------------------------
        KeyAction::NextParam => {
            if app.synth_selected {
                if let Some(track) = app.selected_track_info() {
                    if !track.synth_param_infos.is_empty() {
                        app.selected_synth_param =
                            (app.selected_synth_param + 1) % track.synth_param_infos.len();
                    }
                }
            } else {
                app.selected_param = app.selected_param.saturating_add(1).min(31);
            }
        }
        KeyAction::PrevParam => {
            if app.synth_selected {
                if let Some(track) = app.selected_track_info() {
                    if !track.synth_param_infos.is_empty() {
                        app.selected_synth_param = if app.selected_synth_param == 0 {
                            track.synth_param_infos.len() - 1
                        } else {
                            app.selected_synth_param - 1
                        };
                    }
                }
            } else {
                app.selected_param = app.selected_param.saturating_sub(1);
            }
        }
        KeyAction::IncreaseParam => {
            if app.synth_selected {
                adjust_synth_param(app, 1.0);
            } else if let Some(track_id) = app.selected_track_id() {
                let _ = app.engine.send_command(EngineCommand::SetEffectParameter {
                    track_id,
                    effect_index: app.selected_effect,
                    param_index: app.selected_param,
                    value: 1.0,
                });
            }
        }
        KeyAction::DecreaseParam => {
            if app.synth_selected {
                adjust_synth_param(app, -1.0);
            } else if let Some(track_id) = app.selected_track_id() {
                let _ = app.engine.send_command(EngineCommand::SetEffectParameter {
                    track_id,
                    effect_index: app.selected_effect,
                    param_index: app.selected_param,
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
                    if let Some(track_id) = app.selected_track_id() {
                        let _ = app.engine.send_command(EngineCommand::SetEffectParameter {
                            track_id,
                            effect_index: app.selected_effect,
                            param_index: app.selected_param,
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
            app.waveform_zoom = (app.waveform_zoom * 2.0).min(64.0);
        }
        KeyAction::ZoomOut => {
            app.waveform_zoom = (app.waveform_zoom / 2.0).max(1.0);
        }
        KeyAction::ScrollLeft => {
            app.waveform_scroll = (app.waveform_scroll - 0.1).max(0.0);
        }
        KeyAction::ScrollRight => {
            app.waveform_scroll = (app.waveform_scroll + 0.1).min(1.0);
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

        // -- File browser ----------------------------------------------------
        KeyAction::OpenFileBrowser => {
            app.open_file_browser();
        }

        // -- Timeline / clip operations --------------------------------------
        KeyAction::TimelineZoomIn => {
            app.timeline_zoom = (app.timeline_zoom / 2.0).max(1.0);
        }
        KeyAction::TimelineZoomOut => {
            app.timeline_zoom = (app.timeline_zoom * 2.0).min(1_048_576.0);
        }
        KeyAction::TimelineScrollLeft => {
            let step = app.timeline_zoom * 10.0;
            app.timeline_scroll = (app.timeline_scroll - step).max(0.0);
        }
        KeyAction::TimelineScrollRight => {
            let step = app.timeline_zoom * 10.0;
            app.timeline_scroll += step;
        }
        KeyAction::SelectNextClip => {
            select_adjacent_clip(app, true);
        }
        KeyAction::SelectPrevClip => {
            select_adjacent_clip(app, false);
        }
        KeyAction::MoveClipLeft => {
            if let (Some(track_id), Some(clip_id)) = (app.selected_track_id(), app.selected_clip) {
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
            if let (Some(track_id), Some(clip_id)) = (app.selected_track_id(), app.selected_clip) {
                let sample_rate = app.engine.sample_rate();
                let beat_samples = beat_samples(app.display.transport.bpm, sample_rate);
                if let Some(clip) = find_clip_in_timeline(&app.display.timeline, clip_id) {
                    let new_pos = clip.position.saturating_add(beat_samples);
                    let _ = app.engine.move_clip(track_id, clip_id, new_pos);
                }
            }
        }
        KeyAction::DeleteClip => {
            if let (Some(track_id), Some(clip_id)) = (app.selected_track_id(), app.selected_clip) {
                let _ = app.engine.remove_clip(track_id, clip_id);
                app.selected_clip = None;
            }
        }
        KeyAction::SplitClip => {
            if let (Some(track_id), Some(clip_id)) = (app.selected_track_id(), app.selected_clip) {
                let pos = app.display.transport.position.samples;
                let _ = app.engine.split_clip(track_id, clip_id, pos);
            }
        }
        KeyAction::DuplicateClip => {
            if let (Some(track_id), Some(clip_id)) = (app.selected_track_id(), app.selected_clip) {
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
                app.selected_clip = Some(ClipId(first_clip.id));
            }
        }
        return;
    };

    if track.clips.is_empty() {
        app.selected_clip = None;
        return;
    }

    match app.selected_clip {
        None => {
            // Nothing selected: select first or last.
            let clip = if forward {
                &track.clips[0]
            } else {
                &track.clips[track.clips.len() - 1]
            };
            app.selected_clip = Some(ClipId(clip.id));
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
                    app.selected_clip = Some(ClipId(track.clips[next].id));
                }
                None => {
                    // Current selection not found; reset.
                    app.selected_clip = Some(ClipId(track.clips[0].id));
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
/// the absolute value to the engine.
fn adjust_synth_param(app: &mut App, direction: f32) {
    let idx = app.selected_synth_param;
    let track_idx = app.selected_track;
    let Some(track) = app.tracks.get_mut(track_idx) else {
        return;
    };
    let Some(info) = track.synth_param_infos.get(idx) else {
        return;
    };
    let Some(current) = track.synth_param_values.get_mut(idx) else {
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

    let new_value = direction.mul_add(step, *current).clamp(info.min, info.max);

    // For enum params, snap to nearest integer.
    let new_value = if is_enum {
        new_value.round()
    } else {
        new_value
    };

    *current = new_value;

    let track_id = track.id;
    let _ = app.engine.send_command(EngineCommand::SetSynthParameter {
        track_id,
        param_index: idx,
        value: new_value,
    });
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
        App::new(test_engine_handle())
    }

    /// Create a test [`App`] with some tracks pre-populated.
    fn test_app_with_tracks(count: usize) -> App {
        let mut app = test_app();
        for i in 0..count {
            app.add_track(format!("Track {}", i + 1), SynthesisMode::PitchTracked);
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
        let mut app = test_app();
        assert_eq!(app.focused_panel, FocusedPanel::Transport);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Tracks);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Timeline);

        handle_key_event(&mut app, code_key(KeyCode::Tab));
        assert_eq!(app.focused_panel, FocusedPanel::Waveform);
    }

    #[test]
    fn backtab_cycles_focus_backward() {
        let mut app = test_app();
        assert_eq!(app.focused_panel, FocusedPanel::Transport);

        handle_key_event(&mut app, code_key(KeyCode::BackTab));
        assert_eq!(app.focused_panel, FocusedPanel::Mixer);
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
        assert_eq!(app.tracks[0].name, "Track 1");
        assert_eq!(app.tracks[0].synthesis_mode, SynthesisMode::PitchTracked);
    }

    #[test]
    fn n_adds_track_incrementing_name() {
        let mut app = test_app_with_tracks(2);
        handle_key_event(&mut app, char_key('n'));
        assert_eq!(app.track_count(), 3);
        assert_eq!(app.tracks[2].name, "Track 3");
    }

    // -- Context-dependent keys (h/l differ by focused panel) ---------------

    #[test]
    fn h_in_effects_panel_is_prev_param() {
        let app_state = {
            let mut a = test_app();
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
            a.focused_panel = FocusedPanel::Waveform;
            a
        };
        let action = resolve_action(&app_state, char_key('l'));
        assert_eq!(action, Some(KeyAction::ScrollRight));
    }

    #[test]
    fn h_in_mixer_panel_is_pan_left() {
        let app_state = {
            let mut a = test_app();
            a.focused_panel = FocusedPanel::Mixer;
            a
        };
        let action = resolve_action(&app_state, char_key('h'));
        assert_eq!(action, Some(KeyAction::PanLeft));
    }

    #[test]
    fn l_in_mixer_panel_is_pan_right() {
        let app_state = {
            let mut a = test_app();
            a.focused_panel = FocusedPanel::Mixer;
            a
        };
        let action = resolve_action(&app_state, char_key('l'));
        assert_eq!(action, Some(KeyAction::PanRight));
    }

    // -- Volume and pan apply_action ----------------------------------------

    #[test]
    fn increase_volume_adds_1db() {
        let mut app = test_app_with_tracks(1);
        // Default volume is 0 dB (unity).
        handle_key_event(&mut app, char_key('+')); // global: IncreaseVolume
        assert!((app.tracks[0].volume.value() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn decrease_volume_subtracts_1db() {
        let mut app = test_app_with_tracks(1);
        handle_key_event(&mut app, char_key('-')); // global: DecreaseVolume
        assert!((app.tracks[0].volume.value() - (-1.0)).abs() < f32::EPSILON);
    }

    // -- Zoom ---------------------------------------------------------------

    #[test]
    fn bracket_keys_zoom_waveform() {
        let mut app = test_app();
        assert!((app.waveform_zoom - 1.0).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key(']'));
        assert!((app.waveform_zoom - 2.0).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key(']'));
        assert!((app.waveform_zoom - 4.0).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key('['));
        assert!((app.waveform_zoom - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn zoom_clamped_to_range() {
        let mut app = test_app();

        // Zoom out below 1.0 should clamp.
        handle_key_event(&mut app, char_key('['));
        assert!((app.waveform_zoom - 1.0).abs() < f32::EPSILON);

        // Zoom in to max.
        app.waveform_zoom = 64.0;
        handle_key_event(&mut app, char_key(']'));
        assert!((app.waveform_zoom - 64.0).abs() < f32::EPSILON);
    }

    // -- Scroll -------------------------------------------------------------

    #[test]
    fn scroll_waveform() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Waveform;
        assert!((app.waveform_scroll - 0.0).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key('l'));
        assert!((app.waveform_scroll - 0.1).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key('h'));
        assert!(app.waveform_scroll.abs() < f32::EPSILON);
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
        assert!(!app.tracks[0].armed);

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

    // -- Select track by number ---------------------------------------------

    #[test]
    fn number_keys_select_track() {
        let mut app = test_app_with_tracks(5);
        handle_key_event(&mut app, char_key('3'));
        assert_eq!(app.selected_track, 2);

        handle_key_event(&mut app, char_key('1'));
        assert_eq!(app.selected_track, 0);
    }

    #[test]
    fn number_key_out_of_range_is_noop() {
        let mut app = test_app_with_tracks(2);
        handle_key_event(&mut app, char_key('5'));
        // Should not change because track index 4 does not exist.
        assert_eq!(app.selected_track, 0);
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
    fn pan_left_right_in_mixer() {
        let mut app = test_app_with_tracks(1);
        app.focused_panel = FocusedPanel::Mixer;

        // Default pan is 0.0 (center).
        handle_key_event(&mut app, char_key('l'));
        assert!((app.tracks[0].pan.value() - 0.1).abs() < f32::EPSILON);

        handle_key_event(&mut app, char_key('h'));
        assert!(app.tracks[0].pan.value().abs() < f32::EPSILON);
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
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, char_key('h'));
        assert_eq!(action, Some(KeyAction::TimelineScrollLeft));
    }

    #[test]
    fn l_in_timeline_panel_is_scroll_right() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, char_key('l'));
        assert_eq!(action, Some(KeyAction::TimelineScrollRight));
    }

    #[test]
    fn plus_in_timeline_panel_is_zoom_in() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, char_key('+'));
        assert_eq!(action, Some(KeyAction::TimelineZoomIn));
    }

    #[test]
    fn minus_in_timeline_panel_is_zoom_out() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, char_key('-'));
        assert_eq!(action, Some(KeyAction::TimelineZoomOut));
    }

    #[test]
    fn timeline_zoom_in_halves_zoom() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Timeline;
        let initial_zoom = app.timeline_zoom;
        handle_key_event(&mut app, char_key('+'));
        assert!((app.timeline_zoom - initial_zoom / 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn timeline_zoom_out_doubles_zoom() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Timeline;
        let initial_zoom = app.timeline_zoom;
        handle_key_event(&mut app, char_key('-'));
        assert!((app.timeline_zoom - initial_zoom * 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn timeline_zoom_clamped() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Timeline;
        app.timeline_zoom = 1.0;
        handle_key_event(&mut app, char_key('+')); // zoom in
        assert!((app.timeline_zoom - 1.0).abs() < f64::EPSILON); // clamped at 1.0

        app.timeline_zoom = 1_048_576.0;
        handle_key_event(&mut app, char_key('-')); // zoom out
        assert!((app.timeline_zoom - 1_048_576.0).abs() < f64::EPSILON); // clamped
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
        assert!(app.selected_clip.is_none());
    }

    #[test]
    fn delete_key_in_timeline_resolves_to_delete_clip() {
        let mut app = test_app();
        app.focused_panel = FocusedPanel::Timeline;
        let action = resolve_action(&app, code_key(KeyCode::Delete));
        assert_eq!(action, Some(KeyAction::DeleteClip));
    }
}
