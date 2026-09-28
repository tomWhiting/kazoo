//! kazoo-cs80 — Yamaha CS-80 inspired pad synth.
//!
//! 8-voice polyphonic, dual-layer per voice, per-voice analog drift.
//! Also the home for generative/modular synthesis (node graph patching).
//! See `studio/kazoo-cs80.md` for full specification.

mod app;
mod audio;
mod command;
mod input;
pub mod modular;
mod preset;
pub mod synth;
mod terminal;
mod ui;

use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Receiver;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use kazoo_core::ipc::link::{HubLink, LinkConfig, hub_link};

use crate::app::App;
use crate::audio::{AudioSetup, AudioStats, DisplaySnapshot, MAX_CALLBACK_FRAMES};
use crate::command::CommandLink;
use crate::terminal::Tui;

/// Target frame rate for the TUI.
const TARGET_FPS: u64 = 30;

/// Backend error messages queued for the UI before further ones are only
/// counted.
const STREAM_ERROR_BACKLOG: usize = 8;

/// UI-side ends of the audio plumbing, polled once per frame.
struct UiChannels<'a> {
    commands: CommandLink,
    display_rx: Receiver<DisplaySnapshot>,
    stream_error_rx: Receiver<cpal::StreamError>,
    stats: Arc<AudioStats>,
    hub: &'a HubLink,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    // -----------------------------------------------------------------------
    // Audio setup — direct cpal output
    // -----------------------------------------------------------------------

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let supported_config = device.default_output_config()?;
    let sample_rate = supported_config.sample_rate() as f32;
    let channels = usize::from(supported_config.channels());

    // Command channel: UI -> Audio (lock-free bounded MPSC).
    let (cmd_tx, cmd_rx) = crossbeam_channel::bounded(256);

    // Display channel: Audio -> UI. The audio side evicts the oldest unread
    // snapshot when full, so the UI always sees the newest.
    let (display_tx, display_rx) = crossbeam_channel::bounded::<DisplaySnapshot>(2);

    // Backend errors: Audio -> UI, counted when the backlog is full.
    let (stream_error_tx, stream_error_rx) = crossbeam_channel::bounded(STREAM_ERROR_BACKLOG);
    let stats = Arc::new(AudioStats::default());

    // Plug into the kazoo-mix desk whenever it is running.
    let (hub, hub_audio) = hub_link(LinkConfig::new(
        "kazoo-cs80",
        2,
        supported_config.sample_rate(),
        MAX_CALLBACK_FRAMES as u32,
    ))?;

    let stream = audio::build_audio_stream(
        &device,
        &supported_config.into(),
        AudioSetup {
            sample_rate,
            channels,
            cmd_rx,
            display_tx,
            display_evict: display_rx.clone(),
            stream_error_tx,
            hub_audio,
            stats: Arc::clone(&stats),
        },
    )?;
    stream.play()?;

    // -----------------------------------------------------------------------
    // Terminal + event loop
    // -----------------------------------------------------------------------

    let (mut tui, keyboard) = terminal::setup_terminal()?;

    let mut app = App::new(sample_rate);
    app.key_releases = keyboard.clone();
    let mut channels = UiChannels {
        commands: CommandLink::new(cmd_tx),
        display_rx,
        stream_error_rx,
        stats,
        hub: &hub,
    };
    let result = run(&mut tui, &mut app, &mut channels);

    // -----------------------------------------------------------------------
    // Cleanup: always restore the terminal, even if the loop failed.
    // -----------------------------------------------------------------------

    drop(stream);
    drop(channels);
    drop(hub);
    let restored = terminal::restore_terminal(&mut tui, &keyboard);
    terminal::finish("kazoo-cs80", result, restored)
}

/// The UI event loop.
fn run(tui: &mut Tui, app: &mut App, channels: &mut UiChannels<'_>) -> color_eyre::Result<()> {
    let frame_duration = Duration::from_millis(1000 / TARGET_FPS);

    loop {
        let frame_start = Instant::now();

        poll_audio(app, channels);
        app.frame += 1;

        tui.draw(|f| ui::draw(f, app))?;

        let timeout = frame_duration.saturating_sub(frame_start.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.kind {
                    KeyEventKind::Press => {
                        app.shift_held = key.modifiers.contains(KeyModifiers::SHIFT);
                        handle_key(app, key.code, key.modifiers, &mut channels.commands);
                    }
                    KeyEventKind::Release => {
                        handle_key_release(app, key.code, &mut channels.commands);
                    }
                    // Auto-repeat: a held key is already sounding.
                    KeyEventKind::Repeat => {}
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Pull everything the audio side has produced into the app state, and
/// retry commands the audio callback could not take earlier.
fn poll_audio(app: &mut App, channels: &mut UiChannels<'_>) {
    // Keep only the latest display snapshot.
    if let Some(snap) = channels.display_rx.try_iter().last() {
        app.voice_status = snap.voice_status;
        app.waveform_buf.copy_from_slice(&snap.waveform);
    }
    if let Some(err) = channels.stream_error_rx.try_iter().last() {
        app.health.last_stream_error = Some(err.to_string());
    }

    channels.commands.flush(&app.synth.params);
    app.health.queue = channels.commands.status();
    app.health.display_dropped = channels.stats.display_dropped();
    app.health.stream_errors = channels.stats.stream_errors();
    app.health.stream_errors_unreported = channels.stats.stream_errors_unreported();
    app.hub = Some(channels.hub.status());
}

/// Handle a key press event.
fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers, commands: &mut CommandLink) {
    let ctrl = modifiers.contains(KeyModifiers::CONTROL);
    let shift = modifiers.contains(KeyModifiers::SHIFT);

    match code {
        // Quit.
        KeyCode::Char('`') | KeyCode::Esc => app.should_quit = true,

        // Panic: silence every voice, forget every held key.
        KeyCode::Backspace => {
            app.release_all_notes();
            commands.all_notes_off();
            app.set_status("All notes off", false);
        }

        // Toggle modular view (F2).
        KeyCode::F(2) => app.toggle_view(),

        // Preset save/load (Ctrl+S / Ctrl+L).
        KeyCode::Char('s') if ctrl => save_preset(app),
        KeyCode::Char('l') if ctrl => load_preset(app, commands),

        // Section navigation.
        KeyCode::Tab => app.next_section(),
        KeyCode::BackTab => app.prev_section(),

        // Aftertouch (Shift+Up/Down).
        KeyCode::Up if shift => {
            let pressure = app.increase_aftertouch();
            send_aftertouch_for_held_notes(app, pressure, commands);
        }
        KeyCode::Down if shift => {
            let pressure = app.decrease_aftertouch();
            send_aftertouch_for_held_notes(app, pressure, commands);
        }

        // Parameter navigation (arrow keys only — j/k are musical keys).
        KeyCode::Up => app.prev_param(),
        KeyCode::Down => app.next_param(),

        // Parameter adjustment (Shift+arrow = coarse via app.shift_held).
        KeyCode::Char('+' | '=') | KeyCode::Right => {
            app.increment_param();
            commands.params(&app.synth.params);
        }
        KeyCode::Char('-' | '_') | KeyCode::Left => {
            app.decrement_param();
            commands.params(&app.synth.params);
        }

        // Octave shift.
        KeyCode::Char('[') => app.octave_down(),
        KeyCode::Char(']') => app.octave_up(),

        // Musical keyboard — note on.
        KeyCode::Char(ch) => {
            let Some(note) = input::key_to_note(ch, app.octave) else {
                return;
            };
            let Some(slot) = key_slot(ch) else {
                return;
            };
            // Ignore key repeat — only trigger note_on on the initial press.
            // Without this guard, held keys fire repeated note_on messages
            // which causes the synth to "build up" or re-trigger.
            if app.key_note_map[slot].is_some() {
                return;
            }
            app.key_note_map[slot] = Some(note);
            app.note_on(note, input::DEFAULT_VELOCITY);
            commands.note_on(note, input::DEFAULT_VELOCITY);
        }

        _ => {}
    }
}

/// Index into `App::key_note_map` for an ASCII key, `None` otherwise.
fn key_slot(ch: char) -> Option<usize> {
    ch.is_ascii().then_some(ch as usize)
}

/// Send aftertouch for all currently held notes.
fn send_aftertouch_for_held_notes(app: &App, pressure: f32, commands: &mut CommandLink) {
    for (note, &held) in (0..=u8::MAX).zip(app.held_notes.iter()) {
        if held {
            commands.aftertouch(note, pressure);
        }
    }
}

/// Save current synth params to the quick-save preset, reporting the outcome.
fn save_preset(app: &mut App) {
    let saved = preset::preset_dir().and_then(|dir| preset::save_to(&dir, &app.synth.params));
    match saved {
        Ok(path) => app.set_status(format!("Preset saved to {}", path.display()), false),
        Err(err) => app.set_status(format!("Preset not saved: {err}"), true),
    }
}

/// Load the quick-save preset, reporting the outcome. A preset that fails
/// validation is not applied.
fn load_preset(app: &mut App, commands: &mut CommandLink) {
    match preset::preset_dir().and_then(|dir| preset::load_from(&dir)) {
        Ok(params) => {
            app.synth.params = params;
            app.synth.apply_params();
            commands.params(&app.synth.params);
            app.set_status("Preset loaded", false);
        }
        Err(err) => app.set_status(format!("Preset not loaded: {err}"), true),
    }
}

/// Handle a key release event (for note-off).
///
/// Uses the stored `key_note_map` to find the MIDI note that was triggered
/// when this key was originally pressed. This prevents stuck notes when the
/// octave is changed while a key is held — the release sends note-off for
/// the original note, not the one the key would map to at the new octave.
fn handle_key_release(app: &mut App, code: KeyCode, commands: &mut CommandLink) {
    if let KeyCode::Char(ch) = code {
        if let Some(note) = key_slot(ch).and_then(|slot| app.key_note_map[slot].take()) {
            app.note_off(note);
            commands.note_off(note);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::AudioCommand;

    fn link() -> (CommandLink, Receiver<AudioCommand>) {
        let (tx, rx) = crossbeam_channel::bounded(16);
        (CommandLink::new(tx), rx)
    }

    #[test]
    fn key_press_and_release_send_note_on_and_off() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100.0);
        handle_key(
            &mut app,
            KeyCode::Char('z'),
            KeyModifiers::NONE,
            &mut commands,
        );
        handle_key_release(&mut app, KeyCode::Char('z'), &mut commands);
        let got: Vec<AudioCommand> = rx.try_iter().collect();
        assert_eq!(got.len(), 2);
        assert!(matches!(got[0], AudioCommand::NoteOn { .. }));
        assert!(matches!(got[1], AudioCommand::NoteOff { .. }));
    }

    #[test]
    fn repeated_press_does_not_retrigger() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100.0);
        handle_key(
            &mut app,
            KeyCode::Char('z'),
            KeyModifiers::NONE,
            &mut commands,
        );
        handle_key(
            &mut app,
            KeyCode::Char('z'),
            KeyModifiers::NONE,
            &mut commands,
        );
        assert_eq!(rx.try_iter().count(), 1);
    }

    #[test]
    fn backspace_releases_everything() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100.0);
        handle_key(
            &mut app,
            KeyCode::Char('z'),
            KeyModifiers::NONE,
            &mut commands,
        );
        handle_key(
            &mut app,
            KeyCode::Backspace,
            KeyModifiers::NONE,
            &mut commands,
        );
        let got: Vec<AudioCommand> = rx.try_iter().collect();
        assert!(matches!(got.last(), Some(AudioCommand::AllNotesOff)));
        assert!(app.held_notes.iter().all(|&h| !h));
        assert!(app.key_note_map.iter().all(Option::is_none));
        // The key can be played again straight away.
        handle_key(
            &mut app,
            KeyCode::Char('z'),
            KeyModifiers::NONE,
            &mut commands,
        );
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::NoteOn { .. })));
    }

    #[test]
    fn non_ascii_key_is_ignored() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100.0);
        handle_key(
            &mut app,
            KeyCode::Char('\u{e9}'),
            KeyModifiers::NONE,
            &mut commands,
        );
        handle_key_release(&mut app, KeyCode::Char('\u{e9}'), &mut commands);
        assert_eq!(rx.try_iter().count(), 0);
    }

    #[test]
    fn param_change_is_sent() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100.0);
        handle_key(&mut app, KeyCode::Right, KeyModifiers::NONE, &mut commands);
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::UpdateParams(_))));
    }
}
