//! kazoo-mini — Moog Minimoog inspired bass/lead synth.
//!
//! Monophonic. Three VCOs, 24 dB/oct ladder filter with nonlinear
//! saturation, rate-based glide, cross-modulation.
//! See `studio/kazoo-mini.md` for full specification.
//!
//! Standalone operation: direct cpal audio output with lock-free
//! command channel (UI -> Audio) and display channel (Audio -> UI).

mod app;
mod audio;
mod command;
mod input;
mod params;
mod synth;
mod terminal;
mod ui;

use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::Receiver;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use kazoo_core::ipc::link::{HubLink, LinkConfig, hub_link};

use crate::app::App;
use crate::audio::{
    AudioSetup, AudioStats, DISPLAY_BUF_SIZE, DisplaySnapshot, MAX_CALLBACK_FRAMES,
};
use crate::command::CommandLink;
use crate::params::MiniParams;
use crate::terminal::Tui;

/// UI frame period (~60 FPS).
const TICK_RATE: Duration = Duration::from_millis(16);

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
    // Audio setup — direct cpal output, no input stream needed
    // -----------------------------------------------------------------------

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| color_eyre::eyre::eyre!("no audio output device found"))?;
    let supported_config = device.default_output_config()?;
    let sample_rate = supported_config.sample_rate();
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
        "kazoo-mini",
        2,
        sample_rate,
        MAX_CALLBACK_FRAMES as u32,
    ))?;

    let stream = audio::build_audio_stream(
        &device,
        &supported_config.into(),
        AudioSetup {
            sample_rate: sample_rate as f32,
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
    // Cleanup: stop audio, then always restore the terminal.
    // -----------------------------------------------------------------------

    drop(stream);
    drop(channels);
    drop(hub);
    let restored = terminal::restore_terminal(&mut tui, &keyboard);
    terminal::finish("kazoo-mini", result, restored)
}

/// The UI event loop.
fn run(tui: &mut Tui, app: &mut App, channels: &mut UiChannels<'_>) -> color_eyre::Result<()> {
    loop {
        poll_audio(app, channels);

        tui.draw(|f| ui::draw(f, app))?;

        if event::poll(TICK_RATE)? {
            if let Event::Key(key) = event::read()? {
                handle_key_event(app, key, &mut channels.commands);
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
        app.voice.set_display_note(snap.current_note);
        app.voice.set_display_write_pos(snap.write_pos);
        let app_display = app.voice.display_samples_mut();
        let copy_len = app_display.len().min(DISPLAY_BUF_SIZE);
        app_display[..copy_len].copy_from_slice(&snap.waveform[..copy_len]);
    }
    if let Some(err) = channels.stream_error_rx.try_iter().last() {
        app.health.last_stream_error = Some(err.to_string());
    }

    channels
        .commands
        .flush(|| MiniParams::from_voice(&app.voice));
    app.health.queue = channels.commands.status();
    app.health.display_dropped = channels.stats.display_dropped();
    app.health.stream_errors = channels.stats.stream_errors();
    app.health.stream_errors_unreported = channels.stats.stream_errors_unreported();
    app.hub = Some(channels.hub.status());
}

/// Route one key event: notes, panic, then navigation / parameter edits.
fn handle_key_event(app: &mut App, key: KeyEvent, commands: &mut CommandLink) {
    match key.kind {
        KeyEventKind::Press => {
            if let Some(note) = input::key_to_midi_note(key.code) {
                // Ignore key repeat — only trigger on initial press.
                let held = &mut app.held_notes[usize::from(note)];
                if !*held {
                    *held = true;
                    commands.note_on(note);
                }
            } else if key.code == KeyCode::Backspace {
                // Panic: silence the voice, forget every held key.
                app.release_all_notes();
                commands.all_notes_off();
                app.set_status("All notes off", false);
            } else if input::handle_key(app, key.code, key.modifiers) {
                // Sync parameter changes to the audio thread.
                commands.params(&MiniParams::from_voice(&app.voice));
            }
        }
        KeyEventKind::Release => {
            if let Some(note) = input::key_to_midi_note(key.code) {
                let held = &mut app.held_notes[usize::from(note)];
                if *held {
                    *held = false;
                    commands.note_off(note);
                }
            }
        }
        // Auto-repeat: a held note is already sounding.
        KeyEventKind::Repeat => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::AudioCommand;
    use crossterm::event::{KeyEventState, KeyModifiers};

    fn key(code: KeyCode, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind,
            state: KeyEventState::NONE,
        }
    }

    fn link() -> (CommandLink, Receiver<AudioCommand>) {
        let (tx, rx) = crossbeam_channel::bounded(16);
        (CommandLink::new(tx), rx)
    }

    #[test]
    fn note_key_press_and_release() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100);
        handle_key_event(
            &mut app,
            key(KeyCode::Char('a'), KeyEventKind::Press),
            &mut commands,
        );
        handle_key_event(
            &mut app,
            key(KeyCode::Char('a'), KeyEventKind::Press),
            &mut commands,
        );
        handle_key_event(
            &mut app,
            key(KeyCode::Char('a'), KeyEventKind::Release),
            &mut commands,
        );
        let got: Vec<AudioCommand> = rx.try_iter().collect();
        assert_eq!(got.len(), 2, "repeat press must not retrigger");
        assert!(matches!(got[0], AudioCommand::NoteOn { note: 48 }));
        assert!(matches!(got[1], AudioCommand::NoteOff { note: 48 }));
    }

    #[test]
    fn backspace_releases_everything() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100);
        handle_key_event(
            &mut app,
            key(KeyCode::Char('a'), KeyEventKind::Press),
            &mut commands,
        );
        handle_key_event(
            &mut app,
            key(KeyCode::Backspace, KeyEventKind::Press),
            &mut commands,
        );
        let got: Vec<AudioCommand> = rx.try_iter().collect();
        assert!(matches!(got.last(), Some(AudioCommand::AllNotesOff)));
        assert!(app.held_notes.iter().all(|&h| !h));
        // The key plays again straight away.
        handle_key_event(
            &mut app,
            key(KeyCode::Char('a'), KeyEventKind::Press),
            &mut commands,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(AudioCommand::NoteOn { note: 48 })
        ));
    }

    #[test]
    fn param_edit_sends_params_and_unhandled_key_does_not() {
        let (mut commands, rx) = link();
        let mut app = App::new(44100);
        handle_key_event(
            &mut app,
            key(KeyCode::Right, KeyEventKind::Press),
            &mut commands,
        );
        assert!(matches!(rx.try_recv(), Ok(AudioCommand::UpdateParams(_))));
        handle_key_event(
            &mut app,
            key(KeyCode::F(9), KeyEventKind::Press),
            &mut commands,
        );
        assert!(rx.try_recv().is_err());
    }
}
