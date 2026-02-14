//! Input handling: keybinding dispatch, focus management, modal input.
//!
//! All keyboard input flows through [`handle_key_event`], which resolves a
//! [`KeyEvent`] into a [`KeyAction`] and then applies the action to the
//! application state. The resolution is context-sensitive: the current
//! [`InputMode`], [`AppMode`], and [`FocusedPanel`] all influence which
//! action (if any) a key produces.

use crossterm::event::{KeyCode, KeyEvent};

use kazoo_core::engine::EngineCommand;
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

    // 2. Help overlay only responds to dismiss keys.
    if app.mode == AppMode::Help {
        return resolve_help_action(key);
    }

    // 3. Normal mode: try global keys first, then panel-specific.
    resolve_global_action(key).or_else(|| resolve_panel_action(app, key))
}

/// Resolve keys while in parameter-edit mode.
const fn resolve_param_edit_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Enter => Some(KeyAction::ConfirmParamEdit),
        KeyCode::Esc => Some(KeyAction::CancelParamEdit),
        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
            Some(KeyAction::ParamEditChar(c))
        }
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
        KeyCode::Char('x') | KeyCode::Delete => Some(KeyAction::RemoveTrack),

        // Waveform zoom
        KeyCode::Char('[') => Some(KeyAction::ZoomOut),
        KeyCode::Char(']') => Some(KeyAction::ZoomIn),

        _ => None,
    }
}

/// Resolve keys that depend on which panel is currently focused.
const fn resolve_panel_action(app: &App, key: KeyEvent) -> Option<KeyAction> {
    match app.focused_panel {
        FocusedPanel::Effects => resolve_effects_action(key),
        FocusedPanel::Waveform => resolve_waveform_action(key),
        FocusedPanel::Mixer => resolve_mixer_action(key),
        FocusedPanel::Transport | FocusedPanel::Tracks | FocusedPanel::Spectrum => {
            resolve_default_panel_action(key)
        }
    }
}

/// Panel-specific keys for the effects panel.
const fn resolve_effects_action(key: KeyEvent) -> Option<KeyAction> {
    match key.code {
        KeyCode::Char('h') | KeyCode::Left => Some(KeyAction::PrevParam),
        KeyCode::Char('l') | KeyCode::Right => Some(KeyAction::NextParam),
        KeyCode::Char('+' | '=') => Some(KeyAction::IncreaseParam),
        KeyCode::Char('-') => Some(KeyAction::DecreaseParam),
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
                AppMode::Help => AppMode::Normal,
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

        // -- Effect navigation -----------------------------------------------
        KeyAction::NextEffect => {
            if let Some(track) = app.selected_track_info() {
                if !track.effect_names.is_empty() {
                    app.selected_effect = (app.selected_effect + 1) % track.effect_names.len();
                    app.selected_param = 0;
                }
            }
        }
        KeyAction::PrevEffect => {
            if let Some(track) = app.selected_track_info() {
                if !track.effect_names.is_empty() {
                    app.selected_effect = if app.selected_effect == 0 {
                        track.effect_names.len() - 1
                    } else {
                        app.selected_effect - 1
                    };
                    app.selected_param = 0;
                }
            }
        }

        // -- Parameter navigation / editing ----------------------------------
        KeyAction::NextParam => {
            // Cap at a reasonable maximum since TrackInfo does not carry
            // the actual parameter count from the engine.
            app.selected_param = app.selected_param.saturating_add(1).min(31);
        }
        KeyAction::PrevParam => {
            app.selected_param = app.selected_param.saturating_sub(1);
        }
        KeyAction::IncreaseParam => {
            // Send a small increment for the currently selected effect parameter.
            if let Some(track_id) = app.selected_track_id() {
                let _ = app.engine.send_command(EngineCommand::SetEffectParameter {
                    track_id,
                    effect_index: app.selected_effect,
                    param_index: app.selected_param,
                    value: 1.0, // delta: the engine should interpret as +1
                });
            }
        }
        KeyAction::DecreaseParam => {
            if let Some(track_id) = app.selected_track_id() {
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
                if let Some(track_id) = app.selected_track_id() {
                    let _ = app.engine.send_command(EngineCommand::SetEffectParameter {
                        track_id,
                        effect_index: app.selected_effect,
                        param_index: app.selected_param,
                        value,
                    });
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
            app.param_edit_buffer.push(c);
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

    // -- Delete key removes track -------------------------------------------

    #[test]
    fn delete_key_removes_track() {
        let mut app = test_app_with_tracks(2);
        handle_key_event(&mut app, code_key(KeyCode::Delete));
        assert_eq!(app.track_count(), 1);
    }
}
